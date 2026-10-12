//! Opt-in writes to temporary tables in the dedicated local Docker test databases.
use sqlshop::{
    db::Session,
    model::{CellUpdate, Credentials, Engine, Profile, Table, TableData},
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
        let snapshot = session.table_data(&table).await.unwrap();
        assert_eq!(snapshot.primary_keys, ["tenant", "id"]);
        assert_eq!(snapshot.keys[0], [Some("1".into()), Some("9223372036854775806".into())]);
        assert_eq!(cell(&snapshot, "amount", 0), Some("12.3456"));
        assert!(!snapshot.columns.iter().find(|c| c.name=="generated").unwrap().editable);
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
        session.query(&format!("DELETE FROM {qualified}")).await.unwrap();
        let empty = session.table_data(&table).await.unwrap();
        assert!(empty.output.sets[0].rows.is_empty());
        assert_eq!(empty.output.sets[0].columns.len(), 9);
        session.query(&format!("DROP TABLE {qualified}")).await.unwrap();
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
