//! Opt-in, read-only integration tests. Credentials come exclusively from environment variables.
use sqlshop::{
    db::{ROW_LIMIT, Session, connection_error},
    model::{Credentials, Engine, Profile},
};

#[path = "support/ssh_gateway.rs"]
mod ssh_gateway;

fn profile(engine: Engine, prefix: &str) -> (Profile, String) {
    let get = |key: &str| {
        std::env::var(format!("SQLSHOP_TEST_{prefix}_{key}")).unwrap_or_else(|_| {
            panic!("Set SQLSHOP_TEST_{prefix}_{key} before running this integration test")
        })
    };
    let profile = Profile {
        engine,
        host: get("HOST"),
        port: get("PORT").parse().unwrap(),
        username: get("USER"),
        database: get("DATABASE"),
        tls: false,
        trust_certificate: false,
        remember_password: false,
        ..Profile::default()
    };
    (profile, get("PASSWORD"))
}

fn exercise(engine: Engine, prefix: &str, sql: &str, many_rows: &str) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for through_ssh in [false, true] {
            let (mut profile, password) = profile(engine, prefix);
            let mut credentials = Credentials {
                database_password: password,
                ..Default::default()
            };
            let gateway = if through_ssh {
                let gateway = ssh_gateway::Gateway::start(&profile.host, profile.port)
                    .await
                    .unwrap();
                profile.ssh = gateway.config.clone();
                credentials.ssh_secret = ssh_gateway::PASSWORD.into();
                Some(gateway)
            } else {
                None
            };
            println!(
                "Testing {} via {}",
                engine.label(),
                if through_ssh { "SSH" } else { "TCP" }
            );
            let mut session = Session::connect(&profile, &credentials).await.unwrap();
            session.ping().await.unwrap();
            let databases = session.databases().await.unwrap();
            assert!(
                databases.contains(&profile.database),
                "Expected database {} in {databases:?}",
                profile.database
            );
            session.select_database(&profile.database).await.unwrap();
            assert_eq!(
                session.current_database().await.unwrap().as_deref(),
                Some(profile.database.as_str())
            );
            session.tables().await.unwrap();
            if let Ok(other) = std::env::var("SQLSHOP_TEST_SECOND_DATABASE") {
                assert!(databases.contains(&other));
                let query = match engine {
                    Engine::MySql => "SELECT marker FROM navigation_probe",
                    Engine::Postgres => "SELECT marker FROM public.navigation_probe",
                    Engine::SqlServer => "SELECT marker FROM dbo.navigation_probe",
                };
                assert!(
                    session
                        .tables()
                        .await
                        .unwrap()
                        .iter()
                        .any(|table| table.name == "navigation_probe")
                );
                assert_eq!(
                    session.query(query).await.unwrap().sets[0].rows[0][0].as_deref(),
                    Some(profile.database.as_str())
                );
                session.select_database(&other).await.unwrap();
                assert_eq!(
                    session.current_database().await.unwrap().as_deref(),
                    Some(other.as_str())
                );
                assert!(
                    session
                        .tables()
                        .await
                        .unwrap()
                        .iter()
                        .any(|table| table.name == "navigation_probe")
                );
                assert_eq!(
                    session.query(query).await.unwrap().sets[0].rows[0][0].as_deref(),
                    Some(other.as_str())
                );
                session.select_database(&profile.database).await.unwrap();
            }
            let output = session.query(sql).await.unwrap();
            let set = &output.sets[0];
            assert_eq!(
                set.columns,
                ["number", "text", "nothing", "amount", "binary"]
            );
            assert_eq!(
                set.rows[0],
                [
                    Some("42".into()),
                    Some("你好".into()),
                    None,
                    Some("12.50".into()),
                    Some("0x00ff".into())
                ]
            );
            let multi = session
                .query("SELECT 1 AS first_result; SELECT 2 AS second_result;")
                .await
                .unwrap();
            assert_eq!(multi.sets.len(), 2);
            assert_eq!(multi.sets[0].rows[0][0].as_deref(), Some("1"));
            assert_eq!(multi.sets[1].rows[0][0].as_deref(), Some("2"));
            let limited = session.query(many_rows).await.unwrap();
            assert_eq!(limited.sets[0].rows.len(), ROW_LIMIT);
            assert!(limited.sets[0].truncated);
            // The driver must drain the stream so the same session works after truncation.
            session.ping().await.unwrap();
            let empty = session.query("SELECT 1 AS n WHERE 1 = 0").await.unwrap();
            assert!(empty.sets.iter().all(|set| set.rows.is_empty()));
            let missing = format!("sqlshop_missing_{}", uuid::Uuid::new_v4().simple());
            let error = session.select_database(&missing).await.unwrap_err();
            assert!(session.selection_error_preserves_session(&error));
            assert_eq!(
                session.current_database().await.unwrap().as_deref(),
                Some(profile.database.as_str())
            );
            drop(session);
            drop(gateway);
        }
        if std::env::var("SQLSHOP_TEST_SELF_SIGNED_TLS").as_deref() == Ok("1") {
            let (mut profile, password) = profile(engine, prefix);
            profile.tls = true;
            let mut credentials = Credentials {
                database_password: password,
                ..Default::default()
            };
            let error = match Session::connect(&profile, &credentials).await {
                Ok(_) => panic!("The self-signed test certificate must fail strict verification"),
                Err(error) => connection_error(&profile, error),
            };
            let detail = format!("{error:#}");
            assert!(
                detail.contains("UnknownIssuer") || detail.contains("UnsupportedCertVersion"),
                "{detail}"
            );
            assert!(format!("{error:#}").contains("信任服务器证书"), "{error:#}");
            profile.trust_certificate = true;
            for through_ssh in [false, true] {
                let gateway = if through_ssh {
                    let gateway = ssh_gateway::Gateway::start(&profile.host, profile.port)
                        .await
                        .unwrap();
                    profile.ssh = gateway.config.clone();
                    credentials.ssh_secret = ssh_gateway::PASSWORD.into();
                    Some(gateway)
                } else {
                    None
                };
                let mut session = Session::connect(&profile, &credentials).await.unwrap();
                session.ping().await.unwrap();
                assert!(
                    session
                        .databases()
                        .await
                        .unwrap()
                        .contains(&profile.database)
                );
                session.select_database(&profile.database).await.unwrap();
                assert_eq!(
                    session.current_database().await.unwrap().as_deref(),
                    Some(profile.database.as_str())
                );
                drop(session);
                drop(gateway);
            }
        }
    });
}

#[test]
#[ignore = "requires a MySQL database and SQLSHOP_TEST_MYSQL_* credentials"]
fn mysql() {
    exercise(
        Engine::MySql,
        "MYSQL",
        "SELECT 42 AS number, '你好' AS text, NULL AS nothing, CAST(12.50 AS DECIMAL(10,2)) AS amount, UNHEX('00ff') AS `binary`",
        "SELECT a.n FROM (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9 UNION ALL SELECT 10) a CROSS JOIN (SELECT 0 UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9 UNION ALL SELECT 10) b CROSS JOIN (SELECT 0 UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9 UNION ALL SELECT 10) c",
    );
}

#[test]
#[ignore = "requires a PostgreSQL database and SQLSHOP_TEST_PG_* credentials"]
fn postgres() {
    exercise(
        Engine::Postgres,
        "PG",
        "SELECT 42 AS number, '你好' AS text, NULL AS nothing, 12.50::numeric(10,2) AS amount, decode('00ff','hex') AS binary",
        "SELECT generate_series(1,1100) AS n",
    );
}

#[test]
#[ignore = "requires a SQL Server database and SQLSHOP_TEST_MSSQL_* credentials"]
fn sqlserver() {
    exercise(
        Engine::SqlServer,
        "MSSQL",
        "SELECT 42 AS number, N'你好' AS text, NULL AS nothing, CAST(12.50 AS DECIMAL(10,2)) AS amount, 0x00ff AS binary",
        "SELECT TOP (1100) a.object_id FROM sys.all_objects a CROSS JOIN sys.all_objects b",
    );
}
