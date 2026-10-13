use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Engine {
    #[default]
    MySql,
    Postgres,
    SqlServer,
}

impl Engine {
    pub const ALL: [Self; 3] = [Self::MySql, Self::Postgres, Self::SqlServer];

    pub fn label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL",
            Self::Postgres => "PostgreSQL",
            Self::SqlServer => "SQL Server",
        }
    }

    pub fn port(self) -> u16 {
        match self {
            Self::MySql => 3306,
            Self::Postgres => 5432,
            Self::SqlServer => 1433,
        }
    }

    pub fn quote(self, identifier: &str) -> String {
        match self {
            Self::MySql => format!("`{}`", identifier.replace('`', "``")),
            Self::Postgres => format!("\"{}\"", identifier.replace('"', "\"\"")),
            Self::SqlServer => format!("[{}]", identifier.replace(']', "]]")),
        }
    }

    pub fn select_database_sql(self, database: &str) -> Option<String> {
        match self {
            Self::Postgres => None,
            _ => Some(format!("USE {};", self.quote(database))),
        }
    }

    pub fn preview_sql(self, table: &Table) -> String {
        let name = format!("{}.{}", self.quote(&table.schema), self.quote(&table.name));
        match self {
            Self::SqlServer => format!("SELECT TOP (200) * FROM {name};"),
            _ => format!("SELECT * FROM {name} LIMIT 200;"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub username: String,
    // Accepted from older saved profiles as a PostgreSQL bootstrap fallback.
    // New server connections leave this empty; older values remain available.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub database: String,
    pub tls: bool,
    pub trust_certificate: bool,
    pub remember_password: bool,
    #[serde(default)]
    pub ssh: SshConfig,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "本地 MySQL".into(),
            engine: Engine::MySql,
            host: "127.0.0.1".into(),
            port: 3306,
            username: "root".into(),
            database: String::new(),
            tls: true,
            trust_certificate: false,
            remember_password: true,
            ssh: SshConfig::default(),
        }
    }
}

impl Profile {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "请填写连接名称");
        anyhow::ensure!(!self.host.trim().is_empty(), "请填写主机地址");
        anyhow::ensure!(self.port > 0, "端口必须大于 0");
        anyhow::ensure!(!self.username.trim().is_empty(), "请填写用户名");
        anyhow::ensure!(
            !self.trust_certificate || self.tls,
            "信任服务器证书需要先启用 TLS"
        );
        if self.ssh.enabled {
            self.ssh.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshAuth {
    #[default]
    Password,
    PrivateKey,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    pub private_key: String,
    pub remember_secret: bool,
    pub known_hosts: String,
    pub fingerprint: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 22,
            username: "root".into(),
            auth: SshAuth::Password,
            private_key: "~/.ssh/id_ed25519".into(),
            remember_secret: true,
            known_hosts: "~/.ssh/known_hosts".into(),
            fingerprint: String::new(),
        }
    }
}

impl SshConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.host.trim().is_empty(), "请填写 SSH 主机");
        anyhow::ensure!(self.port > 0, "SSH 端口必须大于 0");
        anyhow::ensure!(!self.username.trim().is_empty(), "请填写 SSH 用户名");
        if self.auth == SshAuth::PrivateKey {
            anyhow::ensure!(!self.private_key.trim().is_empty(), "请选择 SSH 私钥文件");
        }
        anyhow::ensure!(
            !self.known_hosts.trim().is_empty() || !self.fingerprint.trim().is_empty(),
            "请指定 known_hosts 文件或服务器 SHA256 指纹"
        );
        if !self.fingerprint.trim().is_empty() {
            anyhow::ensure!(
                self.fingerprint.trim().starts_with("SHA256:"),
                "SSH 指纹必须使用 SHA256: 格式"
            );
        }
        Ok(())
    }
}

// Secrets are kept out of Profile, serde and Debug output.
#[derive(Clone, Default)]
pub struct Credentials {
    pub database_password: String,
    pub ssh_secret: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    /// SQL types from driver or table metadata, aligned with columns when available.
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub affected: Option<u64>,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub struct TableData {
    pub output: QueryOutput,
    pub primary_keys: Vec<String>,
    pub keys: Vec<Vec<Option<String>>>,
    pub columns: Vec<TableColumn>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Text,
    Integer,
    Unsigned,
    Decimal,
    Float,
    Binary,
    Boolean,
    Date,
    DateTime,
    Time,
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct TableColumn {
    pub name: String,
    /// Original catalog type, for display rather than parameter casts.
    pub declared_type: String,
    pub sql_type: String,
    pub kind: CellKind,
    pub editable: bool,
    pub primary_key: bool,
}

#[derive(Clone, Debug)]
pub struct CellUpdate {
    pub database: String,
    pub table: Table,
    pub primary_keys: Vec<String>,
    pub key_values: Vec<Option<String>>,
    pub column: String,
    pub original: Option<String>,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    pub sets: Vec<ResultSet>,
    pub elapsed_ms: u128,
}

pub fn export_csv(set: &ResultSet, path: &std::path::Path) -> anyhow::Result<()> {
    let mut writer = csv::Writer::from_path(path)?;
    writer.write_record(&set.columns)?;
    for row in &set.rows {
        writer.write_record(row.iter().map(|value| value.as_deref().unwrap_or("NULL")))?;
    }
    writer.flush()?;
    Ok(())
}
