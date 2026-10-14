//! Opt-in writes to temporary tables in the dedicated local Docker test databases.
use sqlshop::{
    db::{
        ColumnFilter, FilterOperator, Session, StructureChange, TableBrowseOptions,
        structure_change_sql,
    },
    model::{
        CellUpdate, Credentials, Engine, InsertValue, Profile, Table, TableData, parse_nullable_csv,
    },
};

fn change(data: &TableData, table: &Table, column: &str, value: Option<&str>) -> CellUpdate {
    let index = data.columns.iter().position(|c| c.name == column).unwrap();
    CellUpdate {
        database: "sqlshop_test".into(),
        table: table.clone(),
        primary_keys: data.primary_keys.clone(),
        key_values: data.keys[0].clone(),
        column: column.into(),
        original: data.output.sets[0].rows[0][index].clone(),
        value: value.map(str::to_owned),
    }
}

fn cell<'a>(data: &'a TableData, column: &str, row: usize) -> Option<&'a str> {
    let index = data.columns.iter().position(|c| c.name == column).unwrap();
    data.output.sets[0].rows[row][index].as_deref()
}

async fn exercise_new_workflows(session: &mut Session, engine: Engine, table: &Table) {
    let names = vec!["tenant".into(), "id".into(), "name".into(), "note".into()];
    let rows = (0..202)
        .map(|i| {
            vec![
                InsertValue::Text("3".into()),
                InsertValue::Text(i.to_string()),
                InsertValue::Text(format!("added {i}")),
                if i == 0 {
                    InsertValue::Null
                } else if i == 1 {
                    InsertValue::Text("NULL".into())
                } else if i == 2 {
                    InsertValue::Text("".into())
                } else {
                    InsertValue::Text(format!("note {i}"))
                },
            ]
        })
        .collect::<Vec<_>>();
    assert_eq!(
        session
            .insert_rows("sqlshop_test", table, &names, &rows)
            .await
            .unwrap(),
        rows.len()
    );
    let data = session.table_data(table).await.unwrap();
    assert_eq!(data.output.sets[0].rows.len(), 200);
    let row_for = |id: &str| {
        data.keys
            .iter()
            .position(|keys| keys[0].as_deref() == Some("3") && keys[1].as_deref() == Some(id))
            .unwrap()
    };
    assert_eq!(cell(&data, "note", row_for("0")), None);
    assert_eq!(cell(&data, "note", row_for("1")), Some("NULL"));
    assert_eq!(cell(&data, "note", row_for("2")), Some(""));

    let initial_export = tempfile::tempdir().unwrap();
    let initial_path = initial_export.path().join("initial.csv");
    assert_eq!(
        session
            .export_table_csv(
                "sqlshop_test",
                table,
                &TableBrowseOptions::default(),
                &initial_path
            )
            .await
            .unwrap(),
        202
    );
    let (headers, initial_rows) =
        parse_nullable_csv(&std::fs::read(&initial_path).unwrap()).unwrap();
    assert_eq!(headers, data.output.sets[0].columns);
    assert_eq!(initial_rows.len(), 202);
    let exported_row = |id: &str| {
        initial_rows
            .iter()
            .find(|row| {
                row[0] == InsertValue::Text("3".into()) && row[1] == InsertValue::Text(id.into())
            })
            .unwrap()
    };
    assert_eq!(exported_row("0")[3], InsertValue::Null);
    assert_eq!(exported_row("1")[3], InsertValue::Text("NULL".into()));
    assert_eq!(exported_row("2")[3], InsertValue::Text("".into()));
    assert_eq!(exported_row("201")[3], InsertValue::Text("note 201".into()));

    let mut first = change(&data, table, "name", Some("batch saved"));
    first.key_values = data.keys[row_for("0")].clone();
    first.original = cell(&data, "name", row_for("0")).map(str::to_owned);
    let mut second = change(&data, table, "note", Some("batch saved too"));
    second.key_values = data.keys[row_for("1")].clone();
    second.original = cell(&data, "note", row_for("1")).map(str::to_owned);
    assert_eq!(
        session
            .update_cells(&[first.clone(), second.clone()])
            .await
            .unwrap(),
        2
    );
    let updated = session.table_data(table).await.unwrap();
    assert_eq!(cell(&updated, "name", row_for("0")), Some("batch saved"));
    assert_eq!(cell(&updated, "note", row_for("0")), None);
    assert_eq!(
        cell(&updated, "note", row_for("1")),
        Some("batch saved too")
    );
    let mut stale = second;
    stale.value = Some("must roll back".into());
    let mut first_again = first;
    first_again.original = Some("batch saved".into());
    first_again.value = Some("must roll back too".into());
    assert!(session.update_cells(&[first_again, stale]).await.is_err());
    session.ping().await.unwrap();
    let rolled_back = session.table_data(table).await.unwrap();
    assert_eq!(
        cell(&rolled_back, "name", row_for("0")),
        Some("batch saved")
    );
    assert_eq!(cell(&rolled_back, "note", row_for("0")), None);
    assert_eq!(
        cell(&rolled_back, "note", row_for("1")),
        Some("batch saved too")
    );

    // Both components of a composite primary key and a regular cell change in
    // one row. Order in which the user staged these edits must not matter.
    let options = |tenant: &str, id: &str| TableBrowseOptions {
        filters: vec![
            ColumnFilter {
                column: "tenant".into(),
                operator: FilterOperator::Eq,
                value: Some(tenant.into()),
            },
            ColumnFilter {
                column: "id".into(),
                operator: FilterOperator::Eq,
                value: Some(id.into()),
            },
        ],
        ..Default::default()
    };
    for reverse in [false, true] {
        let before = session
            .table_page(table, &options("3", "5"))
            .await
            .unwrap()
            .data;
        let mut changes = vec![
            change(&before, table, "tenant", Some("4")),
            change(&before, table, "id", Some("205")),
            change(&before, table, "note", Some("changed with key")),
        ];
        if reverse {
            changes.reverse();
        }
        assert_eq!(session.update_cells(&changes).await.unwrap(), 3);
        let after = session
            .table_page(table, &options("4", "205"))
            .await
            .unwrap()
            .data;
        assert_eq!(after.output.sets[0].rows.len(), 1);
        assert_eq!(cell(&after, "note", 0), Some("changed with key"));
        assert!(
            session
                .table_page(table, &options("3", "5"))
                .await
                .unwrap()
                .data
                .output
                .sets[0]
                .rows
                .is_empty()
        );
        let restore = [
            change(&after, table, "tenant", Some("3")),
            change(&after, table, "id", Some("5")),
            change(&after, table, "note", Some("note 5")),
        ];
        assert_eq!(session.update_cells(&restore).await.unwrap(), 3);
        let before = session
            .table_page(table, &options("3", "5"))
            .await
            .unwrap()
            .data;
        let mut stale = change(&before, table, "note", Some("must not save"));
        stale.original = Some("stale value".into());
        assert!(
            session
                .update_cells(&[change(&before, table, "id", Some("205")), stale])
                .await
                .is_err()
        );
        let preserved = session
            .table_page(table, &options("3", "5"))
            .await
            .unwrap()
            .data;
        assert_eq!(cell(&preserved, "note", 0), Some("note 5"));
    }

    let column = StructureChange::AddColumn {
        name: "extra_note".into(),
        data_type: "VARCHAR".into(),
        size: "40".into(),
    };
    let preview = structure_change_sql(engine, table, &column).unwrap();
    assert!(
        session
            .change_structure("sqlshop_test", table, &column, "SELECT 1")
            .await
            .is_err()
    );
    assert!(
        !session
            .table_structure(table)
            .await
            .unwrap()
            .columns
            .rows
            .iter()
            .any(|row| row[1].as_deref() == Some("extra_note"))
    );
    session
        .change_structure("sqlshop_test", table, &column, &preview)
        .await
        .unwrap();
    let structure = session.table_structure(table).await.unwrap();
    assert!(
        structure
            .columns
            .rows
            .iter()
            .any(|row| row[1].as_deref() == Some("extra_note"))
    );
    let index_name = format!("ix_{}", table.name);
    let index = StructureChange::CreateIndex {
        name: index_name.clone(),
        columns: vec!["extra_note".into()],
    };
    let preview = structure_change_sql(engine, table, &index).unwrap();
    assert!(
        session
            .change_structure("sqlshop_test", table, &index, "SELECT 1")
            .await
            .is_err()
    );
    assert!(
        !session
            .table_structure(table)
            .await
            .unwrap()
            .indexes
            .rows
            .iter()
            .any(|row| row[0].as_deref() == Some(&index_name))
    );
    session
        .change_structure("sqlshop_test", table, &index, &preview)
        .await
        .unwrap();
    let structure = session.table_structure(table).await.unwrap();
    assert!(structure.indexes.rows.iter().any(|row| {
        row[0].as_deref() == Some(&index_name)
            && match engine {
                Engine::Postgres => row[4]
                    .as_deref()
                    .is_some_and(|definition| definition.contains("extra_note")),
                _ => row
                    .iter()
                    .any(|field| field.as_deref() == Some("extra_note")),
            }
    }));
}

async fn exercise_postgres_check_dependency(session: &mut Session, table: &Table) {
    let qualified = format!(
        "{}.{}",
        Engine::Postgres.quote(&table.schema),
        Engine::Postgres.quote(&table.name)
    );
    let constraint = format!("check_{}", uuid::Uuid::new_v4().simple());
    session
        .query(&format!(
            "ALTER TABLE {qualified} ADD CONSTRAINT {constraint} CHECK (note IS NULL OR name <> '')"
        ))
        .await
        .unwrap();
    let action = StructureChange::DropColumn {
        name: "note".into(),
    };
    let preview = structure_change_sql(Engine::Postgres, table, &action).unwrap();
    let error = session
        .change_structure("sqlshop_test", table, &action, &preview)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("CHECK"));
    assert!(
        session
            .table_columns(table)
            .await
            .unwrap()
            .iter()
            .any(|column| column.name == "note")
    );
    let constraints = session.query(&format!("SELECT conname::text FROM pg_constraint WHERE conname='{constraint}' AND conrelid='{qualified}'::regclass")).await.unwrap();
    assert_eq!(constraints.sets[0].rows.len(), 1);
    // An unrelated column remains removable; constraints are not a blanket ban.
    for action in [
        StructureChange::AddColumn {
            name: "safe_drop".into(),
            data_type: "INTEGER".into(),
            size: String::new(),
        },
        StructureChange::DropColumn {
            name: "safe_drop".into(),
        },
    ] {
        let preview = structure_change_sql(Engine::Postgres, table, &action).unwrap();
        session
            .change_structure("sqlshop_test", table, &action, &preview)
            .await
            .unwrap();
    }
    session
        .query(&format!(
            "ALTER TABLE {qualified} DROP CONSTRAINT {constraint}"
        ))
        .await
        .unwrap();
}

async fn exercise_mysql_write_safety(profile: &Profile, credentials: &Credentials) {
    let mut setup = Session::connect(profile, credentials).await.unwrap();
    setup.select_database("sqlshop_test").await.unwrap();
    let table = Table {
        schema: "sqlshop_test".into(),
        name: format!("write_safety_{}", uuid::Uuid::new_v4().simple()),
        kind: "BASE TABLE".into(),
    };
    let qualified = format!("sqlshop_test.{}", Engine::MySql.quote(&table.name));
    setup.query(&format!("CREATE TABLE {qualified} (id INT PRIMARY KEY) ENGINE=InnoDB; INSERT INTO {qualified} VALUES (1),(2)")).await.unwrap();
    let mut session = Session::connect(profile, credentials).await.unwrap();
    session.select_database("sqlshop_test").await.unwrap();
    let snapshot = session.table_data(&table).await.unwrap();
    assert!(snapshot.can_delete_rows());
    // Revalidate against the database when the engine changes after display.
    setup
        .query(&format!("ALTER TABLE {qualified} ENGINE=MyISAM"))
        .await
        .unwrap();
    assert!(
        session
            .delete_rows(
                "sqlshop_test",
                &table,
                &snapshot.primary_keys,
                &[snapshot.keys[0].clone(), vec![Some("999".into())]]
            )
            .await
            .is_err()
    );
    assert!(
        session
            .insert_rows("sqlshop_test", &table, &[], &[vec![]])
            .await
            .is_err()
    );
    assert!(
        session
            .insert_default_row("sqlshop_test", &table)
            .await
            .is_err()
    );
    let preserved = session.table_data(&table).await.unwrap();
    assert!(!preserved.can_delete_rows());
    assert_eq!(preserved.output.sets[0].rows.len(), 2);
    setup.query(&format!("DROP TABLE {qualified}; CREATE TABLE {qualified} (id INT PRIMARY KEY AUTO_INCREMENT) ENGINE=InnoDB")).await.unwrap();
    session
        .insert_default_row("sqlshop_test", &table)
        .await
        .unwrap();
    session
        .insert_rows("sqlshop_test", &table, &[], &[vec![]])
        .await
        .unwrap();
    let identity_only = session.table_data(&table).await.unwrap();
    assert!(identity_only.columns.iter().all(|column| !column.editable));
    assert!(identity_only.can_delete_rows());
    assert_eq!(
        session
            .delete_rows(
                "sqlshop_test",
                &table,
                &identity_only.primary_keys,
                &identity_only.keys
            )
            .await
            .unwrap(),
        2
    );
    setup
        .query(&format!("DROP TABLE {qualified}"))
        .await
        .unwrap();
}

fn exercise(engine: Engine, prefix: &str) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let get = |field| std::env::var(format!("SQLSHOP_TEST_{prefix}_{field}")).unwrap();
        let profile = Profile { engine, host: get("HOST"), port: get("PORT").parse().unwrap(), username: get("USER"), tls: false, ..Default::default() };
        let credentials = Credentials { database_password: get("PASSWORD"), ..Default::default() };
        let mut session = Session::connect(&profile, &credentials).await.unwrap();
        session.select_database("sqlshop_test").await.unwrap();
        let table = Table { schema: match engine { Engine::MySql => "sqlshop_test", Engine::Postgres => "public", Engine::SqlServer => "dbo" }.into(),
            name: format!("cell_edit_{}", uuid::Uuid::new_v4().simple()), kind: "BASE TABLE".into() };
        let qualified = format!("{}.{}", engine.quote(&table.schema), engine.quote(&table.name));
        let fields = match engine {
            Engine::Postgres => "tenant integer, id bigint, name text NOT NULL, note text, amount numeric(20,4), payload bytea, observed_at timestamp(6), score double precision, computed_label text GENERATED ALWAYS AS (name || '!') STORED",
            Engine::MySql => "tenant integer, id bigint, name varchar(100) NOT NULL, note text, amount decimal(20,4), payload blob, observed_at datetime(6), score double, computed_label varchar(110) GENERATED ALWAYS AS (CONCAT(name,'!')) STORED",
            Engine::SqlServer => "tenant int, id bigint, name nvarchar(100) NOT NULL, note nvarchar(max), amount decimal(20,4), payload varbinary(max), observed_at datetime2(6), score float, computed_label AS CONCAT(name,'!')",
        };
        session.query(&format!("CREATE TABLE {qualified} ({fields}, PRIMARY KEY(tenant,id))")).await.unwrap();
        let binary = match engine { Engine::Postgres => "decode('00ff','hex')", _ => "0x00ff" };
        session.query(&format!("INSERT INTO {qualified} (tenant,id,name,note,amount,payload,observed_at,score) VALUES (1,9223372036854775806,'first',NULL,12.3456,{binary},'2026-10-12 12:34:56.123456',0.1),(2,9223372036854775806,'second',NULL,12.3456,{binary},'2026-10-12 12:34:56.123456',0.1)")).await.unwrap();
        // Arbitrary setup SQL has its own physical connection. In particular,
        // MySQL editing is conservatively disabled after user SQL on a session.
        drop(session);
        let mut session = Session::connect(&profile, &credentials).await.unwrap();
        session.select_database("sqlshop_test").await.unwrap();
        let snapshot = session.table_data(&table).await.unwrap();
        assert!(!session.raw_session_requires_confirmation());
        assert_eq!(snapshot.output.sets[0].column_types.len(), snapshot.columns.len());
        if engine == Engine::MySql {
            for (name, declared, cast) in [("name", "varchar(100)", "CHAR CHARACTER SET utf8mb4"), ("amount", "decimal(20,4)", "decimal(20,4)"), ("observed_at", "datetime(6)", "DATETIME(6)")] {
                let index = snapshot.columns.iter().position(|c| c.name == name).unwrap();
                assert_eq!(snapshot.columns[index].declared_type, declared);
                assert_eq!(snapshot.output.sets[0].column_types[index], declared);
                assert_eq!(snapshot.columns[index].sql_type, cast);
            }
        }
        assert_eq!(snapshot.primary_keys, ["tenant", "id"]);
        assert_eq!(snapshot.keys[0], [Some("1".into()), Some("9223372036854775806".into())]);
        assert_eq!(cell(&snapshot, "amount", 0), Some("12.3456"));
        assert!(!snapshot.columns.iter().find(|c| c.name=="computed_label").unwrap().editable);
        assert!(session.update_cell(&change(&snapshot, &table, "computed_label", Some("blocked"))).await.is_err());
        let new_name = "中文 ' ; UPDATE never_run --";
        session.update_cell(&change(&snapshot, &table, "name", Some(new_name))).await.unwrap();
        let data = session.table_data(&table).await.unwrap();
        assert_eq!(cell(&data, "name", 0), Some(new_name));
        assert_eq!(cell(&data, "name", 1), Some("second"));
        assert!(session.update_cell(&change(&snapshot, &table, "name", Some("stale"))).await.is_err());
        session.ping().await.unwrap();
        assert_eq!(cell(&session.table_data(&table).await.unwrap(), "name", 0), Some(new_name));
        for (column, value) in [("note", Some("")), ("note", Some("nullable value")), ("note", None),
            ("amount", Some("9876543210.1234")), ("payload", Some("0x0001ff")),
            ("observed_at", Some("2026-10-13 01:02:03.654321")), ("score", Some("1.23456789012345"))] {
            let data = session.table_data(&table).await.unwrap();
            session.update_cell(&change(&data, &table, column, value)).await.unwrap();
            let actual = session.table_data(&table).await.unwrap();
            if column == "payload" { assert_eq!(cell(&actual, column, 0).unwrap().to_ascii_lowercase(), value.unwrap()); }
            else if column == "score" { assert!((cell(&actual, column, 0).unwrap().parse::<f64>().unwrap() - 1.23456789012345).abs() < 1e-14); }
            else { assert_eq!(cell(&actual, column, 0), value); }
        }
        let data = session.table_data(&table).await.unwrap();
        assert!(session.update_cell(&change(&data, &table, "amount", Some("invalid-number"))).await.is_err());
        session.ping().await.unwrap();
        assert_eq!(cell(&session.table_data(&table).await.unwrap(), "amount", 0), Some("9876543210.1234"));
        let mut wrong_database = change(&data, &table, "name", Some("wrong"));
        wrong_database.database = "sqlshop_other".into();
        assert!(session.update_cell(&wrong_database).await.is_err());
        let no_key = Table { name: "navigation_probe".into(), ..table.clone() };
        let read_only = session.table_data(&no_key).await.unwrap();
        assert!(read_only.primary_keys.is_empty());
        assert!(session.update_cell(&change(&read_only, &no_key, "marker", Some("blocked"))).await.is_err());
        // Reject edits in a user transaction without committing or rolling it back.
        let data = session.table_data(&table).await.unwrap();
        session.query(match engine { Engine::SqlServer => "BEGIN TRANSACTION", _ => "BEGIN" }).await.unwrap();
        session.query(&format!("UPDATE {qualified} SET note='uncommitted' WHERE tenant=1")).await.unwrap();
        assert!(session.update_cell(&change(&data, &table, "name", Some("must-not-save"))).await.is_err());
        session.query("ROLLBACK").await.unwrap();
        assert_eq!(cell(&session.table_data(&table).await.unwrap(), "note", 0), None);
        if engine == Engine::MySql {
            assert!(session.insert_rows("sqlshop_test", &table, &["note".into()], &[vec![InsertValue::Null]]).await.is_err());
            let preview = structure_change_sql(engine, &table, &StructureChange::AddColumn {
                name: "must_not_exist".into(), data_type: "INT".into(), size: "".into()
            }).unwrap();
            assert!(session.change_structure("sqlshop_test", &table, &StructureChange::AddColumn {
                name: "must_not_exist".into(), data_type: "INT".into(), size: "".into()
            }, &preview).await.is_err());
        }
        if engine == Engine::Postgres {
            // The savepoint probe also rejects aborted transactions without
            // ending them; ROLLBACK is still the caller's responsibility.
            session.query("BEGIN").await.unwrap();
            assert!(session.query("SELECT 1/0").await.is_err());
            assert!(session.update_cell(&change(&data, &table, "name", Some("blocked"))).await.is_err());
            assert!(session.query("SELECT 1").await.is_err());
            session.query("ROLLBACK").await.unwrap();
            session.update_cell(&change(&data, &table, "name", Some("after-rollback"))).await.unwrap();
        }
        if engine == Engine::MySql {
            assert!(session.update_cell(&change(&data, &table, "name", Some("raw-only"))).await.is_err());
        }
        session.query(&format!("DELETE FROM {qualified}")).await.unwrap();
        let empty = session.table_data(&table).await.unwrap();
        assert!(empty.output.sets[0].rows.is_empty());
        assert_eq!(empty.output.sets[0].columns.len(), 9);
        assert_eq!(empty.output.sets[0].column_types, snapshot.output.sets[0].column_types);
        // Structured writes on MySQL require a fresh connection after arbitrary SQL.
        drop(session);
        let mut session = Session::connect(&profile, &credentials).await.unwrap();
        session.select_database("sqlshop_test").await.unwrap();
        exercise_new_workflows(&mut session, engine, &table).await;
        if engine == Engine::Postgres {
            exercise_postgres_check_dependency(&mut session, &table).await;
        }
        drop(session);
        let mut cleanup = Session::connect(&profile, &credentials).await.unwrap();
        cleanup.select_database("sqlshop_test").await.unwrap();
        cleanup.query(&format!("DROP TABLE {qualified}")).await.unwrap();
        if engine == Engine::MySql {
            exercise_mysql_write_safety(&profile, &credentials).await;
        }
    });
}

#[test]
#[ignore = "Requires dedicated local Docker MySQL credentials"]
fn mysql() {
    exercise(Engine::MySql, "MYSQL");
}
#[test]
#[ignore = "Requires dedicated local Docker PostgreSQL credentials"]
fn postgres() {
    exercise(Engine::Postgres, "PG");
}
#[test]
#[ignore = "Requires dedicated local Docker SQL Server credentials"]
fn sqlserver() {
    exercise(Engine::SqlServer, "MSSQL");
}
