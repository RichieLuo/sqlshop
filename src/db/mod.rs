mod backup;
mod browser;
mod compare;
mod diagnostics;
mod object_catalog;
mod query_context;
mod query_control;
pub use query_control::{QueryCancelHandle, is_query_cancelled};
mod recovery;
mod security;
pub(crate) use security::AccountChangeFailure;
pub use security::{account_change_preview, security_change_sql};
mod structure;
mod structure_edit;
mod table_edit;
mod table_tree;
pub use browser::{
    ColumnFilter, ColumnSort, FilterOperator, TableBrowseOptions, TablePage, TableStructure,
};
pub use structure_edit::{ColumnEditInfo, StructureChange, column_edit_info, structure_change_sql};
mod values;

pub(crate) use object_catalog::ObjectDefinitionRejected;
pub(crate) use table_edit::EditRejected;

use crate::{
    model::{
        CreateTableColumn, Credentials, Engine, Profile, QueryMessage, QueryOutput, ResultSet,
        Table,
    },
    ssh::Tunnel,
};
use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use sqlx::{
    Column, ConnectOptions, Connection, Either, Row, TypeInfo,
    mysql::{MySqlConnectOptions, MySqlQueryEvent},
    postgres::{PgConnectOptions, PgQueryEvent},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use tiberius::{AuthMethod, Client, Config, DoneKind, EncryptionLevel, QueryEvent};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub const ROW_LIMIT: usize = 1000;
pub const RESULT_LIMIT: usize = 32;

fn validate_identifier(identifier: &str, label: &str) -> Result<()> {
    anyhow::ensure!(!identifier.trim().is_empty(), "{label}不能为空");
    anyhow::ensure!(!identifier.contains('\0'), "{label}包含无效字符");
    Ok(())
}

fn is_system_database(engine: Engine, database: &str) -> bool {
    let system = match engine {
        Engine::MySql => &["mysql", "information_schema", "performance_schema", "sys"][..],
        Engine::Postgres => &["postgres", "template0", "template1"][..],
        Engine::SqlServer => &["master", "model", "msdb", "tempdb"][..],
    };
    system
        .iter()
        .any(|name| name.eq_ignore_ascii_case(database))
}

pub fn column_type_options(engine: Engine) -> &'static [&'static str] {
    match engine {
        Engine::MySql => &[
            "TINYINT",
            "SMALLINT",
            "MEDIUMINT",
            "INT",
            "INTEGER",
            "BIGINT",
            "DECIMAL",
            "NUMERIC",
            "FLOAT",
            "DOUBLE",
            "BOOLEAN",
            "DATE",
            "TIME",
            "DATETIME",
            "TIMESTAMP",
            "CHAR",
            "VARCHAR",
            "TINYTEXT",
            "TEXT",
            "MEDIUMTEXT",
            "LONGTEXT",
            "BINARY",
            "VARBINARY",
            "BLOB",
            "JSON",
        ],
        Engine::Postgres => &[
            "SMALLINT",
            "INTEGER",
            "BIGINT",
            "NUMERIC",
            "REAL",
            "DOUBLE PRECISION",
            "BOOLEAN",
            "DATE",
            "TIME",
            "TIMESTAMP",
            "TIMESTAMPTZ",
            "CHAR",
            "VARCHAR",
            "TEXT",
            "BYTEA",
            "JSON",
            "JSONB",
            "UUID",
        ],
        Engine::SqlServer => &[
            "TINYINT",
            "SMALLINT",
            "INT",
            "BIGINT",
            "DECIMAL",
            "NUMERIC",
            "REAL",
            "FLOAT",
            "BIT",
            "DATE",
            "TIME",
            "DATETIME2",
            "DATETIMEOFFSET",
            "CHAR",
            "VARCHAR",
            "NCHAR",
            "NVARCHAR",
            "TEXT",
            "NTEXT",
            "UNIQUEIDENTIFIER",
            "BINARY",
            "VARBINARY",
            "XML",
        ],
    }
}

fn allowed_column_type(engine: Engine, data_type: &str) -> Result<&'static str> {
    column_type_options(engine)
        .iter()
        .copied()
        .find(|candidate| candidate.eq_ignore_ascii_case(data_type.trim()))
        .ok_or_else(|| anyhow::anyhow!("不支持的数据类型：{}", data_type.trim()))
}

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
    mysql_server_uuid: Option<String>,
}

impl Session {
    pub async fn connect(profile: &Profile, credentials: &Credentials) -> Result<Self> {
        if profile.engine == Engine::SqlServer
            && profile.sqlserver_auth == crate::model::SqlServerAuth::AadToken
            && !profile.database.is_empty()
        {
            return Self::open(profile, credentials, Some(&profile.database)).await;
        }
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
                    .disable_statement_logging()
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
                    .disable_statement_logging()
                    .host(host)
                    .port(port)
                    .username(&profile.username)
                    .password(password)
                    .ssl_mode(mode);
                if let Some(database) = database {
                    options = options.database(database);
                }
                let mut connection = sqlx::PgConnection::connect_with(&options).await?;
                connection.set_server_message_logging(false);
                Driver::Postgres(connection)
            }
            Engine::SqlServer => {
                let mut config = Config::new();
                config.host(&profile.host);
                config.port(profile.port);
                use crate::model::SqlServerAuth;
                config.authentication(match profile.sqlserver_auth {
                    SqlServerAuth::Password => AuthMethod::sql_server(&profile.username, password),
                    SqlServerAuth::AadToken => {
                        anyhow::ensure!(
                            !password.trim().is_empty(),
                            "请填写 SQL Server Entra 访问令牌"
                        );
                        AuthMethod::aad_token(password)
                    }
                    #[cfg(windows)]
                    SqlServerAuth::Integrated => AuthMethod::Integrated,
                    #[cfg(windows)]
                    SqlServerAuth::Windows => AuthMethod::windows(&profile.username, password),
                    #[cfg(not(windows))]
                    SqlServerAuth::Integrated | SqlServerAuth::Windows => {
                        anyhow::bail!("此构建不支持 Windows 集成认证；可使用密码或 Entra 访问令牌")
                    }
                });
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
                    if profile.sqlserver_instance.is_empty() {
                        TcpStream::connect(config.get_addr()).await?
                    } else {
                        let (tcp, port) = crate::instance_discovery::connect_instance(
                            &profile.host,
                            &profile.sqlserver_instance,
                        )
                        .await?;
                        config.port(port);
                        tcp
                    }
                };
                tcp.set_nodelay(true)?;
                let mut client = Client::connect(config, tcp.compat_write()).await?;
                client.set_server_message_logging(false);
                Driver::SqlServer(Box::new(client))
            }
        };
        Ok(Self {
            driver,
            _tunnel: tunnel,
            profile: profile.clone(),
            credentials: credentials.clone(),
            raw_query_used: false,
            mysql_server_uuid: None,
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

    pub async fn create_database(&mut self, database: &str) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(database, "数据库名称")?;
        self.query_internal(&format!(
            "CREATE DATABASE {}",
            self.profile.engine.quote(database)
        ))
        .await?;
        Ok(())
    }

    pub async fn drop_database(&mut self, database: &str) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(database, "数据库名称")?;
        anyhow::ensure!(
            !is_system_database(self.profile.engine, database),
            "系统数据库不能通过此操作删除"
        );
        anyhow::ensure!(
            self.current_database().await?.as_deref() != Some(database),
            "不能删除当前正在使用的数据库；请先切换到其他数据库"
        );
        self.query_internal(&format!(
            "DROP DATABASE {}",
            self.profile.engine.quote(database)
        ))
        .await?;
        Ok(())
    }

    pub async fn create_table(
        &mut self,
        database: &str,
        schema: &str,
        table: &str,
        columns: &[CreateTableColumn],
    ) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(schema, "Schema 名称")?;
        validate_identifier(table, "表名称")?;
        anyhow::ensure!(!columns.is_empty(), "请至少添加一个字段");
        anyhow::ensure!(
            self.current_database().await?.as_deref() == Some(database),
            "当前数据库与新建表的目标数据库不符，请先选择目标数据库"
        );
        let engine = self.profile.engine;
        let mut names = Vec::with_capacity(columns.len());
        let mut primary_keys = Vec::new();
        let mut auto_increment_count = 0;
        let mut definitions = Vec::with_capacity(columns.len() + 1);
        for column in columns {
            validate_identifier(&column.name, "字段名称")?;
            anyhow::ensure!(
                !names
                    .iter()
                    .any(|name: &String| name.eq_ignore_ascii_case(&column.name)),
                "字段名称重复：{}",
                column.name
            );
            names.push(column.name.clone());
            let data_type = allowed_column_type(engine, &column.data_type)?;
            let size = column.size.trim();
            if !size.is_empty() {
                anyhow::ensure!(
                    size.len() <= 16
                        && size
                            .split(',')
                            .all(|part| { part.parse::<u32>().is_ok_and(|value| value > 0) }),
                    "字段长度或精度格式无效：{}",
                    column.name
                );
            }
            let sql_type = if size.is_empty() {
                data_type.to_owned()
            } else {
                format!("{data_type}({size})")
            };
            if column.auto_increment {
                auto_increment_count += 1;
                anyhow::ensure!(
                    ["TINYINT", "SMALLINT", "INT", "INTEGER", "BIGINT"]
                        .iter()
                        .any(|integer| data_type.eq_ignore_ascii_case(integer)),
                    "自增字段必须使用整数类型：{}",
                    column.name
                );
                anyhow::ensure!(
                    column.default_value.trim().is_empty(),
                    "自增字段不能同时设置默认值"
                );
            }
            if column.primary_key {
                primary_keys.push(engine.quote(&column.name));
            }
            let mut definition = format!("{} {sql_type}", engine.quote(&column.name));
            match (engine, column.auto_increment) {
                (Engine::MySql, true) => definition.push_str(" AUTO_INCREMENT"),
                (Engine::Postgres, true) => {
                    definition.push_str(" GENERATED BY DEFAULT AS IDENTITY")
                }
                (Engine::SqlServer, true) => definition.push_str(" IDENTITY(1,1)"),
                _ => {}
            }
            if !column.nullable || column.primary_key || column.auto_increment {
                definition.push_str(" NOT NULL");
            } else {
                definition.push_str(" NULL");
            }
            let default = column.default_value.trim();
            if !default.is_empty() {
                anyhow::ensure!(
                    !default.contains(';') && !default.contains('\0'),
                    "默认值不能包含分号或空字符：{}",
                    column.name
                );
                definition.push_str(" DEFAULT ");
                definition.push_str(default);
            }
            definitions.push(definition);
        }
        anyhow::ensure!(auto_increment_count <= 1, "一张表最多只能有一个自增字段");
        if !primary_keys.is_empty() {
            definitions.push(format!("PRIMARY KEY ({})", primary_keys.join(", ")));
        }
        self.query_internal(&format!(
            "CREATE TABLE {}.{} ({})",
            engine.quote(schema),
            engine.quote(table),
            definitions.join(", ")
        ))
        .await?;
        Ok(())
    }

    pub async fn drop_table(&mut self, database: &str, table: &Table) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(&table.schema, "Schema 名称")?;
        validate_identifier(&table.name, "表名称")?;
        anyhow::ensure!(
            self.current_database().await?.as_deref() == Some(database),
            "当前数据库与删除表的目标数据库不符，请先选择目标数据库"
        );
        let exists_as_table = self.tables().await?.iter().any(|existing| {
            existing.schema == table.schema
                && existing.name == table.name
                && existing.kind.eq_ignore_ascii_case("BASE TABLE")
        });
        anyhow::ensure!(exists_as_table, "目标不存在或不是普通数据表，未执行删除");
        let engine = self.profile.engine;
        self.query_internal(&format!(
            "DROP TABLE {}.{}",
            engine.quote(&table.schema),
            engine.quote(&table.name)
        ))
        .await?;
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
        // User SQL can change relation shapes, search_path, or issue DEALLOCATE
        // ALL. Drop driver-owned plans before that SQL runs, so subsequent
        // context probes cannot reference plans it invalidated. This closes
        // protocol statements only; it does not end the user's transaction.
        self.prepare_user_query().await?;
        self.query_internal(sql).await
    }

    pub(crate) async fn prepare_user_query(&mut self) -> Result<()> {
        match &mut self.driver {
            Driver::MySql(connection) => connection.clear_cached_statements().await?,
            Driver::Postgres(connection) => connection.clear_cached_statements().await?,
            Driver::SqlServer(_) => {}
        }
        Ok(())
    }

    pub(crate) async fn query_internal(&mut self, sql: &str) -> Result<QueryOutput> {
        anyhow::ensure!(!sql.trim().is_empty(), "SQL 不能为空");
        let start = Instant::now();
        let mut output = QueryOutput::default();
        // Observe the original batch protocol. Empty result metadata and server
        // messages must not require preparing, splitting, or re-executing SQL.
        let result: Result<()> = async {
            match &mut self.driver {
                Driver::MySql(connection) => {
                    let events = Arc::new(Mutex::new(QueryEvents::default()));
                    let observed = events.clone();
                    connection.set_query_event_handler(Some(Arc::new(move |event| {
                        let event = match event {
                            MySqlQueryEvent::Metadata(columns) => {
                                ObservedQueryEvent::Metadata(metadata_set(&columns))
                            }
                            MySqlQueryEvent::Completed { warnings, info, .. } => {
                                if warnings == 0 && info.is_empty() {
                                    return;
                                }
                                ObservedQueryEvent::Message(QueryMessage {
                                    severity: if warnings > 0 { "WARNING" } else { "INFO" }.into(),
                                    text: if info.is_empty() {
                                        format!("服务器报告 {warnings} 条警告")
                                    } else {
                                        info
                                    },
                                    ..Default::default()
                                })
                            }
                        };
                        observed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(event);
                    })));
                    let collected: Result<()> = async {
                        let mut set = ResultSet::default();
                        let mut stream = sqlx::raw_sql(sql).fetch_many(&mut *connection);
                        loop {
                            let item = stream.try_next().await;
                            drain_query_events(&events, &mut output, &mut set);
                            match item? {
                                Some(Either::Left(done)) => {
                                    set.affected = Some(done.rows_affected());
                                    push_set(&mut output, std::mem::take(&mut set))?;
                                }
                                Some(Either::Right(row)) => {
                                    if set.columns.is_empty() {
                                        set = metadata_set(row.columns());
                                    }
                                    if set.rows.len() < ROW_LIMIT {
                                        set.rows.push(values::mysql(&row));
                                    } else {
                                        set.truncated = true;
                                    }
                                }
                                None => break,
                            }
                        }
                        Ok(())
                    }
                    .await;
                    connection.set_query_event_handler(None);
                    collected?;
                }
                Driver::Postgres(connection) => {
                    let events = Arc::new(Mutex::new(QueryEvents::default()));
                    let observed = events.clone();
                    connection.set_query_event_handler(Some(Arc::new(move |event| {
                        let event = match event {
                            PgQueryEvent::Metadata(columns) => {
                                ObservedQueryEvent::Metadata(metadata_set(&columns))
                            }
                            PgQueryEvent::Notice(notice) => {
                                ObservedQueryEvent::Message(QueryMessage {
                                    severity: format!("{:?}", notice.severity()).to_uppercase(),
                                    code: Some(notice.code().into()),
                                    text: notice.message().into(),
                                    detail: notice.detail().map(str::to_owned),
                                    hint: notice.hint().map(str::to_owned),
                                    context: notice.r#where().map(str::to_owned),
                                })
                            }
                        };
                        observed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(event);
                    })));
                    let collected: Result<()> = async {
                        let mut set = ResultSet::default();
                        let mut stream = sqlx::raw_sql(sql).fetch_many(&mut *connection);
                        loop {
                            let item = stream.try_next().await;
                            drain_query_events(&events, &mut output, &mut set);
                            match item? {
                                Some(Either::Left(done)) => {
                                    set.affected = Some(done.rows_affected());
                                    push_set(&mut output, std::mem::take(&mut set))?;
                                }
                                Some(Either::Right(row)) => {
                                    if set.columns.is_empty() {
                                        set = metadata_set(row.columns());
                                    }
                                    if set.rows.len() < ROW_LIMIT {
                                        set.rows.push(values::postgres(&row));
                                    } else {
                                        set.truncated = true;
                                    }
                                }
                                None => break,
                            }
                        }
                        Ok(())
                    }
                    .await;
                    connection.set_query_event_handler(None);
                    collected?;
                }
                Driver::SqlServer(client) => {
                    let mut stream = client.simple_query_with_events(sql).await?;
                    let mut current_set: Option<ResultSet> = None;
                    let mut failed_completion = false;
                    while let Some(item) = stream.try_next().await? {
                        match item {
                            QueryEvent::Metadata(metadata) => {
                                // NOCOUNT can omit DONEINPROC. A new metadata token
                                // closes the preceding result in the ordered stream.
                                if let Some(completed) = current_set.take() {
                                    push_set(&mut output, completed)?;
                                }
                                current_set = Some(ResultSet {
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
                                    ..Default::default()
                                });
                            }
                            QueryEvent::Row(row) => {
                                let set = current_set
                                    .as_mut()
                                    .context("SQL Server 未返回结果元数据")?;
                                if set.rows.len() < ROW_LIMIT {
                                    set.rows.push(values::sqlserver(row));
                                } else {
                                    set.truncated = true;
                                }
                            }
                            QueryEvent::Info {
                                number,
                                class,
                                message,
                                procedure,
                                line,
                                state,
                                server,
                            } => {
                                let context = match (procedure.is_empty(), line) {
                                    (false, 0) => Some(format!("过程 {procedure}")),
                                    (false, _) => Some(format!("过程 {procedure} · 第 {line} 行")),
                                    (true, 0) => None,
                                    (true, _) => Some(format!("服务器行号 {line}")),
                                };
                                push_query_message(
                                    &mut output,
                                    QueryMessage {
                                        severity: "INFO".into(),
                                        code: (number != 0).then(|| number.to_string()),
                                        text: message,
                                        detail: Some(if server.is_empty() {
                                            format!("级别 {class} · 状态 {state}")
                                        } else {
                                            format!("级别 {class} · 状态 {state} · 服务器 {server}")
                                        }),
                                        context,
                                        ..Default::default()
                                    },
                                );
                            }
                            QueryEvent::Done {
                                kind,
                                rows_affected,
                                is_error,
                                ..
                            } => {
                                if is_error {
                                    failed_completion = true;
                                    current_set = None;
                                    continue;
                                }
                                if let Some(mut set) = current_set.take() {
                                    set.affected = rows_affected;
                                    push_set(&mut output, set)?;
                                } else if kind != DoneKind::Procedure
                                    && let Some(affected) = rows_affected
                                {
                                    // DONEPROC may summarize earlier DONEINPROC counts.
                                    // Only actual statement completions create new sets.
                                    push_set(
                                        &mut output,
                                        ResultSet {
                                            affected: Some(affected),
                                            ..Default::default()
                                        },
                                    )?;
                                }
                            }
                        }
                    }
                    anyhow::ensure!(
                        !failed_completion,
                        "SQL Server 报告执行失败，未返回详细错误"
                    );
                    // Reaching normal EOF proves the response was drained;
                    // an error EOF exits above and never completes this set.
                    if let Some(completed) = current_set {
                        push_set(&mut output, completed)?;
                    }
                }
            }
            Ok(())
        }
        .await;
        result.map_err(|error| with_partial_output(error, &output, start))?;
        output.elapsed_ms = start.elapsed().as_millis();
        Ok(output)
    }
}

const MESSAGE_LIMIT: usize = 512;

enum ObservedQueryEvent {
    Metadata(ResultSet),
    Message(QueryMessage),
}

#[derive(Default)]
struct QueryEvents {
    events: VecDeque<ObservedQueryEvent>,
    messages: usize,
    omitted: bool,
}

impl QueryEvents {
    fn push(&mut self, event: ObservedQueryEvent) {
        if matches!(&event, ObservedQueryEvent::Message(_)) {
            if self.messages >= MESSAGE_LIMIT {
                self.omitted = true;
                return;
            }
            self.messages += 1;
        }
        self.events.push_back(event);
    }
}

fn metadata_set<C: Column>(columns: &[C]) -> ResultSet {
    ResultSet {
        columns: columns.iter().map(|c| c.name().into()).collect(),
        column_types: columns
            .iter()
            .map(|c| c.type_info().name().into())
            .collect(),
        ..Default::default()
    }
}

fn push_query_message(output: &mut QueryOutput, message: QueryMessage) {
    if output.messages.len() < MESSAGE_LIMIT {
        output.messages.push(message);
    } else if output.messages.len() == MESSAGE_LIMIT {
        output.messages.push(omitted_messages_notice());
    }
}

fn omitted_messages_notice() -> QueryMessage {
    QueryMessage {
        severity: "显示限制".into(),
        text: format!("服务器消息达到 {MESSAGE_LIMIT} 条显示上限，其余消息未显示。"),
        ..Default::default()
    }
}

fn drain_query_events(events: &Mutex<QueryEvents>, output: &mut QueryOutput, set: &mut ResultSet) {
    let mut events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for event in events.events.drain(..) {
        match event {
            ObservedQueryEvent::Metadata(metadata) => *set = metadata,
            ObservedQueryEvent::Message(message) => push_query_message(output, message),
        }
    }
    if events.omitted && output.messages.len() == MESSAGE_LIMIT {
        output.messages.push(omitted_messages_notice());
    }
}

#[derive(Debug)]
struct QueryPartialContext(QueryOutput);

impl std::fmt::Display for QueryPartialContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "此前已接收 {} 个完整结果、{} 条消息",
            self.0.sets.len(),
            self.0.messages.len()
        )
    }
}

fn with_partial_output(
    error: anyhow::Error,
    completed: &QueryOutput,
    started: Instant,
) -> anyhow::Error {
    if completed.sets.is_empty() && completed.messages.is_empty() {
        error
    } else {
        let mut output = completed.clone();
        output.elapsed_ms = started.elapsed().as_millis();
        error.context(QueryPartialContext(output))
    }
}

/// A batch error can retain completed row sets without losing the original
/// driver error used to classify transport/session safety and error positions.
pub fn query_partial_output(error: &anyhow::Error) -> Option<QueryOutput> {
    error
        .downcast_ref::<QueryPartialContext>()
        .map(|partial| partial.0.clone())
}

pub(crate) fn retain_query_output(error: anyhow::Error, output: QueryOutput) -> anyhow::Error {
    error.context(QueryPartialContext(output))
}

fn single_statement_keyword(sql: &str, engine: Engine) -> Option<&str> {
    let Ok(Some(first)) = crate::sql_editor::current_statement_range(sql, 0, engine) else {
        return None;
    };
    let Ok(Some(last)) =
        crate::sql_editor::current_statement_range(sql, sql.chars().count(), engine)
    else {
        return None;
    };
    if first != last {
        return None;
    }
    let mut text = sql[first].trim_start();
    loop {
        if text.starts_with("--")
            && (engine != Engine::MySql
                || text.as_bytes().get(2).is_none_or(u8::is_ascii_whitespace))
            || (engine == Engine::MySql && text.starts_with('#'))
        {
            let end = text.find(['\n', '\r'])?;
            text = text[end..].trim_start();
        } else if text.starts_with("/*") {
            let mut depth = 1usize;
            let mut offset = 2usize;
            while offset < text.len() && depth > 0 {
                if text[offset..].starts_with("/*") && engine != Engine::MySql {
                    depth += 1;
                    offset += 2;
                } else if text[offset..].starts_with("*/") {
                    depth -= 1;
                    offset += 2;
                } else {
                    offset += text[offset..].chars().next().unwrap().len_utf8();
                }
            }
            if depth > 0 {
                return None;
            }
            text = text[offset..].trim_start();
        } else {
            break;
        }
    }
    let word_end = text
        .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .unwrap_or(text.len());
    Some(&text[..word_end])
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

/// Retain driver diagnostics before an anyhow error is converted to display text.
/// PostgreSQL's internal-query positions and stored-procedure SQL Server lines
/// are not positions in the user's editor and deliberately remain non-navigable.
pub fn query_diagnostic(error: &anyhow::Error, sql: &str) -> crate::model::QueryDiagnostic {
    use crate::model::{QueryDiagnostic, QueryErrorPosition};
    let mut diagnostic = QueryDiagnostic::default();
    if let Some(sqlx::Error::Database(database)) = error.downcast_ref::<sqlx::Error>() {
        diagnostic.code = database.code().map(|code| code.into_owned());
        if let Some(pg) = database.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
            diagnostic.detail = pg.detail().map(str::to_owned);
            diagnostic.hint = pg.hint().map(str::to_owned);
            diagnostic.context = pg.r#where().map(str::to_owned);
            if let Some(sqlx::postgres::PgErrorPosition::Original(position)) = pg.position()
                && let Some(offset) = position.checked_sub(1)
                && offset <= sql.chars().count()
            {
                let mut line = 1;
                let mut column = 1;
                for ch in sql.chars().take(offset) {
                    if ch == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                }
                diagnostic.position = Some(QueryErrorPosition {
                    line,
                    column: Some(column),
                    char_offset: offset,
                });
            }
        } else if let Some(mysql) = database.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>() {
            diagnostic.code = Some(match mysql.code() {
                Some(state) => format!("{} / {state}", mysql.number()),
                None => mysql.number().to_string(),
            });
        }
    } else if let Some(tiberius::error::Error::Server(server)) =
        error.downcast_ref::<tiberius::error::Error>()
    {
        diagnostic.code = Some(server.code().to_string());
        if server.line() > 0 {
            diagnostic.context = Some(if server.procedure().is_empty() {
                format!("服务器报告第 {} 行", server.line())
            } else {
                format!(
                    "过程 {}，服务器报告第 {} 行",
                    server.procedure(),
                    server.line()
                )
            });
        }
        let direct_statement =
            single_statement_keyword(sql, Engine::SqlServer).is_some_and(|keyword| {
                ["SELECT", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE"]
                    .iter()
                    .any(|direct| keyword.eq_ignore_ascii_case(direct))
            });
        if direct_statement && server.procedure().is_empty() && server.line() > 0 {
            let line = server.line() as usize;
            let offset = if line == 1 {
                Some(0)
            } else {
                sql.chars()
                    .enumerate()
                    .filter(|(_, ch)| *ch == '\n')
                    .nth(line - 2)
                    .map(|(offset, _)| offset + 1)
            };
            if let Some(char_offset) = offset {
                diagnostic.position = Some(QueryErrorPosition {
                    line,
                    column: None,
                    char_offset,
                });
            }
        }
    }
    diagnostic
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
    matches!(
        error.downcast_ref::<tiberius::error::Error>(),
        Some(tiberius::error::Error::Cancelled)
    ) || matches!(error.downcast_ref::<tiberius::error::Error>(),
            Some(tiberius::error::Error::Server(error)) if error.class() < 20)
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
