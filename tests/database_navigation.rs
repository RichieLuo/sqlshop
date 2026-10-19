//! Exercise SQLx's real PostgreSQL wire client against an in-process protocol
//! fixture. These tests cover reconnect/selection, not PostgreSQL SQL semantics.
use anyhow::Result;
use sqlshop::{
    db::Session,
    model::{Credentials, Engine, Profile},
    worker::{QueryRequest, QueryResponse, QueryWorker, Request, Response, Worker},
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

async fn packet(socket: &mut TcpStream, tag: u8, data: &[u8]) -> std::io::Result<()> {
    socket.write_u8(tag).await?;
    socket.write_u32((data.len() + 4) as u32).await?;
    socket.write_all(data).await
}

async fn row_description(
    socket: &mut TcpStream,
    columns: &[&str],
    formats: &[i16],
) -> std::io::Result<()> {
    let mut description = (columns.len() as i16).to_be_bytes().to_vec();
    for (index, column) in columns.iter().enumerate() {
        description.extend(column.as_bytes());
        description.push(0);
        description.extend(0u32.to_be_bytes());
        description.extend(0i16.to_be_bytes());
        description.extend(25u32.to_be_bytes()); // builtin TEXT; binary TEXT also uses UTF-8
        description.extend((-1i16).to_be_bytes());
        description.extend((-1i32).to_be_bytes());
        description.extend(
            formats
                .get(if formats.len() == 1 { 0 } else { index })
                .copied()
                .unwrap_or(0)
                .to_be_bytes(),
        );
    }
    packet(socket, b'T', &description).await
}

async fn rows(
    socket: &mut TcpStream,
    columns: &[&str],
    rows: &[Vec<String>],
    simple: bool,
) -> std::io::Result<()> {
    if simple {
        row_description(socket, columns, &[]).await?;
    }
    for row in rows {
        let mut data = (row.len() as i16).to_be_bytes().to_vec();
        for cell in row {
            data.extend((cell.len() as i32).to_be_bytes());
            data.extend(cell.as_bytes());
        }
        packet(socket, b'D', &data).await?;
    }
    packet(socket, b'C', format!("SELECT {}\0", rows.len()).as_bytes()).await?;
    // Extended queries become ready only at Sync, after CloseComplete, etc.
    if simple {
        packet(socket, b'Z', b"I").await?;
    }
    Ok(())
}

fn cstring(body: &mut &[u8]) -> Result<String> {
    let end = body
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| anyhow::anyhow!("Unterminated protocol string"))?;
    let value = std::str::from_utf8(&body[..end])?.to_owned();
    *body = &body[end + 1..];
    Ok(value)
}

fn take<'a>(body: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    anyhow::ensure!(length <= body.len(), "Truncated protocol message");
    let (value, rest) = body.split_at(length);
    *body = rest;
    Ok(value)
}

fn result_rows(
    sql: &str,
    database: &str,
    allowed: &[String],
) -> (&'static [&'static str], Vec<Vec<String>>) {
    if sql.contains("sqlshop_slow_query") {
        (&["origin"], vec![vec![database.into()]])
    } else if sql.contains("FROM pg_database") {
        (
            &["datname"],
            allowed.iter().map(|db| vec![db.clone()]).collect(),
        )
    } else if sql.contains("current_database()") {
        (&["current_database"], vec![vec![database.into()]])
    } else if sql.contains("current_schema()") {
        (&["current_schema"], vec![vec!["public".into()]])
    } else if sql.contains("information_schema.tables") {
        (
            &["table_schema", "table_name", "table_type"],
            vec![vec![
                "public".into(),
                format!("{database}_table"),
                "BASE TABLE".into(),
            ]],
        )
    } else {
        (&["connection_test"], vec![vec!["1".into()]])
    }
}

async fn serve(
    mut socket: TcpStream,
    allowed: Arc<Vec<String>>,
    attempts: Arc<Mutex<Vec<String>>>,
    slow_started: Arc<tokio::sync::Notify>,
    slow_release: Arc<tokio::sync::Notify>,
) -> Result<()> {
    let length = socket.read_u32().await? as usize;
    anyhow::ensure!((8..65_536).contains(&length), "Invalid startup packet");
    let mut startup = vec![0; length - 4];
    socket.read_exact(&mut startup).await?;
    let fields: Vec<_> = startup[4..].split(|b| *b == 0).collect();
    let database = fields
        .chunks(2)
        .find_map(|pair| {
            if pair.first() == Some(&b"database".as_slice()) {
                pair.get(1)
                    .map(|value| String::from_utf8_lossy(value).to_string())
            } else {
                None
            }
        })
        .unwrap();
    attempts.lock().unwrap().push(database.clone());
    if !allowed.contains(&database) {
        packet(
            &mut socket,
            b'E',
            b"SFATAL\0C3D000\0Mdatabase does not exist\0\0",
        )
        .await?;
        return Ok(());
    }
    packet(&mut socket, b'R', &0u32.to_be_bytes()).await?;
    packet(&mut socket, b'S', b"server_version\x0017.0\x00").await?;
    packet(&mut socket, b'S', b"client_encoding\0UTF8\0").await?;
    packet(&mut socket, b'K', &[0; 8]).await?;
    packet(&mut socket, b'Z', b"I").await?;
    let mut statements = HashMap::<String, (String, Vec<u32>)>::new();
    let mut portals = HashMap::<String, (String, Vec<i16>)>::new();
    while let Ok(tag) = socket.read_u8().await {
        let length = socket.read_u32().await? as usize;
        anyhow::ensure!(
            (4..65_536).contains(&length),
            "Invalid frontend message length"
        );
        let mut payload = vec![0; length - 4];
        socket.read_exact(&mut payload).await?;
        let mut body = payload.as_slice();
        let (sql, simple) = match tag {
            b'X' => break,
            b'Q' => {
                let sql = cstring(&mut body)?;
                portals.remove("");
                (sql, true)
            }
            b'P' => {
                let name = cstring(&mut body)?;
                let sql = cstring(&mut body)?;
                let count = u16::from_be_bytes(take(&mut body, 2)?.try_into()?) as usize;
                let mut types = Vec::with_capacity(count);
                for _ in 0..count {
                    types.push(u32::from_be_bytes(take(&mut body, 4)?.try_into()?));
                }
                anyhow::ensure!(body.is_empty(), "Trailing Parse data");
                statements.insert(name, (sql, types));
                packet(&mut socket, b'1', &[]).await?; // ParseComplete
                continue;
            }
            b'D' => {
                let kind = take(&mut body, 1)?[0];
                let name = cstring(&mut body)?;
                anyhow::ensure!(body.is_empty(), "Trailing Describe data");
                match kind {
                    b'S' => {
                        let (sql, types) = statements
                            .get(&name)
                            .ok_or_else(|| anyhow::anyhow!("Unknown statement"))?;
                        let mut parameters = (types.len() as u16).to_be_bytes().to_vec();
                        for oid in types {
                            parameters.extend(oid.to_be_bytes());
                        }
                        packet(&mut socket, b't', &parameters).await?; // ParameterDescription
                        let (columns, _) = result_rows(sql, &database, &allowed);
                        row_description(&mut socket, columns, &[]).await?;
                    }
                    b'P' => {
                        let (sql, formats) = portals
                            .get(&name)
                            .ok_or_else(|| anyhow::anyhow!("Unknown portal"))?;
                        let (columns, _) = result_rows(sql, &database, &allowed);
                        row_description(&mut socket, columns, formats).await?;
                    }
                    _ => anyhow::bail!("Invalid Describe kind"),
                }
                continue;
            }
            b'B' => {
                let portal = cstring(&mut body)?;
                let name = cstring(&mut body)?;
                let (sql, types) = statements
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("Unknown statement"))?;
                let count = u16::from_be_bytes(take(&mut body, 2)?.try_into()?) as usize;
                let _parameter_formats = take(&mut body, count * 2)?;
                let count = u16::from_be_bytes(take(&mut body, 2)?.try_into()?) as usize;
                anyhow::ensure!(count == types.len(), "Bind parameter count mismatch");
                for _ in 0..count {
                    let length = i32::from_be_bytes(take(&mut body, 4)?.try_into()?);
                    anyhow::ensure!(length >= -1, "Invalid parameter length");
                    if length >= 0 {
                        take(&mut body, length as usize)?;
                    }
                }
                let count = u16::from_be_bytes(take(&mut body, 2)?.try_into()?) as usize;
                let mut formats = Vec::with_capacity(count);
                for _ in 0..count {
                    let format = i16::from_be_bytes(take(&mut body, 2)?.try_into()?);
                    anyhow::ensure!(matches!(format, 0 | 1), "Invalid result format");
                    formats.push(format);
                }
                anyhow::ensure!(body.is_empty(), "Trailing Bind data");
                portals.insert(portal, (sql.clone(), formats));
                packet(&mut socket, b'2', &[]).await?; // BindComplete
                continue;
            }
            b'E' => {
                let portal = cstring(&mut body)?;
                let max_rows = u32::from_be_bytes(take(&mut body, 4)?.try_into()?);
                anyhow::ensure!(
                    body.is_empty() && max_rows == 0,
                    "Fixture expects unlimited Execute"
                );
                let (sql, _) = portals
                    .get(&portal)
                    .ok_or_else(|| anyhow::anyhow!("Unknown portal"))?;
                (sql.clone(), false)
            }
            b'C' => {
                let kind = take(&mut body, 1)?[0];
                let name = cstring(&mut body)?;
                anyhow::ensure!(body.is_empty(), "Trailing Close data");
                match kind {
                    b'S' => {
                        statements.remove(&name);
                    }
                    b'P' => {
                        portals.remove(&name);
                    }
                    _ => anyhow::bail!("Invalid Close kind"),
                }
                packet(&mut socket, b'3', &[]).await?; // CloseComplete
                continue;
            }
            b'S' => {
                anyhow::ensure!(body.is_empty(), "Sync data must be empty");
                packet(&mut socket, b'Z', b"I").await?;
                continue;
            }
            b'H' => {
                anyhow::ensure!(body.is_empty(), "Flush data must be empty");
                continue;
            }
            _ => anyhow::bail!("Unexpected protocol tag {tag}"),
        };
        if sql.contains("sqlshop_drop_transport") {
            return Ok(());
        }
        if sql.contains("sqlshop_slow_query") {
            slow_started.notify_one();
            slow_release.notified().await;
        }
        let (columns, result) = result_rows(&sql, &database, &allowed);
        rows(&mut socket, columns, &result, simple).await?;
    }
    Ok(())
}

struct Fixture {
    address: SocketAddr,
    attempts: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
    slow_started: Arc<tokio::sync::Notify>,
    slow_release: Arc<tokio::sync::Notify>,
}

impl Fixture {
    async fn start(databases: &[&str]) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let allowed: Arc<Vec<String>> =
            Arc::new(databases.iter().map(|name| name.to_string()).collect());
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let records = attempts.clone();
        let slow_started = Arc::new(tokio::sync::Notify::new());
        let slow_release = Arc::new(tokio::sync::Notify::new());
        let started = slow_started.clone();
        let release = slow_release.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let allowed = allowed.clone();
                let attempts = records.clone();
                let started = started.clone();
                let release = release.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, allowed, attempts, started, release).await;
                });
            }
        });
        Ok(Self {
            address,
            attempts,
            task,
            slow_started,
            slow_release,
        })
    }

    fn profile(&self) -> Profile {
        Profile {
            engine: Engine::Postgres,
            username: "test_user".into(),
            host: "127.0.0.1".into(),
            port: self.address.port(),
            tls: false,
            remember_password: false,
            ..Default::default()
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_transport_failure_discards_only_its_session_without_retry() -> Result<()> {
    let fixture = Fixture::start(&["postgres", "app"]).await?;
    let profile = fixture.profile();
    let tab = uuid::Uuid::new_v4();
    let mut target = sqlshop::workspace::QueryTarget {
        schema: None,
        connection: profile.id,
        database: Some("app".into()),
    };
    let worker = QueryWorker::start(tab, || {});
    // Execution/transaction observations precede the terminal result. They do
    // not represent a second execution and must not replace the transport check.
    let receive = || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let event = worker
                .receiver
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))?;
            if !matches!(
                event,
                QueryResponse::ExecutionStarted { .. }
                    | QueryResponse::CancelStatus { .. }
                    | QueryResponse::TransactionState { .. }
            ) {
                return Ok::<_, std::sync::mpsc::RecvTimeoutError>(event);
            }
        }
    };
    let send = |sql: &str, target: &sqlshop::workspace::QueryTarget| {
        worker.send(QueryRequest::Run {
            profile: profile.clone(),
            credentials: Credentials::default(),
            target: target.clone(),
            sql: sql.into(),
        })
    };
    send("SELECT 1", &target)?;
    match receive()? {
        QueryResponse::Finished {
            tab: origin,
            database: Some(db),
            schema,
            ..
        } => {
            assert_eq!(origin, tab);
            assert_eq!(db, "app");
            assert_eq!(schema.as_deref(), Some("public"));
            target.schema = schema;
        }
        QueryResponse::Error { message, .. } => {
            panic!("Expected first query completion: {message}")
        }
        _ => panic!("Expected first query completion"),
    }
    let before = fixture.attempts.lock().unwrap().len();
    send("SELECT sqlshop_drop_transport", &target)?;
    assert!(
        matches!(receive()?, QueryResponse::Error { tab: origin, connected: false, database: None, .. } if origin == tab)
    );
    assert_eq!(fixture.attempts.lock().unwrap().len(), before);
    assert!(worker.receiver.try_recv().is_err());
    // Only a new, explicitly submitted operation establishes a fresh session.
    // The disconnected actor has no observed schema; rediscover its default.
    target.schema = None;
    send("SELECT 1", &target)?;
    assert!(
        matches!(receive()?, QueryResponse::Finished { tab: origin, database: Some(db), .. } if origin == tab && db == "app")
    );
    assert_eq!(fixture.attempts.lock().unwrap().len(), before + 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_discovers_and_selects_databases_without_a_configured_database() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let fixture = Fixture::start(&["postgres", "app_b", "app_a"]).await?;
        let mut session = Session::connect(&fixture.profile(), &Credentials::default()).await?;
        assert_eq!(session.databases().await?, ["app_a", "app_b", "postgres"]);
        session.select_database("app_a").await?;
        assert_eq!(session.current_database().await?.as_deref(), Some("app_a"));
        assert_eq!(session.tables().await?[0].name, "app_a_table");
        let error = session
            .select_database("missing_database")
            .await
            .unwrap_err();
        assert!(session.selection_error_preserves_session(&error));
        assert_eq!(session.current_database().await?.as_deref(), Some("app_a"));
        session.select_database("app_b").await?;
        assert_eq!(session.tables().await?[0].name, "app_b_table");
        assert_eq!(
            *fixture.attempts.lock().unwrap(),
            ["postgres", "app_a", "missing_database", "app_b"]
        );
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_bootstrap_falls_back_to_the_user_database() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let fixture = Fixture::start(&["test_user", "app"]).await?;
        let mut session = Session::connect(&fixture.profile(), &Credentials::default()).await?;
        assert_eq!(
            session.current_database().await?.as_deref(),
            Some("test_user")
        );
        assert_eq!(*fixture.attempts.lock().unwrap(), ["postgres", "test_user"]);
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_workers_run_independently_and_preserve_targets() -> Result<()> {
    use sqlshop::workspace::QueryTarget;
    use uuid::Uuid;
    let fixture = Fixture::start(&["postgres", "app_a", "app_b"]).await?;
    let a = Worker::start(|| {});
    let b = Worker::start(|| {});
    let receive = |worker: &Worker| worker.receiver.recv_timeout(Duration::from_secs(5));
    let profile_a = fixture.profile();
    let profile_b = fixture.profile();
    let target_a = QueryTarget {
        schema: None,
        connection: profile_a.id,
        database: Some("app_a".into()),
    };
    let target_b = QueryTarget {
        schema: None,
        connection: profile_b.id,
        database: Some("app_b".into()),
    };
    for (worker, profile, database) in [(&a, profile_a.clone(), "app_a"), (&b, profile_b, "app_b")]
    {
        worker.send(Request::Connect(profile, Credentials::default()))?;
        assert!(matches!(receive(worker)?, Response::Connected(_)));
        worker.send(Request::SelectDatabase(database.into()))?;
        assert!(matches!(receive(worker)?, Response::DatabaseSelected(db) if db == database));
    }
    let tab_a = Uuid::new_v4();
    let tab_b = Uuid::new_v4();
    a.send(Request::RunQuery {
        tab: tab_a,
        target: target_a.clone(),
        sql: "SELECT sqlshop_slow_query".into(),
    })?;
    tokio::time::timeout(Duration::from_secs(5), fixture.slow_started.notified()).await?;
    // A is held behind a protocol-fixture barrier, not a timing-dependent sleep.
    b.send(Request::RunQuery {
        tab: tab_b,
        target: target_b.clone(),
        sql: "SELECT 1".into(),
    })?;
    assert!(
        matches!(receive(&b)?, Response::QueryResult { tab, database: Some(db), .. } if tab == tab_b && db == "app_b")
    );
    assert!(matches!(
        a.receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    // Cross-connection targets are rejected even though both workers are live.
    b.send(Request::RunQuery {
        tab: tab_b,
        target: target_a.clone(),
        sql: "SELECT 1".into(),
    })?;
    assert!(
        matches!(receive(&b)?, Response::OperationError { tab, disconnected: false, .. } if tab == tab_b)
    );
    // Testing credentials uses a third, isolated worker, even for an existing profile.
    let test = Worker::start(|| {});
    test.send(Request::Test(profile_a, Credentials::default()))?;
    assert!(matches!(receive(&test)?, Response::Tested));
    b.send(Request::Tables)?;
    assert!(matches!(receive(&b)?, Response::Tables(tables) if tables[0].name == "app_b_table"));
    b.send(Request::Disconnect)?;
    assert!(matches!(receive(&b)?, Response::Disconnected));
    fixture.slow_release.notify_one();
    match receive(&a)? {
        Response::QueryResult {
            tab,
            database,
            output,
        } => {
            assert_eq!(tab, tab_a);
            assert_eq!(database, Some("app_a".into()));
            assert_eq!(output.sets[0].rows[0][0].as_deref(), Some("app_a"));
        }
        _ => panic!("A's result was lost or routed to another operation"),
    }
    a.send(Request::Tables)?;
    assert!(matches!(receive(&a)?, Response::Tables(tables) if tables[0].name == "app_a_table"));
    a.send(Request::RunQuery {
        tab: tab_a,
        target: target_a,
        sql: "SELECT 1".into(),
    })?;
    assert!(matches!(receive(&a)?, Response::QueryResult { tab, .. } if tab == tab_a));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_keeps_server_connection_after_failed_database_selection() -> Result<()> {
    let fixture = Fixture::start(&["postgres", "app"]).await?;
    let worker = Worker::start(|| {});
    let receive = || worker.receiver.recv_timeout(Duration::from_secs(5));
    let profile = fixture.profile();
    let connection = profile.id;
    worker.send(Request::Connect(profile, Credentials::default()))?;
    assert!(matches!(receive()?, Response::Connected(_)));
    worker.send(Request::Databases)?;
    assert!(matches!(receive()?, Response::Databases(_)));
    worker.send(Request::SelectDatabase("app".into()))?;
    assert!(matches!(receive()?, Response::DatabaseSelected(db) if db == "app"));
    worker.send(Request::SelectDatabase("missing".into()))?;
    assert!(matches!(
        receive()?,
        Response::Error {
            disconnected: false,
            ..
        }
    ));
    worker.send(Request::Query("SELECT 1".into()))?;
    assert!(matches!(receive()?, Response::Query { database: Some(db), .. } if db == "app"));
    let tab = uuid::Uuid::new_v4();
    for target in [
        sqlshop::workspace::QueryTarget {
            schema: None,
            connection: uuid::Uuid::new_v4(),
            database: Some("app".into()),
        },
        sqlshop::workspace::QueryTarget {
            schema: None,
            connection,
            database: Some("other".into()),
        },
        sqlshop::workspace::QueryTarget {
            schema: None,
            connection,
            database: None,
        },
    ] {
        worker.send(Request::RunQuery {
            tab,
            target,
            sql: "SELECT 1".into(),
        })?;
        assert!(
            matches!(receive()?, Response::OperationError { tab: response_tab, disconnected: false, .. } if response_tab == tab)
        );
    }
    worker.send(Request::RunQuery {
        tab,
        target: sqlshop::workspace::QueryTarget {
            schema: None,
            connection,
            database: Some("app".into()),
        },
        sql: "SELECT 1".into(),
    })?;
    assert!(
        matches!(receive()?, Response::QueryResult { tab: response_tab, database: Some(db), .. } if response_tab == tab && db == "app")
    );
    worker.send(Request::Disconnect)?;
    assert!(matches!(receive()?, Response::Disconnected));
    Ok(())
}
