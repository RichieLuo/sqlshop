mod browser;
mod structure;
mod table_edit;
pub use browser::{
    ColumnFilter, ColumnSort, FilterOperator, TableBrowseOptions, TablePage, TableStructure,
};
mod values;

pub(crate) use table_edit::EditRejected;

use crate::{
    model::{Credentials, Engine, Profile, QueryOutput, ResultSet, Table},
    ssh::Tunnel,
};
use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use sqlx::{
    Column, Connection, Either, Row, TypeInfo, mysql::MySqlConnectOptions,
    postgres::PgConnectOptions,
};
use std::time::Instant;
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub const ROW_LIMIT: usize = 1000;
pub const RESULT_LIMIT: usize = 32;

pub fn connection_error(profile: &Profile, error: anyhow::Error) -> anyhow::Error {
    if !profile.tls {
        return error;
    }
    let detail = format!("{error:#}");
    if detail.contains("UnknownIssuer") {
        error.context("数据库 TLS 证书的签发机构不受本机信任。确认服务器可信后，在连接窗口的“数据库 TLS”区域勾选“信任服务器证书（跳过证书校验）”，保留 TLS 加密并重新测试；或将服务器 CA 加入本机受信任证书库。此错误与 SSH 私钥路径无关")
    } else if detail.contains("UnsupportedCertVersion") {
        error.context("数据库 TLS 证书版本不受驱动支持，这可能来自 SQL Server 自动生成的证书。请为服务器配置受支持的证书，或确认服务器可信后，在连接窗口的“数据库 TLS”区域勾选“信任服务器证书（跳过证书校验）”，保留 TLS 加密并重新测试")
    } else if detail.contains("NotValidForName") {
        error.context(if profile.ssh.enabled && profile.engine != Engine::SqlServer {
            "数据库 TLS 证书的主机名与隧道转发地址不匹配，当前 MySQL / PostgreSQL 隧道校验的名称是 127.0.0.1。请使用匹配的证书，或确认服务器可信后在连接窗口的“数据库 TLS”区域明确选择“信任服务器证书（跳过证书校验）”"
        } else {
            "数据库 TLS 证书的主机名与连接地址不匹配。请使用证书中的数据库地址，或更新服务器证书"
        })
    } else {
        error
    }
}

enum Driver {
    MySql(sqlx::MySqlConnection),
    Postgres(sqlx::PgConnection),
    SqlServer(Box<Client<Compat<TcpStream>>>),
}

pub struct Session {
    driver: Driver,
    _tunnel: Option<Tunnel>,
    profile: Profile,
    credentials: Credentials,
    raw_query_used: bool,
}

impl Session {
    pub async fn connect(profile: &Profile, credentials: &Credentials) -> Result<Self> {
        if profile.engine != Engine::Postgres {
            return Self::open(profile, credentials, None).await;
        }
        let mut candidates = vec![
            "postgres".to_owned(),
            profile.username.clone(),
            "template1".to_owned(),
        ];
        if !profile.database.is_empty() {
            candidates.push(profile.database.clone());
        }
        let mut attempted = Vec::new();
        let mut last_error = None;
        for database in candidates {
            if attempted.contains(&database) {
                continue;
            }
            attempted.push(database.clone());
            match Self::open(profile, credentials, Some(&database)).await {
                Ok(session) => return Ok(session),
                Err(error) => {
                    let retry = error
                        .downcast_ref::<sqlx::Error>()
                        .and_then(|error| error.as_database_error())
                        .and_then(|error| error.code())
                        .is_some_and(|code| matches!(code.as_ref(), "3D000" | "42501" | "28000"));
                    if !retry {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("没有可尝试的初始化数据库")))
            .context("PostgreSQL 需要一个可访问的初始化库来读取数据库列表。已尝试 postgres、用户默认库及 template1；请检查账号的 CONNECT 权限")
    }

    async fn open(
        profile: &Profile,
        credentials: &Credentials,
        database: Option<&str>,
    ) -> Result<Self> {
        profile.validate()?;
        let tunnel = if profile.ssh.enabled {
            Some(
                Tunnel::open(
                    &profile.ssh,
                    &credentials.ssh_secret,
                    &profile.host,
                    profile.port,
                )
                .await?,
            )
        } else {
            None
        };
        let (host, port) = tunnel
            .as_ref()
            .map(|t| ("127.0.0.1", t.local_addr.port()))
            .unwrap_or((&profile.host, profile.port));
        let password = &credentials.database_password;
        let driver = match profile.engine {
            Engine::MySql => {
                use sqlx::mysql::MySqlSslMode;
                let mode = match (profile.tls, profile.trust_certificate) {
                    (false, _) => MySqlSslMode::Disabled,
                    (true, true) => MySqlSslMode::Required,
                    (true, false) => MySqlSslMode::VerifyIdentity,
                };
                let mut options = MySqlConnectOptions::new()
                    .host(host)
                    .port(port)
                    .username(&profile.username)
                    .password(password)
                    .ssl_mode(mode);
                if let Some(database) = database {
                    options = options.database(database);
                }
                Driver::MySql(sqlx::MySqlConnection::connect_with(&options).await?)
            }
            Engine::Postgres => {
                use sqlx::postgres::PgSslMode;
                let mode = match (profile.tls, profile.trust_certificate) {
                    (false, _) => PgSslMode::Disable,
                    (true, true) => PgSslMode::Require,
                    (true, false) => PgSslMode::VerifyFull,
                };
                let mut options = PgConnectOptions::new()
                    .host(host)
                    .port(port)
                    .username(&profile.username)
                    .password(password)
                    .ssl_mode(mode);
                if let Some(database) = database {
                    options = options.database(database);
                }
                Driver::Postgres(sqlx::PgConnection::connect_with(&options).await?)
            }
            Engine::SqlServer => {
                let mut config = Config::new();
                config.host(&profile.host);
                config.port(profile.port);
                config.authentication(AuthMethod::sql_server(&profile.username, password));
                if let Some(database) = database {
                    config.database(database);
                }
                config.encryption(if profile.tls {
                    EncryptionLevel::Required
                } else {
                    EncryptionLevel::NotSupported
                });
                if profile.trust_certificate {
                    config.trust_cert();
                }
                let tcp = if let Some(tunnel) = &tunnel {
                    TcpStream::connect(tunnel.local_addr).await?
                } else {
                    TcpStream::connect(config.get_addr()).await?
                };
                tcp.set_nodelay(true)?;
                Driver::SqlServer(Box::new(Client::connect(config, tcp.compat_write()).await?))
            }
        };
        Ok(Self {
            driver,
            _tunnel: tunnel,
            profile: profile.clone(),
            credentials: credentials.clone(),
            raw_query_used: false,
        })
    }

    pub async fn ping(&mut self) -> Result<()> {
        self.query_internal("SELECT 1 AS connection_test").await?;
        Ok(())
    }

    pub async fn databases(&mut self) -> Result<Vec<String>> {
        let sql = match self.profile.engine {
            // MySQL's catalog identifiers use a binary collation, which SQLx
            // classifies as VARBINARY. Normalize known text columns so names
            // aren't decoded as hex while real binary query values still are.
            Engine::MySql => {
                "SELECT CAST(SCHEMA_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS database_name FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME"
            }
            Engine::Postgres => {
                "SELECT datname::text FROM pg_database WHERE datallowconn AND NOT datistemplate AND has_database_privilege(datname, 'CONNECT') ORDER BY datname"
            }
            Engine::SqlServer => {
                "SELECT name FROM sys.databases WHERE state = 0 AND HAS_DBACCESS(name) = 1 ORDER BY name"
            }
        };
        let output = self.query_internal(sql).await?;
        let mut databases = Vec::new();
        for set in output.sets {
            anyhow::ensure!(
                !set.truncated,
                "数据库列表超过 {ROW_LIMIT} 项，请使用 SQL 浏览完整列表"
            );
            for row in set.rows {
                if let Some(Some(database)) = row.first() {
                    databases.push(database.clone());
                }
            }
        }
        databases.sort();
        databases.dedup();
        Ok(databases)
    }

    pub async fn select_database(&mut self, database: &str) -> Result<()> {
        anyhow::ensure!(
            !database.is_empty() && !database.contains('\0'),
            "数据库名称无效"
        );
        match &mut self.driver {
            Driver::MySql(connection) => {
                let sql = Engine::MySql
                    .select_database_sql(database)
                    .context("切换语句缺失")?;
                // USE isn't supported by MySQL's prepared statement protocol.
                sqlx::raw_sql(&sql).execute(connection).await?;
            }
            Driver::SqlServer(client) => {
                let sql = Engine::SqlServer
                    .select_database_sql(database)
                    .context("切换语句缺失")?;
                client.simple_query(sql).await?.into_results().await?;
            }
            Driver::Postgres(_) => {
                // Replace only after the new connection is ready; failed selection
                // must not destroy the previous session or its SSH tunnel.
                let mut replacement =
                    Self::open(&self.profile, &self.credentials, Some(database)).await?;
                replacement.ping().await?;
                *self = replacement;
            }
        }
        Ok(())
    }

    pub fn selection_error_preserves_session(&self, error: &anyhow::Error) -> bool {
        self.profile.engine == Engine::Postgres
            || matches!(
                error.downcast_ref::<sqlx::Error>(),
                Some(sqlx::Error::Database(_))
            )
            || matches!(
                error.downcast_ref::<tiberius::error::Error>(),
                Some(tiberius::error::Error::Server(_))
            )
    }

    pub async fn current_database(&mut self) -> Result<Option<String>> {
        let sql = match self.profile.engine {
            Engine::MySql => {
                "SELECT CAST(DATABASE() AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS current_database"
            }
            Engine::Postgres => "SELECT current_database()::text AS current_database",
            Engine::SqlServer => "SELECT DB_NAME() AS current_database",
        };
        Ok(self
            .query_internal(sql)
            .await?
            .sets
            .first()
            .and_then(|set| set.rows.first())
            .and_then(|row| row.first())
            .cloned()
            .flatten())
    }

    pub async fn tables(&mut self) -> Result<Vec<Table>> {
        // Queries are fixed strings. Table preview identifiers are escaped per engine.
        let sql = match &self.driver {
            Driver::MySql(_) => {
                "SELECT CAST(TABLE_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS TABLE_SCHEMA, CAST(TABLE_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS TABLE_NAME, CAST(TABLE_TYPE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_SCHEMA, TABLE_NAME"
            }
            Driver::Postgres(_) => {
                "SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY table_schema, table_name"
            }
            Driver::SqlServer(_) => {
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES ORDER BY TABLE_SCHEMA, TABLE_NAME"
            }
        };
        let output = self.query_internal(sql).await?;
        let mut tables = Vec::new();
        for set in output.sets {
            if set.truncated {
                bail!("表目录超过 {ROW_LIMIT} 项，请使用 SQL 查询 information_schema 浏览完整目录");
            }
            for row in set.rows {
                if row.len() >= 3 {
                    tables.push(Table {
                        schema: row[0].clone().context("表目录缺少 schema")?,
                        name: row[1].clone().context("表目录缺少表名")?,
                        kind: row[2].clone().unwrap_or_default(),
                    });
                }
            }
        }
        Ok(tables)
    }

    pub fn raw_session_requires_confirmation(&self) -> bool {
        self.raw_query_used
    }

    pub async fn query(&mut self, sql: &str) -> Result<QueryOutput> {
        // SQLx hides MySQL server transaction status. Never implicitly commit
        // a possible transaction after arbitrary SQL has run on this connection.
        self.raw_query_used = true;
        self.query_internal(sql).await
    }

    pub(crate) async fn query_internal(&mut self, sql: &str) -> Result<QueryOutput> {
        anyhow::ensure!(!sql.trim().is_empty(), "SQL 不能为空");
        let start = Instant::now();
        let mut output = QueryOutput::default();
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut stream = sqlx::raw_sql(sql).fetch_many(connection);
                let mut set = ResultSet::default();
                while let Some(item) = stream.try_next().await? {
                    match item {
                        Either::Left(done) => {
                            set.affected = Some(done.rows_affected());
                            push_set(&mut output, std::mem::take(&mut set))?;
                        }
                        Either::Right(row) => {
                            if set.columns.is_empty() {
                                set.columns =
                                    row.columns().iter().map(|c| c.name().into()).collect();
                                set.column_types = row
                                    .columns()
                                    .iter()
                                    .map(|c| c.type_info().name().into())
                                    .collect();
                            }
                            if set.rows.len() < ROW_LIMIT {
                                set.rows.push(values::mysql(&row));
                            } else {
                                set.truncated = true;
                            }
                        }
                    }
                }
                if !set.columns.is_empty() {
                    push_set(&mut output, set)?;
                }
            }
            Driver::Postgres(connection) => {
                let mut stream = sqlx::raw_sql(sql).fetch_many(connection);
                let mut set = ResultSet::default();
                while let Some(item) = stream.try_next().await? {
                    match item {
                        Either::Left(done) => {
                            set.affected = Some(done.rows_affected());
                            push_set(&mut output, std::mem::take(&mut set))?;
                        }
                        Either::Right(row) => {
                            if set.columns.is_empty() {
                                set.columns =
                                    row.columns().iter().map(|c| c.name().into()).collect();
                                set.column_types = row
                                    .columns()
                                    .iter()
                                    .map(|c| c.type_info().name().into())
                                    .collect();
                            }
                            if set.rows.len() < ROW_LIMIT {
                                set.rows.push(values::postgres(&row));
                            } else {
                                set.truncated = true;
                            }
                        }
                    }
                }
                if !set.columns.is_empty() {
                    push_set(&mut output, set)?;
                }
            }
            Driver::SqlServer(client) => {
                let mut stream = client.simple_query(sql).await?;
                while let Some(item) = stream.try_next().await? {
                    match item {
                        QueryItem::Metadata(metadata) => {
                            push_set(
                                &mut output,
                                ResultSet {
                                    columns: metadata
                                        .columns()
                                        .iter()
                                        .map(|c| c.name().into())
                                        .collect(),
                                    column_types: metadata
                                        .columns()
                                        .iter()
                                        .map(|c| sqlserver_type_name(c.column_type()).into())
                                        .collect(),
                                    ..ResultSet::default()
                                },
                            )?;
                        }
                        QueryItem::Row(row) => {
                            let set = output
                                .sets
                                .last_mut()
                                .context("SQL Server 未返回结果元数据")?;
                            if set.rows.len() < ROW_LIMIT {
                                set.rows.push(values::sqlserver(row));
                            } else {
                                set.truncated = true;
                            }
                        }
                    }
                }
            }
        }
        output.elapsed_ms = start.elapsed().as_millis();
        Ok(output)
    }
}

/// Tiberius exposes wire types without lengths/precision or alias names.
/// Keep ambiguous wire families explicit rather than inventing a SQL type.
fn sqlserver_type_name(column_type: tiberius::ColumnType) -> &'static str {
    use tiberius::ColumnType::*;
    match column_type {
        Null => "NULL",
        Bit | Bitn => "bit",
        Int1 => "tinyint",
        Int2 => "smallint",
        Int4 => "int",
        Int8 => "bigint",
        Datetime4 => "smalldatetime",
        Float4 => "real",
        Float8 => "float",
        Money => "money",
        Datetime => "datetime",
        Money4 => "smallmoney",
        Guid => "uniqueidentifier",
        Decimaln => "decimal",
        Numericn => "numeric",
        Daten => "date",
        Timen => "time",
        Datetime2 => "datetime2",
        DatetimeOffsetn => "datetimeoffset",
        BigVarBin => "varbinary",
        BigVarChar => "varchar",
        BigBinary => "binary",
        BigChar => "char",
        NVarchar => "nvarchar",
        NChar => "nchar",
        Xml => "xml",
        Text => "text",
        Image => "image",
        NText => "ntext",
        SSVariant => "sql_variant",
        Intn => "integer (TDS Intn; size unknown)",
        Floatn => "floating point (TDS Floatn; size unknown)",
        Datetimen => "datetime / smalldatetime (TDS Datetimen)",
        Udt => "user-defined type (TDS Udt)",
    }
}

fn push_set(output: &mut QueryOutput, set: ResultSet) -> Result<()> {
    anyhow::ensure!(
        output.sets.len() < RESULT_LIMIT,
        "结果集超过 {RESULT_LIMIT} 个；请拆分 SQL 执行，之前的语句可能已提交"
    );
    output.sets.push(set);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sqlserver_type_name;
    use tiberius::ColumnType;

    #[test]
    fn sqlserver_headers_use_sql_names_not_wire_variant_names() {
        for (wire, sql) in [
            (ColumnType::Int4, "int"),
            (ColumnType::Int8, "bigint"),
            (ColumnType::Bitn, "bit"),
            (ColumnType::NVarchar, "nvarchar"),
            (ColumnType::BigVarBin, "varbinary"),
            (ColumnType::Guid, "uniqueidentifier"),
            (ColumnType::Decimaln, "decimal"),
            (ColumnType::Datetime2, "datetime2"),
        ] {
            assert_eq!(sqlserver_type_name(wire), sql);
        }
        assert_eq!(
            sqlserver_type_name(ColumnType::Datetimen),
            "datetime / smalldatetime (TDS Datetimen)"
        );
        assert!(sqlserver_type_name(ColumnType::Intn).contains("unknown"));
        assert!(sqlserver_type_name(ColumnType::Floatn).contains("unknown"));
        assert!(sqlserver_type_name(ColumnType::Udt).contains("user-defined"));
    }
}

/// Only ordinary server statement errors are known to preserve the transport.
/// Unknown errors, including an undrained result-set limit, are unsafe.
pub fn query_error_preserves_session(error: &anyhow::Error) -> bool {
    if let Some(sqlx::Error::Database(error)) = error.downcast_ref::<sqlx::Error>() {
        let code = error.code();
        let code = code.as_deref().unwrap_or("");
        if code.starts_with("08") || matches!(code, "57P01" | "57P02" | "57P03") {
            return false;
        }
        if let Some(pg) = error.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
            return !matches!(
                pg.severity(),
                sqlx::postgres::PgSeverity::Fatal | sqlx::postgres::PgSeverity::Panic
            );
        }
        if let Some(mysql) = error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>() {
            return !matches!(mysql.number(), 1053 | 1927 | 2006 | 2013 | 4031);
        }
        return true;
    }
    matches!(error.downcast_ref::<tiberius::error::Error>(),
        Some(tiberius::error::Error::Server(error)) if error.class() < 20)
}
