//! Real SQLx clients against wire fixtures: target binding and job dispatch,
//! without a database server or native restore process.
use anyhow::Result;
use sqlshop::{
    backup::{self, BackupLocation, BackupOperation, DraftBackupConfig},
    jobs::{self, JobSchedule, JobSpec},
    model::{Credentials, Engine, Profile},
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
        description.extend(
            if columns == ["object_count"] {
                20u32
            } else {
                25u32
            }
            .to_be_bytes(),
        );
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
            let bytes = if columns == ["object_count"] && !simple {
                cell.parse::<i64>().unwrap().to_be_bytes().to_vec()
            } else {
                cell.as_bytes().to_vec()
            };
            data.extend((bytes.len() as i32).to_be_bytes());
            data.extend(bytes);
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
    if sql.contains("FROM pg_class") {
        (
            &["object_count"],
            vec![vec![if database == "nonempty" { "1" } else { "0" }.into()]],
        )
    } else if sql.contains("sqlshop_slow_query") {
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
    queries: Arc<Mutex<Vec<(String, String)>>>,
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
        queries
            .lock()
            .unwrap()
            .push((database.clone(), sql.clone()));
        if sql == JOB_SQL {
            packet(&mut socket, b'C', b"UPDATE 1\0").await?;
            if simple {
                packet(&mut socket, b'Z', b"I").await?;
            }
            continue;
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
    queries: Arc<Mutex<Vec<(String, String)>>>,
}

impl Fixture {
    async fn start(databases: &[&str]) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let allowed: Arc<Vec<String>> =
            Arc::new(databases.iter().map(|name| name.to_string()).collect());
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let records = attempts.clone();
        let queries = Arc::new(Mutex::new(Vec::new()));
        let recorded_queries = queries.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let allowed = allowed.clone();
                let attempts = records.clone();
                let queries = recorded_queries.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, allowed, attempts, queries).await;
                });
            }
        });
        Ok(Self {
            address,
            attempts,
            task,
            queries,
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

async fn mysql_packet(socket: &mut TcpStream, sequence: u8, payload: &[u8]) -> Result<()> {
    socket
        .write_all(&(payload.len() as u32).to_le_bytes()[..3])
        .await?;
    socket.write_u8(sequence).await?;
    socket.write_all(payload).await?;
    Ok(())
}

async fn mysql_receive(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    socket.read_exact(&mut header).await?;
    let length = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    anyhow::ensure!(length < 65_536, "Unexpected MySQL packet size");
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await?;
    Ok(body)
}

async fn mysql_database_row(socket: &mut TcpStream, database: &str) -> Result<()> {
    mysql_packet(socket, 1, &[1]).await?;
    let mut column = Vec::new();
    for field in ["def", "", "", "", "current_database", ""] {
        column.push(field.len() as u8);
        column.extend(field.as_bytes());
    }
    column.extend([12, 45, 0]); // fixed-field size, utf8mb4 collation
    column.extend(1024u32.to_le_bytes());
    column.extend([0xfd, 0, 0, 0, 0, 0]); // VAR_STRING, flags, decimals, filler
    mysql_packet(socket, 2, &column).await?;
    let eof = [0xfe, 0, 0, 2, 0];
    mysql_packet(socket, 3, &eof).await?;
    let mut row = vec![database.len() as u8];
    row.extend(database.as_bytes());
    mysql_packet(socket, 4, &row).await?;
    mysql_packet(socket, 5, &eof).await
}

struct MySqlFixture {
    address: SocketAddr,
    commands: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<Result<()>>,
}

impl MySqlFixture {
    async fn start(reported_database: &str) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let commands = Arc::new(Mutex::new(Vec::new()));
        let received = commands.clone();
        let database = reported_database.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let flags = 0x000A_A20Du32;
            let mut handshake = vec![10];
            handshake.extend(b"8.0.0\0");
            handshake.extend(123u32.to_le_bytes());
            handshake.extend(b"12345678\0");
            handshake.extend((flags as u16).to_le_bytes());
            handshake.push(45);
            handshake.extend(2u16.to_le_bytes());
            handshake.extend(((flags >> 16) as u16).to_le_bytes());
            handshake.push(21);
            handshake.extend([0; 10]);
            handshake.extend(b"abcdefgh1234\0mysql_native_password\0");
            mysql_packet(&mut socket, 0, &handshake).await?;
            let login = mysql_receive(&mut socket).await?;
            let capabilities = u32::from_le_bytes(login[..4].try_into()?);
            let mut body = &login[32..];
            let _username = cstring(&mut body)?;
            let auth_length = take(&mut body, 1)?[0] as usize;
            take(&mut body, auth_length)?;
            anyhow::ensure!(
                capabilities & 8 != 0,
                "Target database missing in handshake"
            );
            received
                .lock()
                .unwrap()
                .push(format!("CONNECT {}", cstring(&mut body)?));
            let ok = [0, 0, 0, 2, 0, 0, 0];
            mysql_packet(&mut socket, 2, &ok).await?;
            while let Ok(command) = mysql_receive(&mut socket).await {
                if command.first() == Some(&1) {
                    break;
                }
                anyhow::ensure!(command.first() == Some(&3), "Expected COM_QUERY");
                let sql = String::from_utf8(command[1..].to_vec())?;
                received.lock().unwrap().push(sql.clone());
                if sql.contains("DATABASE()") {
                    mysql_database_row(&mut socket, &database).await?;
                } else {
                    let mut result = ok;
                    result[1] = u8::from(sql == JOB_SQL);
                    mysql_packet(&mut socket, 1, &result).await?;
                }
            }
            Ok(())
        });
        Ok(Self {
            address,
            commands,
            task,
        })
    }

    fn profile(&self) -> Profile {
        Profile {
            host: "127.0.0.1".into(),
            port: self.address.port(),
            ..Profile::default()
        }
    }
}

impl Drop for MySqlFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const JOB_SQL: &str = "UPDATE sqlshop_job_probe SET value = 777";

fn job(profile: &Profile, database: &str, schema: Option<&str>) -> JobSpec {
    JobSpec {
        id: uuid::Uuid::new_v4(),
        name: "target-bound job".into(),
        profile: profile.id,
        database: database.into(),
        schema: schema.map(str::to_owned),
        sql: JOB_SQL.into(),
        schedule: JobSchedule::EveryMinutes(1),
        enabled: true,
        connection_fingerprint: Some(serde_json::to_string(profile).unwrap()),
    }
}

#[tokio::test]
async fn mysql_job_accepts_database_schema_and_submits_user_sql_once() -> Result<()> {
    let fixture = MySqlFixture::start("app").await?;
    let profile = fixture.profile();
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        jobs::execute_sql_job(
            &profile,
            &Credentials::default(),
            &job(&profile, "app", Some("app")),
        ),
    )
    .await??;
    assert_eq!(
        output
            .sets
            .iter()
            .filter_map(|set| set.affected)
            .sum::<u64>(),
        1
    );
    let commands = fixture.commands.lock().unwrap();
    assert_eq!(commands.first().map(String::as_str), Some("CONNECT app"));
    assert_eq!(commands.iter().filter(|sql| *sql == JOB_SQL).count(), 1);
    Ok(())
}

#[tokio::test]
async fn mysql_job_rejects_a_different_schema_before_connecting() -> Result<()> {
    let fixture = MySqlFixture::start("app").await?;
    let profile = fixture.profile();
    let error = jobs::execute_sql_job(
        &profile,
        &Credentials::default(),
        &job(&profile, "app", Some("other")),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("Schema 必须与目标数据库一致"));
    assert!(fixture.commands.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn job_does_not_submit_sql_when_server_reports_another_database() -> Result<()> {
    let fixture = MySqlFixture::start("wrong_database").await?;
    let profile = fixture.profile();
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        jobs::execute_sql_job(
            &profile,
            &Credentials::default(),
            &job(&profile, "app", Some("app")),
        ),
    )
    .await?
    .unwrap_err();
    assert!(error.to_string().contains("连接未进入指定数据库"));
    assert!(
        !fixture
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|sql| sql == JOB_SQL)
    );
    Ok(())
}

#[tokio::test]
async fn postgres_job_connects_directly_to_restricted_target_and_submits_sql_once() -> Result<()> {
    let fixture = Fixture::start(&["app_only"]).await?;
    let profile = fixture.profile();
    tokio::time::timeout(
        Duration::from_secs(5),
        jobs::execute_sql_job(
            &profile,
            &Credentials::default(),
            &job(&profile, "app_only", None),
        ),
    )
    .await??;
    assert_eq!(*fixture.attempts.lock().unwrap(), ["app_only"]);
    let queries = fixture.queries.lock().unwrap();
    assert_eq!(
        queries
            .iter()
            .filter(|(database, sql)| database == "app_only" && sql == JOB_SQL)
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn postgres_restore_preflight_uses_only_target_and_never_runs_a_tool_on_rejection()
-> Result<()> {
    for database in ["app_only", "nonempty"] {
        let fixture = Fixture::start(&[database]).await?;
        let profile = fixture.profile();
        let directory = tempfile::tempdir()?;
        let mut config = DraftBackupConfig::for_database(&profile, database);
        config.operation = BackupOperation::Restore;
        config.location = BackupLocation::LocalFile(directory.path().join("missing.dump"));
        // Both the absent source file and absent executable ensure this test
        // cannot start a real restore, even if the preflight is accepted.
        config.tool_path = Some(directory.path().join("must-not-execute"));
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            backup::execute(&profile, &Credentials::default(), &config),
        )
        .await?
        .unwrap_err();
        assert!(
            error.to_string().contains(if database == "nonempty" {
                "恢复目标含有 1 个对象"
            } else {
                "恢复文件不存在或为空"
            }),
            "{error:#}"
        );
        assert!(
            fixture
                .attempts
                .lock()
                .unwrap()
                .iter()
                .all(|attempt| attempt == database)
        );
        assert!(
            fixture
                .queries
                .lock()
                .unwrap()
                .iter()
                .any(|(target, sql)| target == database && sql.contains("FROM pg_class"))
        );
    }
    Ok(())
}
