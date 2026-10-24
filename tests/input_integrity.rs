//! Opt-in writes to isolated tables in the dedicated sqlshop_test database.
use sqlshop::{
    db::{ColumnFilter, FilterOperator, Session, TableBrowseOptions},
    model::{CellUpdate, Credentials, Engine, InsertValue, Profile, Table, TableData},
    recovery::VerificationState,
};

fn configuration(engine: Engine, prefix: &str) -> (Profile, Credentials) {
    let get = |name| std::env::var(format!("SQLSHOP_TEST_{prefix}_{name}")).unwrap();
    (
        Profile {
            engine,
            host: get("HOST"),
            port: get("PORT").parse().unwrap(),
            username: get("USER"),
            tls: false,
            ..Default::default()
        },
        Credentials {
            database_password: get("PASSWORD"),
            ..Default::default()
        },
    )
}

async fn connect(profile: &Profile, credentials: &Credentials) -> Session {
    let mut session = Session::connect(profile, credentials).await.unwrap();
    session.select_database("sqlshop_test").await.unwrap();
    session
}

fn change(data: &TableData, table: &Table, column: &str, value: &str) -> CellUpdate {
    let index = data.columns.iter().position(|c| c.name == column).unwrap();
    CellUpdate {
        database: "sqlshop_test".into(),
        table: table.clone(),
        primary_keys: data.primary_keys.clone(),
        key_values: data.keys[0].clone(),
        column: column.into(),
        original: data.output.sets[0].rows[0][index].clone(),
        value: Some(value.into()),
    }
}

fn cell<'a>(data: &'a TableData, column: &str) -> &'a str {
    let index = data.columns.iter().position(|c| c.name == column).unwrap();
    data.output.sets[0].rows[0][index].as_deref().unwrap()
}

async fn filtered_rows(session: &mut Session, table: &Table, column: &str, value: &str) -> usize {
    session
        .table_page(
            table,
            &TableBrowseOptions {
                filters: vec![ColumnFilter {
                    column: column.into(),
                    operator: FilterOperator::Eq,
                    value: Some(value.into()),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .data
        .output
        .sets[0]
        .rows
        .len()
}

#[tokio::test]
#[ignore = "Requires dedicated local PostgreSQL credentials; creates temporary test objects"]
async fn postgres_input_integrity() {
    let (profile, credentials) = configuration(Engine::Postgres, "PG");
    let mut setup = connect(&profile, &credentials).await;
    for compact in [false, true] {
        let sql = sqlshop::sql_editor::format_sql(
            "SELECT 'foo'\n'bar' AS joined",
            Engine::Postgres,
            compact,
        )
        .unwrap();
        let output = setup.query(&sql).await.unwrap();
        assert_eq!(output.sets[0].rows[0][0].as_deref(), Some("foobar"));
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let table = Table {
        schema: "public".into(),
        name: format!("input_integrity_{suffix}"),
        kind: "BASE TABLE".into(),
    };
    let qualified = format!("public.{}", table.name);
    let domain = format!("public.input_domain_{suffix}");
    let nested_domain = format!("public.input_nested_{suffix}");
    let enumeration = format!("public.input_enum_{suffix}");
    setup
        .query("CREATE EXTENSION IF NOT EXISTS citext")
        .await
        .unwrap();
    setup.query(&format!("CREATE DOMAIN {domain} AS varchar(5) CHECK(VALUE <> 'block'); CREATE DOMAIN {nested_domain} AS {domain}; CREATE TYPE {enumeration} AS ENUM ('red','blue'); CREATE TABLE {qualified}(id integer PRIMARY KEY,label varchar(5),fixed char(5),domain_value {nested_domain},amount numeric(8,2),phase {enumeration},doc json,markup xml,ci citext); INSERT INTO {qualified} VALUES(1,'old','old','old',1.25,'red','{{ \"a\":1 }}','<a/>','foo')")).await.unwrap();
    let mut session = connect(&profile, &credentials).await;

    let initial = session.table_data(&table).await.unwrap();
    assert!(
        session
            .update_cell(&change(&initial, &table, "label", "abcdefgh"))
            .await
            .is_err()
    );
    for (column, value) in [
        ("fixed", "abcdefgh"),
        ("domain_value", "abcdefgh"),
        ("domain_value", "block"),
    ] {
        assert!(
            session
                .save_table_draft(
                    "sqlshop_test",
                    &table,
                    &[change(&initial, &table, column, value)],
                    &[],
                    &[]
                )
                .await
                .is_err()
        );
    }
    let unchanged = session.table_data(&table).await.unwrap();
    for column in ["label", "fixed", "domain_value"] {
        assert_eq!(cell(&unchanged, column), "old");
    }

    // An error in a later CSV-style row must roll back earlier inserts.
    let names = vec!["id".into(), "label".into()];
    assert!(
        session
            .insert_rows(
                "sqlshop_test",
                &table,
                &names,
                &[
                    vec![
                        InsertValue::Text("2".into()),
                        InsertValue::Text("good".into())
                    ],
                    vec![
                        InsertValue::Text("3".into()),
                        InsertValue::Text("abcdefgh".into())
                    ],
                ]
            )
            .await
            .is_err()
    );
    assert_eq!(session.table_data(&table).await.unwrap().keys.len(), 1);
    assert!(
        session
            .save_table_draft(
                "sqlshop_test",
                &table,
                &[change(&initial, &table, "label", "fresh")],
                &names,
                &[vec![
                    InsertValue::Text("2".into()),
                    InsertValue::Text("abcdefgh".into())
                ],]
            )
            .await
            .is_err()
    );
    assert_eq!(
        cell(&session.table_data(&table).await.unwrap(), "label"),
        "old"
    );

    let initial = session.table_data(&table).await.unwrap();
    let edits = vec![
        change(&initial, &table, "label", "abcde"),
        change(&initial, &table, "amount", "42.75"),
        change(&initial, &table, "phase", "blue"),
        change(&initial, &table, "domain_value", "new"),
        change(&initial, &table, "doc", "{\"b\":2}"),
        change(&initial, &table, "markup", "<b/>"),
    ];
    let before = session
        .verify_table_draft_with_columns("sqlshop_test", &table, &edits, &[], &[], &initial.columns)
        .await
        .unwrap();
    assert_eq!(before.rows[0].state, VerificationState::Original);
    session
        .save_table_draft("sqlshop_test", &table, &edits, &[], &[])
        .await
        .unwrap();
    let applied = session.table_data(&table).await.unwrap();
    assert_eq!(cell(&applied, "amount"), "42.75");
    assert_eq!(cell(&applied, "phase"), "blue");
    assert_eq!(cell(&applied, "domain_value"), "new");
    assert_eq!(cell(&applied, "doc"), "{\"b\":2}");
    assert_eq!(cell(&applied, "markup"), "<b/>");
    let after = session
        .verify_table_draft_with_columns("sqlshop_test", &table, &edits, &[], &[], &initial.columns)
        .await
        .unwrap();
    assert_eq!(after.rows[0].state, VerificationState::Intended);
    assert_eq!(
        filtered_rows(&mut session, &table, "label", "abcdefgh").await,
        0
    );

    // citext equality considers foo and FOO equal, but an optimistic check must
    // protect the exact spelling read by the user.
    let stale = change(&applied, &table, "ci", "bar");
    setup
        .query(&format!("UPDATE {qualified} SET ci='FOO' WHERE id=1"))
        .await
        .unwrap();
    assert!(
        session
            .save_table_draft(
                "sqlshop_test",
                &table,
                std::slice::from_ref(&stale),
                &[],
                &[]
            )
            .await
            .is_err()
    );
    assert_eq!(
        cell(&session.table_data(&table).await.unwrap(), "ci"),
        "FOO"
    );
    let conflict = session
        .verify_table_draft_with_columns(
            "sqlshop_test",
            &table,
            &[stale],
            &[],
            &[],
            &applied.columns,
        )
        .await
        .unwrap();
    assert_eq!(conflict.rows[0].state, VerificationState::Conflict);
    let fresh = session.table_data(&table).await.unwrap();
    let ci = change(&fresh, &table, "ci", "bAr");
    session.update_cell(&ci).await.unwrap();
    let verified = session
        .verify_table_draft_with_columns("sqlshop_test", &table, &[ci], &[], &[], &fresh.columns)
        .await
        .unwrap();
    assert_eq!(verified.rows[0].state, VerificationState::Intended);

    // JSON whitespace is part of the stored representation as well.
    let stale_json = change(&applied, &table, "doc", "{\"c\":3}");
    setup
        .query(&format!(
            "UPDATE {qualified} SET doc='{{ \"b\":2 }}' WHERE id=1"
        ))
        .await
        .unwrap();
    assert!(session.update_cell(&stale_json).await.is_err());
    setup.query(&format!("DROP TABLE {qualified}; DROP DOMAIN {nested_domain}; DROP DOMAIN {domain}; DROP TYPE {enumeration}")).await.unwrap();
}

#[tokio::test]
#[ignore = "Requires dedicated local SQL Server credentials; creates a temporary test table"]
async fn sqlserver_input_integrity() {
    let (profile, credentials) = configuration(Engine::SqlServer, "MSSQL");
    let mut setup = connect(&profile, &credentials).await;
    let table = Table {
        schema: "dbo".into(),
        name: format!("input_integrity_{}", uuid::Uuid::new_v4().simple()),
        kind: "BASE TABLE".into(),
    };
    let qualified = format!("dbo.{}", table.name);
    setup.query(&format!("CREATE TABLE {qualified}(id int PRIMARY KEY,label nvarchar(5),narrow varchar(5) COLLATE Latin1_General_100_CI_AS,fixed nchar(5),payload varbinary(2),fixed_payload binary(2),han varchar(4) COLLATE Chinese_PRC_CI_AS,han_fixed char(4) COLLATE Chinese_PRC_CI_AS,utf8 varchar(4) COLLATE Latin1_General_100_CI_AS_SC_UTF8); INSERT INTO {qualified} VALUES(1,N'abcde','abcde',N'old',0x0102,0x0102,N'旧',N'旧','old')")).await.unwrap();
    let mut session = connect(&profile, &credentials).await;
    let initial = session.table_data(&table).await.unwrap();
    for (column, value) in [
        ("label", "abcdef"),
        ("label", "😀😀😀"),
        ("narrow", "abcdef"),
        ("narrow", "汉"),
        ("han", "汉字串"),
        ("utf8", "😀 "),
        ("payload", "0x010203"),
        ("fixed_payload", "0x010203"),
    ] {
        assert!(
            session
                .update_cell(&change(&initial, &table, column, value))
                .await
                .is_err()
        );
        assert!(
            session
                .save_table_draft(
                    "sqlshop_test",
                    &table,
                    &[change(&initial, &table, column, value)],
                    &[],
                    &[]
                )
                .await
                .is_err()
        );
    }
    assert_eq!(
        filtered_rows(&mut session, &table, "label", "abcdef").await,
        0
    );
    assert_eq!(
        filtered_rows(&mut session, &table, "narrow", "abcdef").await,
        0
    );
    assert_eq!(
        filtered_rows(&mut session, &table, "payload", "0x010203").await,
        0
    );
    let names = vec!["id".into(), "payload".into()];
    assert!(
        session
            .insert_rows(
                "sqlshop_test",
                &table,
                &names,
                &[
                    vec![
                        InsertValue::Text("2".into()),
                        InsertValue::Text("0x0102".into())
                    ],
                    vec![
                        InsertValue::Text("3".into()),
                        InsertValue::Text("0x010203".into())
                    ],
                ]
            )
            .await
            .is_err()
    );
    assert_eq!(session.table_data(&table).await.unwrap().keys.len(), 1);
    // The same Unicode input is valid in one column's code page and lossy in
    // another. Neither the database default nor an ASCII-only policy is enough.
    let names = vec!["id".into(), "narrow".into()];
    assert!(
        session
            .insert_rows(
                "sqlshop_test",
                &table,
                &names,
                &[
                    vec![
                        InsertValue::Text("2".into()),
                        InsertValue::Text("good".into())
                    ],
                    vec![
                        InsertValue::Text("3".into()),
                        InsertValue::Text("汉".into())
                    ],
                ]
            )
            .await
            .is_err()
    );
    assert_eq!(session.table_data(&table).await.unwrap().keys.len(), 1);
    assert!(
        session
            .save_table_draft(
                "sqlshop_test",
                &table,
                &[
                    change(&initial, &table, "label", "fresh"),
                    change(&initial, &table, "narrow", "汉"),
                ],
                &[],
                &[]
            )
            .await
            .is_err()
    );
    assert_eq!(
        cell(&session.table_data(&table).await.unwrap(), "label"),
        "abcde"
    );
    for (column, value) in [
        ("narrow", "é"),
        ("han", "汉字"),
        ("han_fixed", "汉"),
        ("utf8", "😀"),
    ] {
        let snapshot = session.table_data(&table).await.unwrap();
        let edit = change(&snapshot, &table, column, value);
        session.update_cell(&edit).await.unwrap();
        let verified = session
            .verify_table_draft_with_columns(
                "sqlshop_test",
                &table,
                &[edit],
                &[],
                &[],
                &snapshot.columns,
            )
            .await
            .unwrap();
        assert_eq!(
            verified.rows[0].state,
            VerificationState::Intended,
            "column {column}"
        );
    }
    let snapshot = session.table_data(&table).await.unwrap();
    let lossy = change(&snapshot, &table, "narrow", "汉");
    setup
        .query(&format!("UPDATE {qualified} SET narrow='?' WHERE id=1"))
        .await
        .unwrap();
    let verified = session
        .verify_table_draft_with_columns(
            "sqlshop_test",
            &table,
            &[lossy],
            &[],
            &[],
            &snapshot.columns,
        )
        .await
        .unwrap();
    assert_eq!(verified.rows[0].state, VerificationState::Conflict);
    let fixed = change(&initial, &table, "fixed", "X");
    session.update_cell(&fixed).await.unwrap();
    let verified = session
        .verify_table_draft_with_columns(
            "sqlshop_test",
            &table,
            &[fixed],
            &[],
            &[],
            &initial.columns,
        )
        .await
        .unwrap();
    assert_eq!(verified.rows[0].state, VerificationState::Intended);
    let current = session.table_data(&table).await.unwrap();
    let binary = change(&current, &table, "fixed_payload", "0x01");
    session.update_cell(&binary).await.unwrap();
    let verified = session
        .verify_table_draft_with_columns(
            "sqlshop_test",
            &table,
            &[binary],
            &[],
            &[],
            &current.columns,
        )
        .await
        .unwrap();
    assert_eq!(verified.rows[0].state, VerificationState::Intended);
    session.query("SET ANSI_WARNINGS OFF").await.unwrap();
    let current = session.table_data(&table).await.unwrap();
    assert!(
        session
            .update_cell(&change(&current, &table, "label", "fresh"))
            .await
            .is_err()
    );
    assert_eq!(
        cell(&session.table_data(&table).await.unwrap(), "label"),
        "abcde"
    );
    setup
        .query(&format!("DROP TABLE {qualified}"))
        .await
        .unwrap();
}
