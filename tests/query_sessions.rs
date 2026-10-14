//! Opt-in real driver checks for the tab-owned query connection protocol.
use sqlshop::{
    db::Session,
    model::{Credentials, Engine, Profile, QueryOutput},
    worker::{QueryRequest, QueryResponse, QueryWorker},
    workspace::QueryTarget,
};
use std::time::Duration;
use uuid::Uuid;

#[path = "support/ssh_gateway.rs"]
mod ssh_gateway;

fn config(engine: Engine, prefix: &str) -> (Profile, Credentials) {
    let get = |key: &str| {
        std::env::var(format!("SQLSHOP_TEST_{prefix}_{key}"))
            .unwrap_or_else(|_| panic!("Missing SQLSHOP_TEST_{prefix}_{key}"))
    };
    (
        Profile {
            engine,
            host: get("HOST"),
            port: get("PORT").parse().unwrap(),
            username: get("USER"),
            database: get("DATABASE"),
            tls: false,
            trust_certificate: false,
            remember_password: false,
            ..Profile::default()
        },
        Credentials {
            database_password: get("PASSWORD"),
            ..Default::default()
        },
    )
}

fn send(
    worker: &QueryWorker,
    profile: &Profile,
    credentials: &Credentials,
    target: &QueryTarget,
    sql: &str,
) {
    worker
        .send(QueryRequest::Run {
            profile: profile.clone(),
            credentials: credentials.clone(),
            target: target.clone(),
            sql: sql.into(),
        })
        .unwrap();
}

fn receive(worker: &QueryWorker) -> QueryResponse {
    loop {
        let response = worker
            .receiver
            .recv_timeout(Duration::from_secs(20))
            .unwrap();
        if !matches!(
            response,
            QueryResponse::ExecutionStarted { .. }
                | QueryResponse::CancelStatus { .. }
                | QueryResponse::TransactionState { .. }
        ) {
            return response;
        }
    }
}

fn finished(response: QueryResponse, tab: Uuid, target: &QueryTarget) -> QueryOutput {
    match response {
        QueryResponse::Finished {
            tab: origin,
            output,
            database,
            ..
        } => {
            assert_eq!(origin, tab);
            assert_eq!(database, target.database);
            output
        }
        QueryResponse::Error { message, .. } => panic!("Unexpected query error: {message}"),
        QueryResponse::Disconnected { .. } => panic!("Unexpected disconnect"),
        QueryResponse::SchemasLoaded { .. }
        | QueryResponse::SchemaLoadError { .. }
        | QueryResponse::ExecutionStarted { .. }
        | QueryResponse::CancelStatus { .. }
        | QueryResponse::TransactionState { .. }
        | QueryResponse::ControlFinished { .. }
        | QueryResponse::ContextChecked { .. } => {
            panic!("Unexpected schema response")
        }
    }
}

fn retained_error(response: QueryResponse, tab: Uuid, target: &QueryTarget) {
    match response {
        QueryResponse::Error {
            tab: origin,
            connected,
            database,
            ..
        } => {
            assert_eq!(origin, tab);
            assert!(
                connected,
                "An ordinary statement error must preserve the session"
            );
            assert_eq!(database, target.database);
        }
        _ => panic!("Expected a retained query session error"),
    }
}

fn exercise(engine: Engine, prefix: &str) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for through_ssh in [false, true] {
            let (mut profile, mut credentials) = config(engine, prefix);
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
            let a_target = QueryTarget {
                schema: None,
                connection: profile.id,
                database: Some(profile.database.clone()),
            };
            let b_target = QueryTarget {
                schema: None,
                connection: profile.id,
                database: Some(std::env::var("SQLSHOP_TEST_SECOND_DATABASE").unwrap()),
            };
            let a_tab = Uuid::new_v4();
            let b_tab = Uuid::new_v4();
            let a = QueryWorker::start(a_tab, || {});
            let b = QueryWorker::start(b_tab, || {});
            let mut management = Session::connect(&profile, &credentials).await.unwrap();
            management.select_database(&profile.database).await.unwrap();

            let (create, table, begin, rollback, slow) = match engine {
                Engine::MySql => (
                    "CREATE TEMPORARY TABLE sqlshop_query_probe (marker VARCHAR(20))",
                    "sqlshop_query_probe",
                    "START TRANSACTION",
                    "ROLLBACK",
                    "SELECT SLEEP(3)",
                ),
                Engine::Postgres => (
                    "CREATE TEMP TABLE sqlshop_query_probe (marker VARCHAR(20))",
                    "sqlshop_query_probe",
                    "BEGIN",
                    "ROLLBACK",
                    "SELECT pg_sleep(3)",
                ),
                Engine::SqlServer => (
                    "CREATE TABLE #sqlshop_query_probe (marker VARCHAR(20))",
                    "#sqlshop_query_probe",
                    "BEGIN TRANSACTION",
                    "ROLLBACK TRANSACTION",
                    "WAITFOR DELAY '00:00:03'; SELECT 1 AS completed",
                ),
            };
            for (worker, tab, target, marker) in
                [(&a, a_tab, &a_target, "a"), (&b, b_tab, &b_target, "b")]
            {
                send(
                    worker,
                    &profile,
                    &credentials,
                    target,
                    &format!("{create}; INSERT INTO {table} VALUES ('{marker}')"),
                );
                finished(receive(worker), tab, target);
            }
            // Sidebar selection affects only the management connection.
            management
                .select_database(b_target.database.as_deref().unwrap())
                .await
                .unwrap();
            send(
                &a,
                &profile,
                &credentials,
                &a_target,
                &format!("{begin}; UPDATE {table} SET marker = 'transaction'"),
            );
            finished(receive(&a), a_tab, &a_target);
            send(
                &a,
                &profile,
                &credentials,
                &a_target,
                &format!("SELECT marker FROM {table}"),
            );
            assert_eq!(
                finished(receive(&a), a_tab, &a_target).sets[0].rows[0][0].as_deref(),
                Some("transaction")
            );
            send(
                &b,
                &profile,
                &credentials,
                &b_target,
                &format!("SELECT marker FROM {table}"),
            );
            assert_eq!(
                finished(receive(&b), b_tab, &b_target).sets[0].rows[0][0].as_deref(),
                Some("b")
            );
            assert!(!management.raw_session_requires_confirmation());
            assert_eq!(
                management.current_database().await.unwrap(),
                b_target.database
            );

            // Misrouting or edited profiles must not discard an open transaction.
            send(&a, &profile, &credentials, &b_target, "SELECT 1");
            retained_error(receive(&a), a_tab, &a_target);
            let mut edited = profile.clone();
            edited.port = edited.port.saturating_add(1);
            send(&a, &edited, &credentials, &a_target, "SELECT 1");
            retained_error(receive(&a), a_tab, &a_target);
            send(&a, &profile, &credentials, &a_target, "  ");
            retained_error(receive(&a), a_tab, &a_target);
            send(
                &a,
                &profile,
                &credentials,
                &a_target,
                "SELECT * FROM sqlshop_nonexistent_query_relation",
            );
            retained_error(receive(&a), a_tab, &a_target);
            // PostgreSQL has an aborted transaction here: context probes must not
            // prevent the rollback from reaching the same physical connection.
            send(&a, &profile, &credentials, &a_target, rollback);
            finished(receive(&a), a_tab, &a_target);
            send(
                &a,
                &profile,
                &credentials,
                &a_target,
                &format!("SELECT marker FROM {table}"),
            );
            assert_eq!(
                finished(receive(&a), a_tab, &a_target).sets[0].rows[0][0].as_deref(),
                Some("a")
            );

            // A pending query on A cannot serialize B behind the same server.
            send(&a, &profile, &credentials, &a_target, slow);
            send(&b, &profile, &credentials, &b_target, "SELECT 42 AS answer");
            let b_response = b.receiver.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(
                finished(b_response, b_tab, &b_target).sets[0].rows[0][0].as_deref(),
                Some("42")
            );
            finished(receive(&a), a_tab, &a_target);

            // USE changes only its source tab, including when a later statement
            // in the batch fails. PostgreSQL has no USE statement.
            if let Some(change_database) =
                engine.select_database_sql(b_target.database.as_deref().unwrap())
            {
                send(
                    &a,
                    &profile,
                    &credentials,
                    &a_target,
                    &format!("{change_database} SELECT * FROM sqlshop_nonexistent_query_relation"),
                );
                retained_error(receive(&a), a_tab, &b_target);
                send(&a, &profile, &credentials, &a_target, "SELECT 1");
                retained_error(receive(&a), a_tab, &b_target);
                send(&a, &profile, &credentials, &b_target, "SELECT 42 AS answer");
                finished(receive(&a), a_tab, &b_target);
                assert_eq!(
                    management.current_database().await.unwrap(),
                    b_target.database
                );
            }
            a.send(QueryRequest::Disconnect).unwrap();
            assert!(matches!(receive(&a), QueryResponse::Disconnected { tab } if tab == a_tab));
            send(&a, &profile, &credentials, &b_target, "SELECT 42 AS answer");
            finished(receive(&a), a_tab, &b_target);
            drop(a);
            drop(b);
            drop(management);
            drop(gateway);
        }
    });
}

#[test]
#[ignore = "Requires the isolated MySQL test database"]
fn mysql() {
    exercise(Engine::MySql, "MYSQL");
}

#[test]
#[ignore = "Requires the isolated PostgreSQL test database"]
fn postgres() {
    exercise(Engine::Postgres, "PG");
}

#[test]
#[ignore = "Requires the isolated SQL Server test database"]
fn sqlserver() {
    exercise(Engine::SqlServer, "MSSQL");
}
