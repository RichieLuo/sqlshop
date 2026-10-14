//! Oracle has its own persistent document actor. ODPI-C dynamically loads an
//! installed Oracle Client; opening a document never connects or installs it.
use crate::model::{QueryMessage, QueryOutput, ResultSet};
use anyhow::{Context, Result, ensure};
use oracle::{
    ConnStatus, Connection, StatementType,
    sql_type::{Blob, Clob, InnerValue, Nclob, OracleType},
};
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub struct OracleSettings {
    pub username: String,
    pub password: String,
    /// Easy Connect service, TNS alias, or full connect descriptor.
    pub connect_string: String,
}

#[derive(Clone, Debug)]
pub struct OracleObject {
    pub owner: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub last_ddl: Option<String>,
}

#[derive(Clone, Debug)]
pub enum OracleTransaction {
    Idle,
    Active(String),
    Unknown(String),
}
impl OracleTransaction {
    pub fn may_be_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    pub fn label(&self) -> String {
        match self {
            Self::Idle => "无活动本地事务".into(),
            Self::Active(id) => format!("事务 {id} · 手动提交"),
            Self::Unknown(reason) => format!("事务状态未知：{reason}"),
        }
    }
}

// Credentials deliberately have no Debug or serialization implementation.
pub enum OracleRequest {
    Connect(OracleSettings),
    Disconnect,
    Catalog { owner: String },
    Run { sql: String, allow_write: bool },
    Definition(OracleObject),
    Commit,
    Rollback,
    Transaction,
}

pub enum OracleData {
    Connected {
        schema: String,
    },
    Disconnected,
    Catalog {
        objects: Vec<OracleObject>,
        truncated: bool,
    },
    Output(QueryOutput),
    Definition {
        object: OracleObject,
        text: String,
        truncated: bool,
    },
    ConfirmSql {
        sql: String,
        kind: String,
    },
    Transaction,
}

pub enum OracleResponse {
    Finished {
        generation: u64,
        data: OracleData,
        connected: bool,
        transaction: OracleTransaction,
    },
    Failed {
        generation: u64,
        message: String,
        connected: bool,
        transaction: OracleTransaction,
    },
    StopRequested {
        generation: u64,
        message: String,
    },
}
struct Command {
    generation: u64,
    request: OracleRequest,
}
struct Running {
    generation: u64,
    connection: Arc<Connection>,
}

#[derive(Debug)]
struct OracleCommitUnconfirmed;
impl std::fmt::Display for OracleCommitUnconfirmed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("COMMIT 未获可靠确认，提交结果未知")
    }
}
impl std::error::Error for OracleCommitUnconfirmed {}

pub struct OracleWorker {
    sender: mpsc::SyncSender<Command>,
    pub receiver: mpsc::Receiver<OracleResponse>,
    active: Arc<Mutex<Option<Running>>>,
    responses: mpsc::Sender<OracleResponse>,
    stopping: Arc<AtomicU64>,
}
impl OracleWorker {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (sender, requests) = mpsc::sync_channel::<Command>(1);
        let (responses, receiver) = mpsc::channel();
        let active = Arc::new(Mutex::new(None));
        let worker_active = active.clone();
        let worker_responses = responses.clone();
        let wake = Arc::new(wake);
        std::thread::spawn(move || {
            let mut connection: Option<Arc<Connection>> = None;
            while let Ok(command) = requests.recv() {
                if let Some(connection) = &connection
                    && let Ok(mut active) = worker_active.lock()
                {
                    *active = Some(Running {
                        generation: command.generation,
                        connection: connection.clone(),
                    });
                }
                let commit = matches!(&command.request, OracleRequest::Commit);
                let result = operate(&mut connection, command.request);
                let mut transaction = connection
                    .as_ref()
                    .map_or(OracleTransaction::Idle, |connection| {
                        transaction_state(connection)
                    });
                let connected = connection.as_ref().is_some_and(|connection| {
                    matches!(connection.status(), Ok(ConnStatus::Normal))
                });
                let mut message = result.as_ref().err().map(|error| format!("{error:#}"));
                if !connected {
                    if connection.is_some() {
                        transaction = OracleTransaction::Unknown(
                            "连接失效，服务器最终状态未获确认；不会自动重连或重放".into(),
                        );
                    }
                    connection = None;
                }
                if (commit && result.is_err())
                    || result.as_ref().err().is_some_and(|error| {
                        error.downcast_ref::<OracleCommitUnconfirmed>().is_some()
                    })
                {
                    message = message.map(|message| format!("COMMIT 未获可靠确认，提交结果未知；请重新连接后核实，勿重复提交。{message}"));
                    transaction = OracleTransaction::Unknown("COMMIT 未确认".into());
                    connection = None;
                }
                // Stop holds this mutex while issuing break_execution. The next
                // command cannot be published until cancellation has finished.
                if let Ok(mut active) = worker_active.lock() {
                    *active = None;
                }
                let response = match result {
                    Ok(data) => OracleResponse::Finished {
                        generation: command.generation,
                        data,
                        connected: connection.is_some(),
                        transaction,
                    },
                    Err(_) => OracleResponse::Failed {
                        generation: command.generation,
                        message: message.unwrap_or_else(|| "Oracle 操作失败".into()),
                        connected: connection.is_some(),
                        transaction,
                    },
                };
                if worker_responses.send(response).is_err() {
                    break;
                }
                wake();
            }
            if let Ok(mut active) = worker_active.lock() {
                *active = None;
            }
        });
        Self {
            sender,
            receiver,
            active,
            responses,
            stopping: Arc::new(AtomicU64::new(u64::MAX)),
        }
    }

    pub fn send(&self, generation: u64, request: OracleRequest) -> Result<()> {
        self.sender
            .try_send(Command {
                generation,
                request,
            })
            .map_err(|_| anyhow::anyhow!("Oracle 工作线程已有待处理命令或已退出；原 SQL 未重放"))
    }

    pub fn stop(&self, generation: u64, wake: impl Fn() + Send + Sync + 'static) {
        if self
            .stopping
            .compare_exchange(u64::MAX, generation, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            let _ = self.responses.send(OracleResponse::StopRequested {
                generation,
                message: "已有停止请求在处理，等待 Oracle 驱动返回；没有发送第二次停止".into(),
            });
            wake();
            return;
        }
        let active = self.active.clone();
        let responses = self.responses.clone();
        let stopping = self.stopping.clone();
        let wake = Arc::new(wake);
        let waiting_wake = wake.clone();
        let waiting_responses = responses.clone();
        let (completed, completion) = mpsc::channel();
        std::thread::spawn(move || {
            if completion.recv_timeout(Duration::from_secs(5)).is_err() {
                let _ = waiting_responses.send(OracleResponse::StopRequested {generation,message:"Oracle 停止接口尚未返回；当前命令仍受保护，继续等待驱动完成，尚不能认定 SQL 已停止".into()});
                waiting_wake();
            }
        });
        std::thread::spawn(move || {
            let message = match active.lock() {
                Ok(active) => match active
                    .as_ref()
                    .filter(|running| running.generation == generation)
                {
                    Some(running) => match running.connection.break_execution() {
                        Ok(()) => {
                            "停止请求已发送至当前 Oracle 会话；是否停止以语句的服务器返回为准"
                                .into()
                        }
                        Err(error) => format!("停止失败：{error}"),
                    },
                    None => "此命令已完成，或尚在连接阶段，未取消其他命令".into(),
                },
                Err(_) => "无法取得 Oracle 停止句柄".into(),
            };
            let _ = responses.send(OracleResponse::StopRequested {
                generation,
                message,
            });
            stopping.store(u64::MAX, Ordering::Release);
            let _ = completed.send(());
            wake();
        });
    }
}

fn transaction_state(connection: &Connection) -> OracleTransaction {
    // The argument defaults to FALSE, so this observation never creates a transaction.
    match connection.query_row_as::<Option<String>>(
        "SELECT DBMS_TRANSACTION.LOCAL_TRANSACTION_ID FROM DUAL",
        &[],
    ) {
        Ok(Some(id)) => OracleTransaction::Active(id),
        Ok(None) => OracleTransaction::Idle,
        Err(error) => OracleTransaction::Unknown(error.to_string()),
    }
}

fn operate(connection: &mut Option<Arc<Connection>>, request: OracleRequest) -> Result<OracleData> {
    if let OracleRequest::Connect(settings) = request {
        ensure!(
            connection.is_none(),
            "Oracle 会话仍在线；请先明确断开后再更改连接"
        );
        ensure!(
            !settings.username.trim().is_empty() && !settings.connect_string.trim().is_empty(),
            "请填写 Oracle 用户名和服务/TNS 连接串"
        );
        let connected = Connection::connect(&settings.username, &settings.password, &settings.connect_string).map_err(|error| {
            if error.dpi_code() == Some(1047) || error.dpi_code() == Some(1072) {
                anyhow::anyhow!("Oracle Client 缺少或版本/架构不兼容。此连接需要本机已安装并可加载的 Oracle Instant Client；没有自动安装。驱动返回：{error}")
            } else { anyhow::Error::new(error) }
        })?;
        let schema = connected.query_row_as::<String>(
            "SELECT SYS_CONTEXT('USERENV','CURRENT_SCHEMA') FROM DUAL",
            &[],
        )?;
        *connection = Some(Arc::new(connected));
        return Ok(OracleData::Connected { schema });
    }
    let live = connection.as_ref().context("请先连接 Oracle")?;
    match request {
        OracleRequest::Connect(_) => unreachable!(),
        OracleRequest::Disconnect => {
            live.close()?;
            *connection = None;
            Ok(OracleData::Disconnected)
        }
        OracleRequest::Commit => {
            live.commit()?;
            Ok(OracleData::Transaction)
        }
        OracleRequest::Rollback => {
            live.rollback()?;
            Ok(OracleData::Transaction)
        }
        OracleRequest::Transaction => Ok(OracleData::Transaction),
        OracleRequest::Catalog { owner } => {
            ensure!(!owner.trim().is_empty(), "请指定对象所属 Schema");
            let rows = live.query("SELECT OWNER,OBJECT_NAME,OBJECT_TYPE,STATUS,TO_CHAR(LAST_DDL_TIME,'YYYY-MM-DD HH24:MI:SS') FROM (SELECT OWNER,OBJECT_NAME,OBJECT_TYPE,STATUS,LAST_DDL_TIME FROM ALL_OBJECTS WHERE OWNER=:1 AND SUBOBJECT_NAME IS NULL AND OBJECT_TYPE IN ('TABLE','VIEW','MATERIALIZED VIEW','FUNCTION','PROCEDURE','TRIGGER','SEQUENCE','INDEX','PACKAGE','PACKAGE BODY','TYPE','TYPE BODY','SYNONYM') ORDER BY OBJECT_TYPE,OBJECT_NAME) WHERE ROWNUM<=2001", &[&owner])?;
            let mut objects = Vec::new();
            for row in rows {
                let row = row?;
                objects.push(OracleObject {
                    owner: row.get(0)?,
                    name: row.get(1)?,
                    kind: row.get(2)?,
                    status: row.get(3)?,
                    last_ddl: row.get(4)?,
                });
            }
            let truncated = objects.len() > 2000;
            objects.truncate(2000);
            Ok(OracleData::Catalog { objects, truncated })
        }
        OracleRequest::Definition(object) => definition(live, object),
        OracleRequest::Run { sql, allow_write } => {
            ensure!(!sql.trim().is_empty(), "SQL 内容为空");
            ensure!(sql.len() <= 16 * 1024 * 1024, "SQL 超过 16 MiB 上限");
            let start = Instant::now();
            let mut statement = live
                .statement(&sql)
                .lob_locator()
                .fetch_array_size(50)
                .build()?;
            if !statement.is_query() && !allow_write {
                return Ok(OracleData::ConfirmSql {
                    sql,
                    kind: format!("{:?}", statement.statement_type()),
                });
            }
            let mut output = QueryOutput::default();
            if statement.is_query() {
                output.sets.push(collect_rows(statement.query(&[])?)?);
            } else {
                let executed = statement.execute(&[]);
                if statement.statement_type() == StatementType::Commit {
                    executed.context(OracleCommitUnconfirmed)?;
                } else {
                    executed?;
                }
                output.sets.push(ResultSet {
                    affected: if statement.is_dml() {
                        Some(statement.row_count()?)
                    } else {
                        None
                    },
                    ..Default::default()
                });
                while let Some(mut cursor) = statement.implicit_result()? {
                    if output.sets.len() >= crate::db::RESULT_LIMIT {
                        output.messages.push(QueryMessage {
                            severity: "提示".into(),
                            text: "隐式结果集超过显示上限，后续结果未读取".into(),
                            ..Default::default()
                        });
                        break;
                    }
                    output.sets.push(collect_rows(cursor.query()?)?);
                }
            }
            if statement.is_ddl() {
                output.messages.push(QueryMessage {
                    severity: "事务".into(),
                    text: "Oracle DDL 会隐式提交事务；请以服务器实际结果为准".into(),
                    ..Default::default()
                });
            }
            if let Some(warning) = live.last_warning() {
                output.messages.push(QueryMessage {
                    severity: "警告".into(),
                    text: warning.to_string(),
                    ..Default::default()
                });
            }
            output.elapsed_ms = start.elapsed().as_millis();
            Ok(OracleData::Output(output))
        }
    }
}

fn collect_rows(mut rows: oracle::ResultSet<'_, oracle::Row>) -> Result<ResultSet> {
    let mut set = ResultSet {
        columns: rows
            .column_info()
            .iter()
            .map(|column| column.name().to_owned())
            .collect(),
        column_types: rows
            .column_info()
            .iter()
            .map(|column| column.oracle_type().to_string())
            .collect(),
        ..Default::default()
    };
    let mut bytes = 0usize;
    for row in &mut rows {
        let row = row?;
        if set.rows.len() >= crate::db::ROW_LIMIT {
            set.truncated = true;
            break;
        }
        ensure!(
            row.sql_values().len() == set.columns.len(),
            "Oracle 返回的字段数与结果元数据不一致，未伪造缺失单元格"
        );
        let mut values = Vec::new();
        for value in row.sql_values() {
            if value.is_null()? {
                values.push(None);
                continue;
            }
            let (text, truncated) = match value.oracle_type()? {
                OracleType::Raw(_) | OracleType::LongRaw => {
                    let InnerValue::Raw(bytes) = value.as_inner_value()? else {
                        anyhow::bail!("Oracle RAW 的原始返回类型不一致");
                    };
                    let truncated = bytes.len() > 8 * 1024;
                    let mut text = hex(&bytes[..bytes.len().min(8 * 1024)]);
                    if truncated {
                        text.push_str("… [RAW 预览截断]");
                    }
                    (text, truncated)
                }
                OracleType::CLOB => lob_text(value.get::<Clob>()?, 8 * 1024)?,
                OracleType::NCLOB => lob_text(value.get::<Nclob>()?, 8 * 1024)?,
                OracleType::BLOB => {
                    let mut content = Vec::new();
                    value
                        .get::<Blob>()?
                        .take(8 * 1024 + 1)
                        .read_to_end(&mut content)?;
                    let truncated = content.len() > 8 * 1024;
                    content.truncate(8 * 1024);
                    let mut text = hex(&content);
                    if truncated {
                        text.push_str("… [BLOB 预览截断]");
                    }
                    (text, truncated)
                }
                OracleType::BFILE
                | OracleType::Object(_)
                | OracleType::RefCursor
                | OracleType::Json
                | OracleType::Xml => (format!("[暂不支持的类型：{}]", value.oracle_type()?), true),
                _ => (value.get::<String>()?, false),
            };
            set.truncated |= truncated;
            bytes = bytes.saturating_add(text.len());
            values.push(Some(text));
        }
        if bytes > 16 * 1024 * 1024 {
            set.truncated = true;
            break;
        }
        set.rows.push(values);
    }
    Ok(set)
}

fn definition(connection: &Connection, object: OracleObject) -> Result<OracleData> {
    let kind = match object.kind.as_str() {
        "MATERIALIZED VIEW" => "MATERIALIZED_VIEW",
        "PACKAGE BODY" => "PACKAGE_BODY",
        "TYPE BODY" => "TYPE_BODY",
        "TABLE" | "VIEW" | "INDEX" | "SEQUENCE" | "FUNCTION" | "PROCEDURE" | "TRIGGER"
        | "PACKAGE" | "TYPE" | "SYNONYM" => object.kind.as_str(),
        _ => anyhow::bail!("此 Oracle 对象类型暂不支持读取 DDL"),
    };
    let mut statement = connection
        .statement("SELECT DBMS_METADATA.GET_DDL(:1,:2,:3) FROM DUAL")
        .lob_locator()
        .build()?;
    let row = statement.query_row(&[&kind, &object.name, &object.owner])?;
    let content: Option<Clob> = row.get(0)?;
    let content = content.context("服务器未返回对象 DDL（对象可能不存在或无读取权限）")?;
    let (text, truncated) = lob_text(content, 16 * 1024 * 1024)?;
    Ok(OracleData::Definition {
        object,
        text,
        truncated,
    })
}
fn lob_text(lob: impl Read, limit: usize) -> Result<(String, bool)> {
    let mut bytes = Vec::new();
    lob.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push_str("\n… [LOB 预览截断]");
    }
    Ok((text, truncated))
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::from("0x");
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
