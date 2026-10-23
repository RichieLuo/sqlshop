//! An independent, bounded Redis worker. No SQL session, reconnect or write replay.
use crate::{
    model::{Credentials, SshConfig},
    ssh::Tunnel,
};
use anyhow::{Context, Result, bail};
use redis::{AsyncConnectionConfig, RedisConnectionInfo, Value, aio::MultiplexedConnection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    task::JoinHandle,
};
use uuid::Uuid;

pub const MAX_VALUE_BYTES: usize = 1024 * 1024;
pub const MAX_COLLECTION_ITEMS: usize = 200;
const MAX_WIRE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 256;
const QUEUE_SIZE: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisProfile {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    // Kept for old saved profiles only. New connections always start at DB 0.
    pub database: u32,
    pub tls: bool,
    pub remember_password: bool,
    pub ssh: SshConfig,
}

impl Default for RedisProfile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "本地 Redis".into(),
            host: "localhost".into(),
            port: 6379,
            username: String::new(),
            database: 0,
            tls: false,
            remember_password: true,
            ssh: SshConfig::default(),
        }
    }
}

impl RedisProfile {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "请填写连接名称");
        anyhow::ensure!(!self.host.trim().is_empty(), "请填写 Redis 主机地址");
        anyhow::ensure!(self.port > 0, "Redis 端口必须大于 0");
        anyhow::ensure!(
            !self.host.contains(['/', '\\', '@', '\r', '\n', '\0']),
            "Redis 主机地址无效，请只填写主机名或 IP"
        );
        anyhow::ensure!(
            !(self.tls && self.ssh.enabled),
            "暂不支持 Redis TLS + SSH 组合；不会关闭证书或主机名校验，请使用直接 TLS 或仅 SSH"
        );
        if self.ssh.enabled {
            self.ssh.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedisKind {
    String,
    Hash,
    List,
    Set,
    ZSet,
    Stream,
    Other(String),
}
impl RedisKind {
    pub fn label(&self) -> &str {
        match self {
            Self::String => "string",
            Self::Hash => "hash",
            Self::List => "list",
            Self::Set => "set",
            Self::ZSet => "zset",
            Self::Stream => "stream",
            Self::Other(name) => name,
        }
    }
    fn from_label(name: String) -> Self {
        match name.as_str() {
            "string" => Self::String,
            "hash" => Self::Hash,
            "list" => Self::List,
            "set" => Self::Set,
            "zset" => Self::ZSet,
            "stream" => Self::Stream,
            _ => Self::Other(name),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RedisMutation {
    SetString {
        value: Vec<u8>,
        expected: Vec<u8>,
    },
    HashSet {
        field: Vec<u8>,
        value: Vec<u8>,
    },
    HashDelete {
        field: Vec<u8>,
    },
    ListSet {
        index: i64,
        value: Vec<u8>,
        expected: Vec<u8>,
    },
    ListPush {
        value: Vec<u8>,
    },
    SetAdd {
        value: Vec<u8>,
    },
    SetRemove {
        value: Vec<u8>,
    },
    ZSetAdd {
        score: f64,
        value: Vec<u8>,
    },
    ZSetRemove {
        value: Vec<u8>,
    },
    Expire {
        seconds: i64,
    },
    Persist,
    Rename {
        new_key: Vec<u8>,
    },
    Delete,
}

pub enum RedisRequest {
    Connect {
        profile: RedisProfile,
        credentials: Credentials,
    },
    Disconnect,
    Databases,
    SelectDb(u32),
    PreviewNamespace {
        prefix: Vec<u8>,
    },
    DeleteNamespace {
        token: u64,
    },
    Scan {
        cursor: u64,
        pattern: String,
    },
    ReadKey {
        key: Vec<u8>,
    },
    Mutate {
        key: Vec<u8>,
        change: RedisMutation,
    },
    Create {
        key: Vec<u8>,
        kind: RedisKind,
        value: Vec<u8>,
        field: Vec<u8>,
        score: f64,
    },
    Command {
        args: Vec<Vec<u8>>,
        confirmed: bool,
    },
}

#[derive(Clone, Debug)]
pub enum RedisValue {
    String(Vec<u8>),
    Hash(Vec<(Vec<u8>, Vec<u8>)>),
    List(Vec<Vec<u8>>),
    Set(Vec<Vec<u8>>),
    ZSet(Vec<(Vec<u8>, f64)>),
    Unsupported(String),
}
#[derive(Clone, Debug)]
pub struct RedisKey {
    pub key: Vec<u8>,
    pub kind: RedisKind,
    pub ttl_ms: i64,
    pub size: u64,
    pub truncated: bool,
    pub value: RedisValue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedisDatabase {
    pub index: u32,
    pub keys: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum RedisData {
    Connected {
        database: u32,
        server: String,
    },
    Disconnected,
    Databases {
        databases: Vec<RedisDatabase>,
        complete: bool,
        notice: Option<String>,
    },
    Database(u32),
    NamespacePreview {
        token: u64,
        prefix: Vec<u8>,
        count: usize,
    },
    NamespaceDeleted {
        prefix: Vec<u8>,
        deleted: u64,
    },
    Keys {
        cursor: u64,
        keys: Vec<Vec<u8>>,
    },
    Key(RedisKey),
    Changed {
        key: Vec<u8>,
        deleted: bool,
    },
    Output(String),
}
pub struct RedisResponse {
    pub generation: u64,
    pub database: u32,
    pub connected: bool,
    pub result: std::result::Result<RedisData, String>,
}

pub struct RedisWorker {
    sender: mpsc::SyncSender<(u64, RedisRequest)>,
    pub receiver: mpsc::Receiver<RedisResponse>,
}
impl RedisWorker {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (sender, requests) = mpsc::sync_channel::<(u64, RedisRequest)>(QUEUE_SIZE);
        let (responses, receiver) = mpsc::sync_channel(QUEUE_SIZE);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let mut connection: Option<RedisConnection> = None;
            let mut database = 0;
            while let Ok((generation, request)) = requests.recv() {
                let result = match &runtime {
                    Ok(runtime) => {
                        runtime.block_on(handle_request(&mut connection, &mut database, request))
                    }
                    Err(_) => Err(anyhow::anyhow!("Redis 后台运行时初始化失败")),
                };
                let response = RedisResponse {
                    generation,
                    database,
                    connected: connection.is_some(),
                    result: result.map_err(|error| format!("{error:#}")),
                };
                if responses.send(response).is_err() {
                    break;
                }
                wake();
            }
        });
        Self { sender, receiver }
    }
    pub fn send(&self, generation: u64, request: RedisRequest) -> Result<()> {
        validate_request(&request)?;
        self.sender
            .try_send((generation, request))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    anyhow::anyhow!("Redis 请求队列已满，请等待当前操作完成")
                }
                mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("Redis 后台连接已关闭"),
            })
    }
}

fn validate_request(request: &RedisRequest) -> Result<()> {
    match request {
        RedisRequest::Connect {
            profile,
            credentials,
        } => {
            profile.validate()?;
            anyhow::ensure!(
                credentials.database_password.len() + credentials.ssh_secret.len()
                    <= MAX_VALUE_BYTES,
                "Redis 凭据超过大小限制"
            );
        }
        RedisRequest::Scan { pattern, .. } => check_bytes(pattern.as_bytes())?,
        RedisRequest::PreviewNamespace { prefix } => {
            check_bytes(prefix)?;
            anyhow::ensure!(
                prefix.ends_with(b":"),
                "命名空间前缀必须非空并包含末尾的 :；不能删除整个数据库"
            );
        }
        RedisRequest::ReadKey { key } => check_bytes(key)?,
        RedisRequest::Mutate { key, change } => {
            check_bytes(key)?;
            match change {
                RedisMutation::SetString { value, expected }
                | RedisMutation::ListSet {
                    value, expected, ..
                } => {
                    check_bytes(value)?;
                    check_bytes(expected)?;
                }
                RedisMutation::HashSet { field, value } => {
                    check_bytes(field)?;
                    check_bytes(value)?;
                }
                RedisMutation::HashDelete { field } => check_bytes(field)?,
                RedisMutation::ListPush { value }
                | RedisMutation::SetAdd { value }
                | RedisMutation::SetRemove { value }
                | RedisMutation::ZSetRemove { value } => check_bytes(value)?,
                RedisMutation::ZSetAdd { score, value } => {
                    check_bytes(value)?;
                    check_score(*score)?;
                }
                RedisMutation::Rename { new_key } => check_bytes(new_key)?,
                RedisMutation::Expire { seconds } => {
                    anyhow::ensure!(*seconds > 0, "TTL 必须大于 0；删除请使用删除操作")
                }
                _ => {}
            }
        }
        RedisRequest::Create {
            key,
            value,
            field,
            score,
            kind,
        } => {
            check_bytes(key)?;
            check_bytes(value)?;
            check_bytes(field)?;
            anyhow::ensure!(
                matches!(
                    kind,
                    RedisKind::String
                        | RedisKind::Hash
                        | RedisKind::List
                        | RedisKind::Set
                        | RedisKind::ZSet
                ),
                "此 Redis 类型暂不支持新建"
            );
            if *kind == RedisKind::ZSet {
                check_score(*score)?;
            }
        }
        RedisRequest::Command { args, confirmed } => {
            validate_command(args, *confirmed)?;
        }
        _ => {}
    }
    Ok(())
}
fn check_bytes(bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(bytes.len() <= MAX_VALUE_BYTES, "Redis 参数超过 1 MiB 限制");
    Ok(())
}
fn check_score(score: f64) -> Result<()> {
    anyhow::ensure!(score.is_finite(), "Redis 分数必须是有限数字");
    Ok(())
}

// Transport and command implementation live separately to keep the public contract readable.
mod commands;
mod transport;
use commands::{handle_request, validate_command};
use transport::RedisConnection;

/// Parse one binary-safe command; no shell expansion, statements or credential history.
pub fn parse_command(input: &str) -> Result<Vec<Vec<u8>>> {
    anyhow::ensure!(input.len() <= MAX_VALUE_BYTES, "Redis 命令超过 1 MiB 限制");
    let bytes = input.as_bytes();
    let mut args = Vec::new();
    let mut token = Vec::new();
    let mut quote = None;
    let mut started = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte == b'\\' {
            started = true;
            let escaped = *bytes.get(index).context("命令末尾转义不完整")?;
            index += 1;
            token.push(match escaped {
                b'x' => {
                    let high = *bytes.get(index).context("\\x 需要两位十六进制数字")?;
                    let low = *bytes.get(index + 1).context("\\x 需要两位十六进制数字")?;
                    index += 2;
                    hex_digit(high)? * 16 + hex_digit(low)?
                }
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'b' => 8,
                b'a' => 7,
                b'\\' | b'\'' | b'"' | b' ' => escaped,
                _ => bail!("不支持的转义；二进制字节请使用 \\xNN"),
            });
        } else if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            } else {
                token.push(byte);
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            started = true;
        } else if byte.is_ascii_whitespace() {
            if started {
                args.push(std::mem::take(&mut token));
                started = false;
            }
        } else {
            token.push(byte);
            started = true;
        }
        anyhow::ensure!(args.len() < MAX_ARGUMENTS, "Redis 命令参数过多");
    }
    anyhow::ensure!(quote.is_none(), "Redis 命令引号未闭合");
    if started {
        args.push(token);
    }
    anyhow::ensure!(
        !args.is_empty() && !args[0].is_empty(),
        "请填写一条 Redis 命令"
    );
    Ok(args)
}

pub fn command_needs_confirmation(args: &[Vec<u8>]) -> bool {
    !commands::readonly_command(args)
}

pub fn display_bytes(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.chars().any(char::is_control)
    {
        return text.to_owned();
    }
    bytes.iter().map(|byte| format!("\\x{byte:02X}")).collect()
}
fn hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("无效的十六进制数字"),
    }
}
pub fn parse_hex(input: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(
        input.len() <= MAX_VALUE_BYTES * 3,
        "十六进制输入超过大小限制"
    );
    let digits: Vec<_> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    anyhow::ensure!(
        digits.len().is_multiple_of(2),
        "十六进制输入必须包含完整的字节对"
    );
    anyhow::ensure!(
        digits.len() / 2 <= MAX_VALUE_BYTES,
        "Redis 参数超过 1 MiB 限制"
    );
    digits
        .chunks_exact(2)
        .map(|pair| Ok(hex_digit(pair[0])? * 16 + hex_digit(pair[1])?))
        .collect()
}
pub fn format_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().saturating_mul(3));
    use std::fmt::Write;
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        let _ = write!(text, "{byte:02X}");
    }
    text
}
