//! Exercise SQLx's real PostgreSQL wire client against an in-process protocol
//! fixture. These tests cover reconnect/selection, not PostgreSQL SQL semantics.
use anyhow::Result;
use sqlshop::{
    db::Session,
    model::{Credentials, Engine, Profile},
    worker::{Request, Response, Worker},
};
use std::{
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

async fn rows(
    socket: &mut TcpStream,
    columns: &[&str],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
    let mut description = (columns.len() as i16).to_be_bytes().to_vec();
    for column in columns {
        description.extend(column.as_bytes());
        description.push(0);
        description.extend(0u32.to_be_bytes());
        description.extend(0i16.to_be_bytes());
        description.extend(25u32.to_be_bytes()); // builtin TEXT, text transfer format
        description.extend((-1i16).to_be_bytes());
        description.extend((-1i32).to_be_bytes());
        description.extend(0i16.to_be_bytes());
    }
    packet(socket, b'T', &description).await?;
    for row in rows {
        let mut data = (row.len() as i16).to_be_bytes().to_vec();
        for cell in row {
            data.extend((cell.len() as i32).to_be_bytes());
            data.extend(cell.as_bytes());
        }
        packet(socket, b'D', &data).await?;
    }
    packet(socket, b'C', format!("SELECT {}\0", rows.len()).as_bytes()).await?;
    packet(socket, b'Z', b"I").await
}

async fn serve(
    mut socket: TcpStream,
    allowed: Arc<Vec<String>>,
    attempts: Arc<Mutex<Vec<String>>>,
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
    while let Ok(tag) = socket.read_u8().await {
        let length = socket.read_u32().await? as usize;
        let mut body = vec![0; length - 4];
        socket.read_exact(&mut body).await?;
        if tag == b'X' {
            break;
        }
        anyhow::ensure!(tag == b'Q', "Unexpected protocol tag {tag}");
        let sql = String::from_utf8_lossy(&body);
        if sql.contains("FROM pg_database") {
            let result: Vec<_> = allowed.iter().map(|db| vec![db.clone()]).collect();
            rows(&mut socket, &["datname"], &result).await?;
        } else if sql.contains("current_database()") {
            rows(
                &mut socket,
                &["current_database"],
                &[vec![database.clone()]],
            )
            .await?;
        } else if sql.contains("information_schema.tables") {
            rows(
                &mut socket,
                &["table_schema", "table_name", "table_type"],
                &[vec![
                    "public".into(),
                    format!("{database}_table"),
                    "BASE TABLE".into(),
                ]],
            )
            .await?;
        } else {
            rows(&mut socket, &["connection_test"], &[vec!["1".into()]]).await?;
        }
    }
    Ok(())
}

struct Fixture {
    address: SocketAddr,
    attempts: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn start(databases: &[&str]) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let allowed: Arc<Vec<String>> =
            Arc::new(databases.iter().map(|name| name.to_string()).collect());
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let records = attempts.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let allowed = allowed.clone();
                let attempts = records.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, allowed, attempts).await;
                });
            }
        });
        Ok(Self {
            address,
            attempts,
            task,
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
async fn worker_keeps_server_connection_after_failed_database_selection() -> Result<()> {
    let fixture = Fixture::start(&["postgres", "app"]).await?;
    let worker = Worker::start(|| {});
    let receive = || worker.receiver.recv_timeout(Duration::from_secs(5));
    worker.send(Request::Connect(fixture.profile(), Credentials::default()))?;
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
    worker.send(Request::Disconnect)?;
    assert!(matches!(receive()?, Response::Disconnected));
    Ok(())
}
