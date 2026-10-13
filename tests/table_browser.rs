//! Opt-in tests: only writes UUID-named fixtures in dedicated sqlshop_test databases.
use sqlshop::{
    db::{
        ColumnFilter, ColumnSort, FilterOperator, Session, TableBrowseOptions,
        query_error_preserves_session,
    },
    model::{Credentials, Engine, Profile, Table},
};

fn exercise(engine: Engine, prefix: &str) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let get = |name| std::env::var(format!("SQLSHOP_TEST_{prefix}_{name}")).unwrap();
        let profile = Profile { engine, host: get("HOST"), port: get("PORT").parse().unwrap(), username: get("USER"), tls: false, ..Default::default() };
        let credentials = Credentials { database_password: get("PASSWORD"), ..Default::default() };
        let mut session = Session::connect(&profile, &credentials).await.unwrap();
        session.select_database("sqlshop_test").await.unwrap();
        let table = Table { schema: match engine { Engine::MySql => "sqlshop_test", Engine::Postgres => "public", Engine::SqlServer => "dbo" }.into(), name: format!("browse_{}",uuid::Uuid::new_v4().simple()), kind: "BASE TABLE".into() };
        let name = format!("{}.{}",engine.quote(&table.schema),engine.quote(&table.name));
        session.query(&format!("CREATE TABLE {name} (id int NOT NULL PRIMARY KEY, parent_id int, label varchar(100), score int DEFAULT 7, CONSTRAINT {} FOREIGN KEY (parent_id) REFERENCES {name}(id))", engine.quote(&format!("fk_{}", table.name)))).await.unwrap();
        session.query(&format!("CREATE INDEX {} ON {name}(label,score)",engine.quote(&format!("ix_{}",table.name)))).await.unwrap();
        session.query(&format!("INSERT INTO {name}(id,parent_id,label,score) VALUES (1,NULL,'same',1),(2,1,'same',2),(3,1,'100%_literal',3),(4,1,NULL,4),(5,1,'quote''; --',5)")).await.unwrap();
        let mut options = TableBrowseOptions { page_size: 2, sorts: vec![ColumnSort { column: "label".into(), descending: false }], ..Default::default() };
        if engine == Engine::MySql {
            session.query(&format!("ALTER TABLE {name} CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")).await.unwrap();
            let filtered = TableBrowseOptions {
                filters: vec![ColumnFilter { column: "label".into(), operator: FilterOperator::Eq, value: Some("same".into()) }],
                ..Default::default()
            };
            for collation in ["utf8mb4_general_ci", "utf8mb4_unicode_ci"] {
                session.query(&format!("SET collation_connection = '{collation}'")).await.unwrap();
                let page = session.table_page(&table, &filtered).await.unwrap();
                assert_eq!(page.data.keys, vec![vec![Some("1".into())], vec![Some("2".into())]]);
            }
        }
        let first = session.table_page(&table,&options).await.unwrap();
        if engine == Engine::MySql {
            assert!(first.sql.starts_with("SELECT `id`, `parent_id`, `label`, `score` FROM "));
            assert!(!first.sql.contains("CAST"));
            assert!(!first.sql.contains("COLLATE"));
        }
        assert_eq!(first.data.output.sets[0].rows.len(),2); assert!(first.has_more);
        let headers = &first.data.output.sets[0];
        assert_eq!(headers.columns.len(), headers.column_types.len());
        assert_eq!(headers.column_types, first.data.columns.iter().map(|c| c.declared_type.clone()).collect::<Vec<_>>());
        let label = first.data.columns.iter().position(|c| c.name == "label").unwrap();
        assert_eq!(headers.column_types[label], if engine == Engine::Postgres { "character varying(100)" } else { "varchar(100)" });
        if engine == Engine::MySql {
            assert_eq!(first.data.columns[label].sql_type, "CHAR CHARACTER SET utf8mb4");
            assert_ne!(first.data.columns[label].sql_type, first.data.columns[label].declared_type);
        }
        let raw = session.query(&format!("SELECT id, label FROM {name} WHERE id=1")).await.unwrap();
        let raw_set = &raw.sets[0];
        assert_eq!(raw_set.columns, ["id", "label"]);
        assert_eq!(raw_set.column_types.len(), raw_set.columns.len());
        assert!(raw_set.column_types.iter().all(|ty| !ty.is_empty()));
        assert!(raw_set.column_types[0].to_ascii_lowercase().contains("int"));
        options.offset=2;
        let second = session.table_page(&table,&options).await.unwrap();
        assert!(second.has_more);
        assert!(first.data.keys.iter().all(|k| !second.data.keys.contains(k)));
        options.offset=4;
        let last = session.table_page(&table,&options).await.unwrap(); assert!(!last.has_more); assert_eq!(last.data.keys.len(),1);
        options.offset=100;
        let empty = session.table_page(&table,&options).await.unwrap(); assert!(!empty.has_more); assert_eq!(empty.data.output.sets[0].columns.len(),4); assert!(empty.data.keys.is_empty());
        assert_eq!(empty.data.output.sets[0].column_types, headers.column_types);
        options.offset=0;
        options.filters=vec![ColumnFilter { column: "label".into(), operator: FilterOperator::Contains, value: Some("%_".into()) }];
        let filtered=session.table_page(&table,&options).await.unwrap(); assert_eq!(filtered.data.keys,vec![vec![Some("3".into())]]); assert!(!filtered.sql.contains("%_"));
        for (operator,value,expected) in [(FilterOperator::Eq,Some("quote'; --"),vec!["5"]),(FilterOperator::IsNull,None,vec!["4"])] {
            options.filters=vec![ColumnFilter { column:"label".into(),operator,value:value.map(str::to_owned) }];
            let page=session.table_page(&table,&options).await.unwrap(); assert_eq!(page.data.keys,expected.iter().map(|v| vec![Some((*v).to_owned())]).collect::<Vec<_>>());
        }
        options.filters=vec![ColumnFilter { column:"score".into(),operator:FilterOperator::Gt,value:Some("3".into()) }];
        assert_eq!(session.table_page(&table,&options).await.unwrap().data.keys.len(),2);
        options.filters[0].column="score); DROP TABLE users;--".into(); assert!(session.table_page(&table,&options).await.is_err()); session.ping().await.unwrap();
        let structure=session.table_structure(&table).await.unwrap();
        assert_eq!(structure.columns.rows.len(),4); assert!(!structure.indexes.rows.is_empty()); assert_eq!(structure.foreign_keys.rows.len(),1);
        assert!(structure.ddl.contains(&table.name)); assert!(structure.ddl.contains("CREATE TABLE"));
        if engine!=Engine::MySql { assert!(structure.ddl.contains("NOT a lossless")); }
        let error=session.query("SELECT * FROM sqlshop_deliberately_missing_table_892374").await.unwrap_err(); assert!(query_error_preserves_session(&error)); session.ping().await.unwrap();
        let too_many=(0..34).map(|_| "SELECT 1;").collect::<String>();
        let error=session.query(&too_many).await.unwrap_err(); assert!(!query_error_preserves_session(&error));
        // Undrained-limit sessions are discarded; cleanup gets a new connection.
        drop(session);
        let mut cleanup=Session::connect(&profile,&credentials).await.unwrap(); cleanup.select_database("sqlshop_test").await.unwrap();
        cleanup.query(&format!("DROP TABLE {name}")).await.unwrap();
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
#[test]
fn client_and_transport_errors_do_not_preserve_sessions() {
    assert!(!query_error_preserves_session(&anyhow::anyhow!("unknown")));
    assert!(!query_error_preserves_session(
        &sqlx::Error::Protocol("bad packet".into()).into()
    ));
    assert!(!query_error_preserves_session(
        &sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset"
        ))
        .into()
    ));
}
