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
    #[serde(default)]
    pub sqlserver_auth: SqlServerAuth,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sqlserver_instance: String,
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
            sqlserver_auth: SqlServerAuth::Password,
            sqlserver_instance: String::new(),
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
        if self.engine != Engine::SqlServer
            || self.sqlserver_auth == SqlServerAuth::Password
            || self.sqlserver_auth == SqlServerAuth::Windows
        {
            anyhow::ensure!(!self.username.trim().is_empty(), "请填写用户名");
        }
        if self.engine == Engine::SqlServer && !self.sqlserver_instance.is_empty() {
            anyhow::ensure!(
                !self.ssh.enabled,
                "命名实例发现使用 UDP；SSH 连接请填写已确定的 TCP 端口并清空实例名"
            );
            anyhow::ensure!(
                self.sqlserver_instance.len() <= 128
                    && self
                        .sqlserver_instance
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'$')),
                "实例名称包含无效字符"
            );
        }
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
pub enum SqlServerAuth {
    #[default]
    Password,
    AadToken,
    Integrated,
    Windows,
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
pub struct CreateTableColumn {
    pub name: String,
    pub data_type: String,
    pub size: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub auto_increment: bool,
    pub default_value: String,
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
    /// The actual relation supports ordinary transactional table writes.
    pub writable: bool,
}

impl TableData {
    pub fn can_delete_rows(&self) -> bool {
        self.writable
            && !self.primary_keys.is_empty()
            && self
                .columns
                .iter()
                .filter(|column| column.primary_key)
                .all(|column| column.kind != CellKind::Unsupported)
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertValue {
    Default,
    Null,
    Text(String),
}

#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    pub sets: Vec<ResultSet>,
    pub messages: Vec<QueryMessage>,
    pub elapsed_ms: u128,
}

/// A server notice or informational token, separate from local execution stats.
#[derive(Clone, Debug, Default)]
pub struct QueryMessage {
    pub severity: String,
    pub code: Option<String>,
    pub text: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub context: Option<String>,
}

/// Structured fields supplied by the driver. Positions refer to the SQL sent
/// to the server, not to a possibly larger or subsequently edited document.
#[derive(Clone, Debug, Default)]
pub struct QueryDiagnostic {
    pub code: Option<String>,
    pub position: Option<QueryErrorPosition>,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub context: Option<String>,
}

#[derive(Clone, Debug)]
pub struct QueryErrorPosition {
    /// One-based line, and column when the server supplies a character offset.
    pub line: usize,
    pub column: Option<usize>,
    /// Zero-based Unicode scalar index. Never derived from error message text.
    pub char_offset: usize,
}

// A nullable cell is encoded as an unquoted empty field; every non-null value is
// quoted, including empty strings and the literal "NULL". Import uses the same rule.
pub fn write_csv_row<W: std::io::Write>(
    writer: &mut W,
    row: &[Option<String>],
) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    for (index, value) in row.iter().enumerate() {
        if index > 0 {
            bytes.push(b',');
        }
        if let Some(value) = value {
            bytes.push(b'"');
            for byte in value.as_bytes() {
                if *byte == b'"' {
                    bytes.push(b'"');
                }
                bytes.push(*byte);
            }
            bytes.push(b'"');
        }
    }
    bytes.extend_from_slice(b"\r\n");
    writer.write_all(&bytes)?;
    Ok(())
}

// Parse fields and quotation together so record boundaries cannot drift from
// NULL detection. In a one-column CSV, a blank record represents one NULL.
fn nullable_csv_record(
    bytes: &[u8],
    offset: &mut usize,
) -> anyhow::Result<Option<(Vec<InsertValue>, bool)>> {
    if *offset == bytes.len() {
        return Ok(None);
    }
    let blank = matches!(bytes[*offset], b'\r' | b'\n');
    let mut fields = Vec::new();
    loop {
        let quoted = bytes.get(*offset) == Some(&b'"');
        let mut value = Vec::new();
        if quoted {
            *offset += 1;
            loop {
                let byte = bytes
                    .get(*offset)
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("CSV 引号未闭合"))?;
                *offset += 1;
                if byte == b'"' {
                    if bytes.get(*offset) == Some(&b'"') {
                        value.push(b'"');
                        *offset += 1;
                    } else {
                        break;
                    }
                } else {
                    value.push(byte);
                }
            }
            anyhow::ensure!(
                bytes
                    .get(*offset)
                    .is_none_or(|byte| matches!(byte, b',' | b'\r' | b'\n')),
                "CSV 引号后存在无效字符"
            );
        } else {
            while let Some(byte) = bytes.get(*offset).copied() {
                if matches!(byte, b',' | b'\r' | b'\n') {
                    break;
                }
                anyhow::ensure!(byte != b'"', "CSV 未加引号字段包含双引号");
                value.push(byte);
                *offset += 1;
            }
        }
        fields.push(if value.is_empty() && !quoted {
            InsertValue::Null
        } else {
            InsertValue::Text(String::from_utf8(value)?)
        });
        match bytes.get(*offset) {
            Some(b',') => *offset += 1,
            Some(b'\r' | b'\n') => {
                let carriage_return = bytes[*offset] == b'\r';
                *offset += 1;
                if carriage_return && bytes.get(*offset) == Some(&b'\n') {
                    *offset += 1;
                }
                break;
            }
            None => break,
            _ => unreachable!("validated CSV delimiter"),
        }
    }
    Ok(Some((fields, blank)))
}

pub fn parse_nullable_csv(bytes: &[u8]) -> anyhow::Result<(Vec<String>, Vec<Vec<InsertValue>>)> {
    let text = std::str::from_utf8(bytes)?;
    let bytes = text.strip_prefix('\u{feff}').unwrap_or(text).as_bytes();
    let mut offset = 0;
    let names = loop {
        let (record, blank) = nullable_csv_record(bytes, &mut offset)?
            .ok_or_else(|| anyhow::anyhow!("CSV 缺少表头"))?;
        if !blank {
            break record
                .into_iter()
                .map(|value| match value {
                    InsertValue::Text(text) => text,
                    _ => String::new(),
                })
                .collect::<Vec<_>>();
        }
    };
    let mut rows = Vec::new();
    while let Some((record, blank)) = nullable_csv_record(bytes, &mut offset)? {
        // Empty lines in a multi-column file are separators, not partial rows.
        if blank && names.len() > 1 {
            continue;
        }
        anyhow::ensure!(record.len() == names.len(), "CSV 数据列数与表头不符");
        rows.push(record);
    }
    Ok((names, rows))
}

pub fn export_csv(set: &ResultSet, path: &std::path::Path) -> anyhow::Result<()> {
    let mut writer = std::io::BufWriter::new(std::fs::File::create(path)?);
    write_csv_row(
        &mut writer,
        &set.columns.iter().cloned().map(Some).collect::<Vec<_>>(),
    )?;
    for row in &set.rows {
        write_csv_row(&mut writer, row)?;
    }
    std::io::Write::flush(&mut writer)?;
    Ok(())
}

#[cfg(test)]
mod csv_workflow_tests {
    use super::*;

    #[test]
    fn single_column_null_records_are_not_skipped() {
        let set = ResultSet {
            columns: vec!["note".into()],
            rows: vec![
                vec![None],
                vec![None],
                vec![Some(String::new())],
                vec![Some("中文\r\n\"quote\"".into())],
                vec![None],
            ],
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("null.csv");
        export_csv(&set, &path).unwrap();
        let (headers, rows) = parse_nullable_csv(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(headers, set.columns);
        assert_eq!(
            rows,
            vec![
                vec![InsertValue::Null],
                vec![InsertValue::Null],
                vec![InsertValue::Text(String::new())],
                vec![InsertValue::Text("中文\r\n\"quote\"".into())],
                vec![InsertValue::Null]
            ]
        );
    }

    #[test]
    fn blank_lines_do_not_change_quoted_empty_fields() {
        for newline in ["\n", "\r\n", "\r"] {
            let input = format!(
                "\u{feff}{newline}a,b{newline}{newline}{newline}\"\",x{newline},\"\"{newline}"
            );
            let (headers, rows) = parse_nullable_csv(input.as_bytes()).unwrap();
            assert_eq!(headers, ["a", "b"]);
            assert_eq!(
                rows,
                vec![
                    vec![
                        InsertValue::Text(String::new()),
                        InsertValue::Text("x".into())
                    ],
                    vec![InsertValue::Null, InsertValue::Text(String::new())],
                ]
            );
        }
        assert_eq!(
            parse_nullable_csv(b"a,b\nx,").unwrap().1,
            vec![vec![InsertValue::Text("x".into()), InsertValue::Null]]
        );
    }

    #[test]
    fn malformed_csv_is_rejected_before_import() {
        for input in [
            "",
            "\n\r\n",
            "a,b\nx",
            "a\n\"unclosed",
            "a\n\"x\"tail",
            "a\nun\"quoted",
        ] {
            assert!(parse_nullable_csv(input.as_bytes()).is_err(), "{input:?}");
        }
    }

    #[test]
    fn null_empty_and_literal_null_round_trip_including_multiline() {
        let mut bytes = Vec::new();
        write_csv_row(&mut bytes, &[Some("note".into()), Some("value".into())]).unwrap();
        write_csv_row(&mut bytes, &[None, Some("NULL".into())]).unwrap();
        write_csv_row(
            &mut bytes,
            &[Some(String::new()), Some("line\n\"quote\",x".into())],
        )
        .unwrap();
        let (headers, rows) = parse_nullable_csv(&bytes).unwrap();
        assert_eq!(headers, ["note", "value"]);
        assert_eq!(
            rows,
            vec![
                vec![InsertValue::Null, InsertValue::Text("NULL".into())],
                vec![
                    InsertValue::Text(String::new()),
                    InsertValue::Text("line\n\"quote\",x".into())
                ],
            ]
        );
    }
}
