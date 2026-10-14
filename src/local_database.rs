//! A persistent, file-bound SQLite connection for one document.
use crate::model::{QueryMessage, QueryOutput, ResultSet};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};
use uuid::Uuid;

pub struct LocalRequest {
    pub generation: Uuid,
    pub sql: String,
    pub confirmed: bool,
}
pub struct LocalResponse {
    pub generation: Uuid,
    pub output: Option<QueryOutput>,
    pub error: Option<String>,
    pub objects: Vec<(String, String, String)>,
    pub in_transaction: bool,
}
type Running = Arc<Mutex<Option<(Uuid, rusqlite::InterruptHandle)>>>;
pub struct LocalDatabaseWorker {
    sender: mpsc::Sender<LocalRequest>,
    receiver: mpsc::Receiver<LocalResponse>,
    running: Running,
}
impl LocalDatabaseWorker {
    pub fn start(path: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (sender, requests) = mpsc::channel::<LocalRequest>();
        let (responses, receiver) = mpsc::channel();
        let running: Running = Arc::new(Mutex::new(None));
        let execution = running.clone();
        std::thread::spawn(move || {
            if std::env::var_os("SQLSHOP_SCREENSHOT").is_some() {
                return;
            }
            let connected = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            );
            let connection = match connected {
                Ok(connection) => connection,
                Err(error) => {
                    while let Ok(request) = requests.recv() {
                        if responses
                            .send(LocalResponse {
                                generation: request.generation,
                                output: None,
                                error: Some(format!("打开 SQLite 失败：{error}")),
                                objects: Vec::new(),
                                in_transaction: false,
                            })
                            .is_err()
                        {
                            break;
                        }
                        wake();
                    }
                    return;
                }
            };
            let _ = connection.busy_timeout(std::time::Duration::from_secs(5));
            let configured = connection.execute_batch("PRAGMA foreign_keys = ON;");
            while let Ok(request) = requests.recv() {
                if let Ok(mut current) = execution.lock() {
                    *current = Some((request.generation, connection.get_interrupt_handle()));
                }
                let start = Instant::now();
                let result = match &configured {
                    Ok(()) => query(&connection, &request.sql, request.confirmed),
                    Err(error) => Err(anyhow::anyhow!("SQLite 外键保护初始化失败：{error}")),
                };
                if let Ok(mut current) = execution.lock() {
                    *current = None;
                }
                let (output, mut error) = match result {
                    Ok(mut output) => {
                        output.elapsed_ms = start.elapsed().as_millis();
                        (Some(output), None)
                    }
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                let objects = match catalog(&connection) {
                    Ok(objects) => objects,
                    Err(e) => {
                        error.get_or_insert_with(|| format!("对象目录读取失败：{e:#}"));
                        Vec::new()
                    }
                };
                if responses
                    .send(LocalResponse {
                        generation: request.generation,
                        output,
                        error,
                        objects,
                        in_transaction: !connection.is_autocommit(),
                    })
                    .is_err()
                {
                    break;
                }
                wake();
            }
            // Dropping an unfinished SQLite transaction rolls it back.
        });
        Self {
            sender,
            receiver,
            running,
        }
    }
    pub fn send(&self, request: LocalRequest) -> Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("SQLite 文档连接已关闭"))
    }
    pub fn try_recv(&self) -> Option<LocalResponse> {
        self.receiver.try_recv().ok()
    }
    pub fn interrupt(&self, generation: Uuid) -> Result<()> {
        let current = self
            .running
            .lock()
            .map_err(|_| anyhow::anyhow!("SQLite 停止状态不可用"))?;
        if let Some((active, handle)) = &*current
            && *active == generation
        {
            handle.interrupt();
        }
        Ok(())
    }
}

fn query(connection: &Connection, sql: &str, confirmed: bool) -> Result<QueryOutput> {
    anyhow::ensure!(!sql.trim().is_empty(), "请填写一条 SQLite 语句");
    let first = sql.split_whitespace().next().unwrap_or_default();
    let ordinary_read = first.eq_ignore_ascii_case("SELECT");
    // Some PRAGMAs change connection settings during preparation.
    anyhow::ensure!(
        ordinary_read || confirmed,
        "请勾选页内确认后执行当前 SQLite 语句"
    );
    let mut statement = connection.prepare(sql)?;
    anyhow::ensure!(
        (statement.readonly() && ordinary_read) || confirmed,
        "此语句可能改变 SQLite；请勾选页内确认后运行"
    );
    anyhow::ensure!(
        statement.parameter_count() == 0,
        "查询编辑器尚不支持未绑定参数"
    );
    // SQLite retains the first prepared statement's original text. Do not
    // prepare the tail: a second PRAGMA could take effect during preparation.
    let prepared = statement
        .expanded_sql()
        .context("无法核实 SQLite 语句边界")?;
    let tail = sql
        .strip_prefix(&prepared)
        .context("无法核实 SQLite 原文边界，语句未执行")?;
    anyhow::ensure!(
        trailing_trivia(tail),
        "一次只执行一条语句；尾部语句未执行。第一条 PRAGMA 可能在准备阶段改变连接设置"
    );
    let mut set = ResultSet {
        columns: statement
            .column_names()
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        column_types: statement
            .columns()
            .iter()
            .map(|column| column.decl_type().unwrap_or("").to_owned())
            .collect(),
        ..Default::default()
    };
    let dml = ["INSERT", "UPDATE", "DELETE", "REPLACE"]
        .iter()
        .any(|keyword| first.eq_ignore_ascii_case(keyword));
    let mut clipped_values = false;
    let read_only = statement.readonly();
    if set.columns.is_empty() {
        let affected = statement.execute([])? as u64;
        if dml {
            set.affected = Some(affected);
        }
    } else {
        let count = set.columns.len();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if set.rows.len() >= 1000 {
                set.truncated = true;
                if read_only {
                    break;
                }
                continue;
            }
            let mut values = Vec::with_capacity(count);
            for column in 0..count {
                values.push(match row.get_ref(column)? {
                    ValueRef::Null => None,
                    ValueRef::Integer(value) => Some(value.to_string()),
                    ValueRef::Real(value) => Some(value.to_string()),
                    ValueRef::Text(value) => {
                        let text =
                            std::str::from_utf8(value).context("SQLite 文本不是有效 UTF-8")?;
                        let mut preview: String = text.chars().take(8192).collect();
                        if preview.len() < text.len() {
                            clipped_values = true;
                            preview.push_str("…（值预览截断）");
                        }
                        Some(preview)
                    }
                    ValueRef::Blob(value) => {
                        if value.len() > 8192 {
                            clipped_values = true;
                        }
                        Some(format!(
                            "X'{}'{}",
                            value
                                .iter()
                                .take(8192)
                                .map(|byte| format!("{byte:02X}"))
                                .collect::<String>(),
                            if value.len() > 8192 {
                                "（值预览截断）"
                            } else {
                                ""
                            }
                        ))
                    }
                });
            }
            set.rows.push(values);
        }
    }
    Ok(QueryOutput {
        sets: vec![set],
        messages: if clipped_values {
            vec![QueryMessage {
                severity: "预览限制".into(),
                text: "长文本最多显示 8192 字符，BLOB 最多显示 8 KiB；带截断标记的值不是完整数据。"
                    .into(),
                ..Default::default()
            }]
        } else {
            Vec::new()
        },
        ..Default::default()
    })
}

fn trailing_trivia(mut text: &str) -> bool {
    loop {
        text = text.trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || character == ';'
        });
        if text.is_empty() {
            return true;
        }
        if let Some(comment) = text.strip_prefix("--") {
            text = comment.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
        } else if let Some(comment) = text.strip_prefix("/*") {
            let Some((_, rest)) = comment.split_once("*/") else {
                return true;
            };
            text = rest;
        } else {
            return false;
        }
    }
}

fn catalog(connection: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut statement = connection.prepare("SELECT name,type,COALESCE(sql,'') FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name LIMIT 1001")?;
    let objects = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(objects)
}
