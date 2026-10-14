use super::{EditRejected, ROW_LIMIT, Session, validate_identifier};
use crate::{
    model::{Engine, QueryOutput},
    security::{
        AccessPrincipal, AccountChange, AccountPassword, PrincipalKind, SecurityCatalog,
        SecurityChange,
    },
};
use anyhow::Result;

fn rejected(message: impl Into<String>) -> anyhow::Error {
    EditRejected(message.into()).into()
}

#[derive(Debug)]
pub(crate) struct AccountChangeFailure {
    pub message: String,
    pub outcome_unknown: bool,
}
impl std::fmt::Display for AccountChangeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
// No original source: driver error text and partial output can echo passwords.
impl std::error::Error for AccountChangeFailure {}

fn password_literal(
    engine: Engine,
    password: &AccountPassword,
    redacted: bool,
    no_backslash_escapes: bool,
) -> Result<String> {
    let value = password.expose();
    if engine == Engine::Postgres
        && ((value.len() == 35
            && value.starts_with("md5")
            && value[3..].bytes().all(|byte| byte.is_ascii_hexdigit()))
            || value.starts_with("SCRAM-SHA-256$"))
    {
        return Err(rejected(
            "此密码文本会被 PostgreSQL 识别为密码验证串；请使用不含验证串前缀的普通密码。",
        ));
    }
    if value.is_empty()
        || value.contains('\0')
        || value.len() > 16384
        || (engine == Engine::SqlServer && value.encode_utf16().count() > 128)
    {
        return Err(rejected(
            "请填写有效密码；SQL Server 密码最长 128 个字符，其他密码最长 16384 字节，且不能包含空字符。",
        ));
    }
    if redacted {
        return Ok(if engine == Engine::SqlServer {
            "N'<密码已隐藏>'"
        } else {
            "'<密码已隐藏>'"
        }
        .into());
    }
    let mut value = value.replace('\'', "''");
    match engine {
        Engine::Postgres => {
            value = value.replace('\\', "\\\\");
            Ok(format!("E'{value}'"))
        }
        Engine::MySql => {
            if !no_backslash_escapes {
                value = value.replace('\\', "\\\\");
            }
            Ok(format!("'{value}'"))
        }
        Engine::SqlServer => Ok(format!("N'{value}'")),
    }
}

fn new_account(
    engine: Engine,
    name: &str,
    host: Option<&str>,
    kind: PrincipalKind,
) -> Result<AccessPrincipal> {
    let principal = AccessPrincipal {
        name: name.into(),
        host: host.map(str::to_owned),
        kind,
        enabled: None,
        default_schema: None,
        identity: None,
    };
    principal_sql(engine, &principal)?;
    Ok(principal)
}

fn account_sql(
    engine: Engine,
    change: &AccountChange,
    redacted: bool,
    no_backslash_escapes: bool,
) -> Result<String> {
    Ok(match change {
        AccountChange::CreateUser {
            name,
            host,
            password,
        } => {
            let principal = new_account(engine, name, host.as_deref(), PrincipalKind::User)?;
            match engine {
                Engine::Postgres => format!(
                    "CREATE ROLE {} LOGIN PASSWORD {};",
                    principal_sql(engine, &principal)?,
                    password_literal(engine, password, redacted, no_backslash_escapes)?
                ),
                Engine::MySql => format!(
                    "CREATE USER {} IDENTIFIED BY {};",
                    principal_sql(engine, &principal)?,
                    password_literal(engine, password, redacted, no_backslash_escapes)?
                ),
                Engine::SqlServer => {
                    return Err(rejected(
                        "SQL Server 请先创建服务器 SQL 登录，再创建数据库用户映射。",
                    ));
                }
            }
        }
        AccountChange::CreateLogin { name, password } => {
            if engine != Engine::SqlServer {
                return Err(rejected("服务器登录创建仅适用于 SQL Server。"));
            }
            check_name(engine, name)?;
            format!(
                "CREATE LOGIN {} WITH PASSWORD = {};",
                engine.quote(name),
                password_literal(engine, password, redacted, no_backslash_escapes)?
            )
        }
        AccountChange::ChangePassword {
            principal,
            password,
            old_password,
        } => {
            let account = principal_sql(engine, principal)?;
            let literal = password_literal(engine, password, redacted, no_backslash_escapes)?;
            match engine {
                Engine::Postgres => format!("ALTER ROLE {account} PASSWORD {literal};"),
                Engine::MySql => {
                    if principal.identity.as_ref().is_some_and(|identity| {
                        ![
                            "mysql_native_password:",
                            "sha256_password:",
                            "caching_sha2_password:",
                        ]
                        .iter()
                        .any(|plugin| identity.starts_with(plugin))
                    }) {
                        return Err(rejected(
                            "此 MySQL 账号使用外部或未支持的认证插件，当前流程不能修改其密码。",
                        ));
                    }
                    format!("SET PASSWORD FOR {account} = {literal};")
                }
                Engine::SqlServer => {
                    if principal.kind != PrincipalKind::Login
                        || principal
                            .identity
                            .as_ref()
                            .is_some_and(|id| !id.starts_with("login:S:"))
                    {
                        return Err(rejected(
                            "密码修改仅支持 SQL Server SQL 登录；其他身份的密码由其认证提供方管理。",
                        ));
                    }
                    let old = match old_password {
                        Some(old) => format!(
                            " OLD_PASSWORD = {}",
                            password_literal(engine, old, redacted, no_backslash_escapes)?
                        ),
                        None => String::new(),
                    };
                    format!("ALTER LOGIN {account} WITH PASSWORD = {literal}{old};")
                }
            }
        }
        AccountChange::SetLogin { principal, enabled } => {
            let account = principal_sql(engine, principal)?;
            match engine {
                Engine::Postgres => format!(
                    "ALTER ROLE {account} {};",
                    if *enabled { "LOGIN" } else { "NOLOGIN" }
                ),
                Engine::MySql => format!(
                    "ALTER USER {account} ACCOUNT {};",
                    if *enabled { "UNLOCK" } else { "LOCK" }
                ),
                Engine::SqlServer => {
                    if principal.kind != PrincipalKind::Login {
                        return Err(rejected(
                            "SQL Server 登录启禁操作需选择服务器登录；数据库用户不能冒充登录。",
                        ));
                    }
                    format!(
                        "ALTER LOGIN {account} {};",
                        if *enabled { "ENABLE" } else { "DISABLE" }
                    )
                }
            }
        }
        AccountChange::DropUser { principal } => {
            if engine == Engine::SqlServer && principal.kind != PrincipalKind::User {
                return Err(rejected(
                    "当前流程只删除 SQL Server 数据库用户，不删除服务器登录或角色。",
                ));
            }
            if engine != Engine::SqlServer {
                check_mutable_role(engine, principal)?;
            }
            format!(
                "DROP {} {};",
                if engine == Engine::Postgres {
                    "ROLE"
                } else {
                    "USER"
                },
                principal_sql(engine, principal)?
            )
        }
        AccountChange::CreateDatabaseUser {
            name,
            login,
            default_schema,
        } => {
            if engine != Engine::SqlServer || login.kind != PrincipalKind::Login {
                return Err(rejected("数据库用户映射需要选择 SQL Server 服务器登录。"));
            }
            check_name(engine, name)?;
            principal_sql(engine, login)?;
            let schema = match default_schema
                .as_deref()
                .filter(|schema| !schema.is_empty())
            {
                Some(schema) => {
                    check_name(engine, schema)?;
                    format!(" WITH DEFAULT_SCHEMA = {}", engine.quote(schema))
                }
                None => String::new(),
            };
            format!(
                "CREATE USER {} FOR LOGIN {}{schema};",
                engine.quote(name),
                engine.quote(&login.name)
            )
        }
    })
}

/// Review text only: a real password can never appear in this result.
pub fn account_change_preview(engine: Engine, change: &AccountChange) -> Result<String> {
    account_sql(engine, change, true, false)
}

fn check_name(engine: Engine, name: &str) -> Result<()> {
    validate_identifier(name, "账号或角色名称").map_err(|error| rejected(error.to_string()))?;
    let valid = match engine {
        Engine::MySql => name.chars().count() <= 32 && !name.contains('\\'),
        Engine::Postgres => name.len() <= 63,
        Engine::SqlServer => name.encode_utf16().count() <= 128,
    };
    if !valid {
        return Err(rejected(
            "账号或角色名称超过数据库限制，或包含当前账号管理不支持的反斜杠。",
        ));
    }
    Ok(())
}

fn principal_sql(engine: Engine, principal: &AccessPrincipal) -> Result<String> {
    check_name(engine, &principal.name)?;
    match engine {
        Engine::MySql => {
            let host = principal
                .host
                .as_deref()
                .ok_or_else(|| rejected("MySQL 账号缺少 Host，无法安全定位"))?;
            if host.is_empty() || host.len() > 255 || host.contains(['\0', '\\']) {
                return Err(rejected("MySQL Host 无效或包含当前账号管理不支持的反斜杠"));
            }
            Ok(format!(
                "'{}'@'{}'",
                principal.name.replace('\'', "''"),
                host.replace('\'', "''")
            ))
        }
        Engine::Postgres | Engine::SqlServer => {
            if principal.host.is_some() {
                return Err(rejected("此数据库的账号身份不含 Host"));
            }
            Ok(engine.quote(&principal.name))
        }
    }
}

fn check_database_principal(engine: Engine, principal: &AccessPrincipal) -> Result<()> {
    principal_sql(engine, principal)?;
    if engine == Engine::SqlServer && principal.kind == PrincipalKind::Login {
        return Err(rejected(
            "此操作仅管理当前数据库用户和角色；服务器登录需要独立的服务器权限流程。",
        ));
    }
    Ok(())
}

fn check_role(engine: Engine, role: &AccessPrincipal) -> Result<()> {
    check_database_principal(engine, role)?;
    // PostgreSQL roles with LOGIN are still roles. MySQL explicitly permits
    // any account to act as a role; it has no reliable 'is_role' catalog flag.
    if engine == Engine::SqlServer && role.kind != PrincipalKind::Role {
        return Err(rejected(
            "请选择当前数据库中的角色，不能把用户或服务器登录当作角色删除。",
        ));
    }
    Ok(())
}

fn check_mutable_role(engine: Engine, role: &AccessPrincipal) -> Result<()> {
    check_role(engine, role)?;
    let system = match engine {
        Engine::MySql => {
            ["mysql.sys", "mysql.session", "mysql.infoschema", "root"].contains(&role.name.as_str())
        }
        Engine::Postgres => role.name == "public" || role.name.starts_with("pg_"),
        Engine::SqlServer => [
            "public",
            "db_owner",
            "db_accessadmin",
            "db_securityadmin",
            "db_ddladmin",
            "db_backupoperator",
            "db_datareader",
            "db_datawriter",
            "db_denydatareader",
            "db_denydatawriter",
        ]
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&role.name)),
    };
    if system {
        return Err(rejected("内置角色或系统账号不能通过此角色变更流程修改。"));
    }
    Ok(())
}

/// Produces exactly one typed, quoted statement for a reviewable permission
/// change. No caller-provided SQL fragment, password, or GRANT OPTION is accepted.
pub fn security_change_sql(engine: Engine, change: &SecurityChange) -> Result<String> {
    match change {
        SecurityChange::CreateRole { name } => {
            check_name(engine, name)?;
            let role = AccessPrincipal {
                name: name.clone(),
                host: (engine == Engine::MySql).then(|| "%".into()),
                kind: PrincipalKind::Role,
                enabled: None,
                default_schema: None,
                identity: None,
            };
            check_mutable_role(engine, &role)?;
            Ok(format!("CREATE ROLE {};", principal_sql(engine, &role)?))
        }
        SecurityChange::DropRole { role } => {
            check_mutable_role(engine, role)?;
            Ok(format!("DROP ROLE {};", principal_sql(engine, role)?))
        }
        SecurityChange::GrantRole { role, member }
        | SecurityChange::RevokeRole { role, member } => {
            check_role(engine, role)?;
            check_database_principal(engine, member)?;
            if role.name == member.name && role.host == member.host {
                return Err(rejected("角色不能授予给自身。"));
            }
            let grant = matches!(change, SecurityChange::GrantRole { .. });
            let role = principal_sql(engine, role)?;
            let member = principal_sql(engine, member)?;
            Ok(if engine == Engine::SqlServer {
                format!(
                    "ALTER ROLE {role} {} MEMBER {member};",
                    if grant { "ADD" } else { "DROP" }
                )
            } else {
                format!(
                    "{} {role} {} {member};",
                    if grant { "GRANT" } else { "REVOKE" },
                    if grant { "TO" } else { "FROM" }
                )
            })
        }
        SecurityChange::GrantTablePrivilege {
            principal,
            table,
            privilege,
        }
        | SecurityChange::RevokeTablePrivilege {
            principal,
            table,
            privilege,
        } => {
            check_database_principal(engine, principal)?;
            for (value, label) in [(&table.schema, "Schema 名称"), (&table.name, "表名称")] {
                validate_identifier(value, label).map_err(|error| rejected(error.to_string()))?;
            }
            let grant = matches!(change, SecurityChange::GrantTablePrivilege { .. });
            Ok(format!(
                "{} {} ON {}{}.{} {} {};",
                if grant { "GRANT" } else { "REVOKE" },
                privilege.sql(),
                if engine == Engine::SqlServer {
                    "OBJECT::"
                } else if engine == Engine::Postgres {
                    "TABLE "
                } else {
                    ""
                },
                engine.quote(&table.schema),
                engine.quote(&table.name),
                if grant { "TO" } else { "FROM" },
                principal_sql(engine, principal)?
            ))
        }
    }
}

impl Session {
    pub async fn apply_account_change(
        &mut self,
        change: &AccountChange,
        expected_preview: &str,
    ) -> Result<()> {
        if account_change_preview(self.profile.engine, change)? != expected_preview {
            return Err(rejected("账号变更与密码已遮盖的预览不符，请重新预览。"));
        }
        self.ensure_edit_transaction_safe().await?;
        let engine = self.profile.engine;
        match change {
            AccountChange::CreateUser { name, host, .. } => {
                let principal = new_account(engine, name, host.as_deref(), PrincipalKind::User)?;
                if self.account_snapshot(&principal).await?.is_some() {
                    return Err(rejected("目标账号已存在，请刷新后重新预览。"));
                }
            }
            AccountChange::CreateLogin { name, .. } => {
                let principal = new_account(engine, name, None, PrincipalKind::Login)?;
                if self.account_snapshot(&principal).await?.is_some() {
                    return Err(rejected("服务器登录已存在，请刷新后重新预览。"));
                }
            }
            AccountChange::CreateDatabaseUser {
                name,
                login,
                default_schema,
            } => {
                self.verify_account_snapshot(login).await?;
                let user = new_account(engine, name, None, PrincipalKind::User)?;
                if self.account_snapshot(&user).await?.is_some() {
                    return Err(rejected("数据库用户已存在，请刷新后重新预览。"));
                }
                if let Some(schema) = default_schema
                    .as_deref()
                    .filter(|schema| !schema.is_empty())
                {
                    let found = self
                        .parameter_query("SELECT name FROM sys.schemas WHERE name=@P1", &[schema])
                        .await?;
                    if found.sets.first().is_none_or(|set| set.rows.len() != 1) {
                        return Err(rejected("默认 Schema 不存在或当前账号无法查看，请刷新。"));
                    }
                }
            }
            AccountChange::ChangePassword { principal, .. }
            | AccountChange::SetLogin { principal, .. }
            | AccountChange::DropUser { principal } => {
                self.verify_account_snapshot(principal).await?;
                if (engine == Engine::Postgres && principal.name.starts_with("pg_"))
                    || (engine == Engine::MySql
                        && ["mysql.sys", "mysql.session", "mysql.infoschema"]
                            .contains(&principal.name.as_str()))
                {
                    return Err(rejected("系统账号不能通过此基础账号管理流程修改。"));
                }
                if matches!(
                    change,
                    AccountChange::DropUser { .. } | AccountChange::SetLogin { enabled: false, .. }
                ) {
                    let sql = match engine {
                        Engine::Postgres => "SELECT current_user::text",
                        Engine::MySql => {
                            "SELECT CAST(CURRENT_USER() AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci"
                        }
                        Engine::SqlServer => {
                            if principal.kind == PrincipalKind::Login {
                                "SELECT SUSER_SNAME()"
                            } else {
                                "SELECT USER_NAME()"
                            }
                        }
                    };
                    let current = self.query_internal(sql).await?;
                    let current = current
                        .sets
                        .first()
                        .and_then(|set| set.rows.first())
                        .and_then(|row| cell(row, 0))
                        .ok_or_else(|| rejected("无法核实当前登录身份，操作未执行。"))?;
                    if current == principal.display_name() {
                        return Err(rejected(
                            "当前执行账号不能通过此流程删除或禁用登录，请使用另一个管理账号。",
                        ));
                    }
                }
                if engine == Engine::SqlServer
                    && matches!(change, AccountChange::DropUser { .. })
                    && ["dbo", "guest", "sys", "INFORMATION_SCHEMA"]
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&principal.name))
                {
                    return Err(rejected("内置数据库用户不能删除。"));
                }
            }
        }
        let no_backslash_escapes = if engine == Engine::MySql {
            let mode = self
                .query_internal("SELECT CAST(@@session.sql_mode AS CHAR)")
                .await?;
            let mode = mode
                .sets
                .first()
                .and_then(|set| set.rows.first())
                .and_then(|row| cell(row, 0))
                .ok_or_else(|| rejected("无法读取 MySQL 字符串模式，账号变更未执行。"))?;
            mode.split(',')
                .any(|mode| mode.trim() == "NO_BACKSLASH_ESCAPES")
        } else {
            false
        };
        // Actual secret-bearing SQL exists only at the execution boundary. It
        // must never be returned, persisted, attached as context or logged.
        let sql = account_sql(engine, change, false, no_backslash_escapes)?;
        match self.query_internal(&sql).await {
            Ok(_) => Ok(()),
            Err(error) => {
                let outcome_unknown = !super::query_error_preserves_session(&error);
                let code = super::query_diagnostic(&error, "")
                    .code
                    .unwrap_or_default()
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '/'))
                    .take(40)
                    .collect::<String>();
                let message = if outcome_unknown {
                    "账号变更未收到成功确认；连接已丢弃，请重新读取账号状态核实结果。".into()
                } else {
                    format!(
                        "数据库拒绝账号变更{}；请检查权限、服务器密码策略、账号类型与依赖。",
                        if code.is_empty() {
                            String::new()
                        } else {
                            format!("（错误代码 {code}）")
                        }
                    )
                };
                Err(AccountChangeFailure {
                    message,
                    outcome_unknown,
                }
                .into())
            }
        }
    }

    async fn verify_account_snapshot(&mut self, expected: &AccessPrincipal) -> Result<()> {
        if expected.identity.is_none() {
            return Err(rejected("账号快照缺少目录身份，请重新读取账号列表。"));
        }
        let actual = self
            .account_snapshot(expected)
            .await?
            .ok_or_else(|| rejected("账号不存在或当前账号无法读取，请刷新后重新预览。"))?;
        if actual.identity != expected.identity
            || actual.enabled != expected.enabled
            || actual.default_schema != expected.default_schema
            || (self.profile.engine != Engine::MySql && actual.kind != expected.kind)
        {
            return Err(rejected("账号身份或登录属性已变化，请刷新后重新预览。"));
        }
        Ok(())
    }

    async fn account_snapshot(
        &mut self,
        principal: &AccessPrincipal,
    ) -> Result<Option<AccessPrincipal>> {
        principal_sql(self.profile.engine, principal)?;
        let (sql, parameters): (&str, Vec<&str>) = match self.profile.engine {
            Engine::MySql => (
                "SELECT CAST(User AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,CAST(Host AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,'user',CASE WHEN account_locked='Y' THEN 'false' ELSE 'true' END,NULL,CONCAT(CAST(plugin AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,':',COALESCE(CAST(password_last_changed AS CHAR),'unknown')) FROM mysql.user WHERE User=? AND Host=?",
                vec![&principal.name, principal.host.as_deref().unwrap()],
            ),
            Engine::Postgres => (
                "SELECT rolname::text,NULL,CASE WHEN rolcanlogin THEN 'user' ELSE 'role' END,rolcanlogin::text,NULL,oid::text FROM pg_roles WHERE rolname=$1",
                vec![&principal.name],
            ),
            Engine::SqlServer => {
                if principal.kind == PrincipalKind::Login {
                    (
                        "SELECT name,NULL,'login',CASE WHEN is_disabled=1 THEN 'false' ELSE 'true' END,NULL,'login:'+type+':'+CONVERT(nvarchar(20),principal_id)+':'+CONVERT(nvarchar(33),create_date,126) FROM sys.server_principals WHERE name=@P1",
                        vec![&principal.name],
                    )
                } else {
                    (
                        "SELECT name,NULL,CASE WHEN type='R' THEN 'role' ELSE 'user' END,NULL,default_schema_name,'user:'+type+':'+CONVERT(nvarchar(20),principal_id)+':'+CONVERT(nvarchar(33),create_date,126) FROM sys.database_principals WHERE name=@P1",
                        vec![&principal.name],
                    )
                }
            }
        };
        let output = self.parameter_query(sql, &parameters).await?;
        let mut catalog = SecurityCatalog::default();
        append_principals(&mut catalog, output, self.profile.engine, "账号快照");
        if catalog.principals.len() > 1 {
            return Err(rejected("账号目录身份不唯一，操作未执行。"));
        }
        Ok(catalog.principals.pop())
    }

    pub async fn security_principals(&mut self) -> Result<SecurityCatalog> {
        let sql = match self.profile.engine {
            Engine::MySql => MYSQL_PRINCIPALS,
            Engine::Postgres => PG_PRINCIPALS,
            Engine::SqlServer => MSSQL_PRINCIPALS,
        };
        let output = self.query_internal(sql).await?;
        let mut catalog = SecurityCatalog::default();
        append_principals(
            &mut catalog,
            output,
            self.profile.engine,
            "账号与数据库角色",
        );
        if self.profile.engine == Engine::MySql {
            // Role edge sources identify accounts currently used as roles. A
            // newly-created role with no edges is indistinguishable from an
            // ordinary locked account, so do not guess from password/lock flags.
            match self.query_internal("SELECT CAST(FROM_USER AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,CAST(FROM_HOST AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci FROM mysql.role_edges GROUP BY FROM_USER,FROM_HOST ORDER BY FROM_USER,FROM_HOST LIMIT 1001").await {
                Ok(output) => {
                    for set in output.sets {
                        if set.truncated { catalog.truncated=true;catalog.warnings.push(format!("角色关系超过 {ROW_LIMIT} 项，仅标识当前已显示的角色。")); }
                        for row in set.rows {
                            if let (Some(name),Some(host))=(cell(&row,0),cell(&row,1)) && let Some(principal)=catalog.principals.iter_mut().find(|principal|principal.name==name && principal.host.as_deref()==Some(host)) {
                                principal.kind=PrincipalKind::Role;
                            }
                        }
                    }
                }
                Err(error) if super::query_error_preserves_session(&error) => catalog.warnings.push(format!("角色关系不可读取（服务器版本或权限限制）：{error:#}")),
                Err(error)=>return Err(error),
            }
            catalog.warnings.push("MySQL 账号可兼作角色；没有成员关系的角色无法从目录可靠区分，显示为用户。角色删除会删除对应账号，必须核对 SQL 预览中的 User 与 Host。".into());
        } else if self.profile.engine == Engine::SqlServer {
            match self.query_internal(MSSQL_LOGINS).await {
                Ok(output) => {
                    append_principals(&mut catalog, output, self.profile.engine, "服务器登录")
                }
                Err(error) if super::query_error_preserves_session(&error) => catalog
                    .warnings
                    .push(format!("服务器登录目录不可读取：{error:#}")),
                Err(error) => return Err(error),
            }
        }
        Ok(catalog)
    }

    /// Direct grants and role memberships are separate result sets. They are
    /// not represented as a complete calculation of effective permissions.
    pub async fn security_grants(&mut self, principal: &AccessPrincipal) -> Result<QueryOutput> {
        let account = principal_sql(self.profile.engine, principal)?;
        let mut output = match self.profile.engine {
            Engine::MySql => {
                let mut output = self
                    .query_internal(&format!("SHOW GRANTS FOR {account}"))
                    .await?;
                if output
                    .sets
                    .iter()
                    .flat_map(|set| &set.rows)
                    .flat_map(|row| row.iter().flatten())
                    .any(|value| value.to_ascii_uppercase().contains(" IDENTIFIED "))
                {
                    return Err(rejected(
                        "此服务器的 SHOW GRANTS 包含认证数据，不能在当前无凭据权限视图中展示；请使用支持独立 SHOW CREATE USER 的 MySQL 版本。",
                    ));
                }
                let host = principal.host.as_deref().unwrap();
                match self
                    .parameter_query(
                        MYSQL_MEMBERSHIPS,
                        &[
                            &principal.name,
                            host,
                            &principal.name,
                            host,
                            &principal.name,
                            host,
                        ],
                    )
                    .await
                {
                    Ok(mut membership) => {
                        set_names(
                            &mut membership,
                            &[
                                "relationship",
                                "role",
                                "role_host",
                                "member",
                                "member_host",
                                "admin_option",
                                "default_role",
                            ],
                        );
                        output.sets.extend(membership.sets);
                        output.elapsed_ms += membership.elapsed_ms;
                    }
                    Err(error) if super::query_error_preserves_session(&error) => {
                        output.messages.push(crate::model::QueryMessage {
                            severity: "目录范围".into(),
                            text: format!("角色成员关系不可读取：{error:#}"),
                            ..Default::default()
                        })
                    }
                    Err(error) => return Err(error),
                }
                output
            }
            Engine::Postgres => {
                let mut output = self
                    .parameter_query(PG_MEMBERSHIPS, &[&principal.name])
                    .await?;
                set_names(
                    &mut output,
                    &["relationship", "role", "member", "admin_option"],
                );
                let mut privileges = self.parameter_query(PG_GRANTS, &[&principal.name]).await?;
                set_names(
                    &mut privileges,
                    &[
                        "class",
                        "grantor",
                        "grantee",
                        "schema",
                        "object",
                        "privilege",
                        "grantable",
                    ],
                );
                output.sets.extend(privileges.sets);
                output.elapsed_ms += privileges.elapsed_ms;
                let mut attributes=self.parameter_query("SELECT rolcanlogin::text,rolsuper::text,rolcreatedb::text,rolcreaterole::text,rolinherit::text,rolreplication::text,rolbypassrls::text,rolvaliduntil::text FROM pg_roles WHERE rolname=$1",&[&principal.name]).await?;
                set_names(
                    &mut attributes,
                    &[
                        "login",
                        "superuser",
                        "create_database",
                        "create_role",
                        "inherit",
                        "replication",
                        "bypass_rls",
                        "login_valid_until",
                    ],
                );
                output.sets.extend(attributes.sets);
                output.elapsed_ms += attributes.elapsed_ms;
                output
            }
            Engine::SqlServer => {
                let (memberships, grants) = if principal.kind == PrincipalKind::Login {
                    (MSSQL_SERVER_MEMBERSHIPS, MSSQL_SERVER_GRANTS)
                } else {
                    (MSSQL_MEMBERSHIPS, MSSQL_GRANTS)
                };
                let mut output = self
                    .parameter_query(memberships, &[&principal.name])
                    .await?;
                set_names(&mut output, &["relationship", "role", "member"]);
                let mut privileges = self.parameter_query(grants, &[&principal.name]).await?;
                set_names(
                    &mut privileges,
                    &["class", "schema", "object", "column", "state", "permission"],
                );
                output.sets.extend(privileges.sets);
                output.elapsed_ms += privileges.elapsed_ms;
                output
            }
        };
        // Bound metadata queries themselves to 1001 and preserve truncation in
        // parameter_query results (which intentionally do not clip structure).
        for set in &mut output.sets {
            if set.rows.len() > ROW_LIMIT {
                set.rows.truncate(ROW_LIMIT);
                set.truncated = true;
            }
        }
        output.messages.push(crate::model::QueryMessage { severity:"权限说明".into(),text:"显示目录可见的直接授权与角色关系；继承、PUBLIC/内置角色、所有者、服务器权限、DENY、RLS 与列级规则可能改变有效权限。".into(),..Default::default() });
        Ok(output)
    }

    pub async fn apply_security_change(
        &mut self,
        change: &SecurityChange,
        expected_sql: &str,
    ) -> Result<()> {
        let sql = security_change_sql(self.profile.engine, change)?;
        if sql != expected_sql {
            return Err(rejected("权限变更与已展示的 SQL 预览不符，请重新预览。"));
        }
        self.ensure_edit_transaction_safe().await?;
        match change {
            SecurityChange::CreateRole { name } => {
                let role = AccessPrincipal {
                    name: name.clone(),
                    host: (self.profile.engine == Engine::MySql).then(|| "%".into()),
                    kind: PrincipalKind::Role,
                    enabled: None,
                    default_schema: None,
                    identity: None,
                };
                if self.security_principal_exists(&role).await? {
                    return Err(rejected("目标账号或角色已存在；目录已发生变化，请刷新。"));
                }
            }
            SecurityChange::DropRole { role } => self.ensure_security_principal(role).await?,
            SecurityChange::GrantRole { role, member }
            | SecurityChange::RevokeRole { role, member } => {
                self.ensure_security_principal(role).await?;
                self.ensure_security_principal(member).await?;
            }
            SecurityChange::GrantTablePrivilege {
                principal, table, ..
            }
            | SecurityChange::RevokeTablePrivilege {
                principal, table, ..
            } => {
                self.ensure_security_principal(principal).await?;
                // Verify existence/visibility in the bound database before any
                // grant; never infer that a browse snapshot is still current.
                self.table_columns(table).await?;
            }
        }
        self.query_internal(&sql).await?;
        Ok(())
    }

    async fn ensure_security_principal(&mut self, principal: &AccessPrincipal) -> Result<()> {
        if !self.security_principal_exists(principal).await? {
            return Err(rejected(
                "账号或角色已不存在、类型已变化或当前账号无法查看；请刷新权限目录。",
            ));
        }
        Ok(())
    }

    async fn security_principal_exists(&mut self, principal: &AccessPrincipal) -> Result<bool> {
        principal_sql(self.profile.engine, principal)?;
        let (sql, parameters): (&str, Vec<&str>) = match self.profile.engine {
            Engine::MySql => (
                "SELECT CASE WHEN EXISTS(SELECT 1 FROM mysql.user WHERE User=? AND Host=?) THEN 'true' ELSE 'false' END",
                vec![&principal.name, principal.host.as_deref().unwrap()],
            ),
            Engine::Postgres => (
                "SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=$1)::text",
                vec![&principal.name],
            ),
            Engine::SqlServer => {
                if principal.kind == PrincipalKind::Login {
                    return Err(rejected("变更流程不支持服务器登录。"));
                }
                (
                    "SELECT CASE WHEN EXISTS(SELECT 1 FROM sys.database_principals WHERE name=@P1 AND ((@P2='role' AND type='R') OR (@P2='user' AND type IN ('S','U','G','E','X','C','K')))) THEN 'true' ELSE 'false' END",
                    vec![
                        &principal.name,
                        if principal.kind == PrincipalKind::Role {
                            "role"
                        } else {
                            "user"
                        },
                    ],
                )
            }
        };
        let output = self.parameter_query(sql, &parameters).await?;
        let value = output
            .sets
            .first()
            .and_then(|set| set.rows.first())
            .and_then(|row| cell(row, 0))
            .ok_or_else(|| rejected("服务器未返回账号存在状态，请刷新。"))?;
        Ok(value == "true")
    }
}

fn cell(row: &[Option<String>], index: usize) -> Option<&str> {
    row.get(index).and_then(Option::as_deref)
}

fn append_principals(
    catalog: &mut SecurityCatalog,
    output: QueryOutput,
    engine: Engine,
    label: &str,
) {
    for set in output.sets {
        if set.truncated {
            catalog.truncated = true;
            catalog.warnings.push(format!(
                "{label}超过 {ROW_LIMIT} 项，仅显示前 {ROW_LIMIT} 项。"
            ));
        }
        for row in set.rows {
            if let Some(name) = cell(&row, 0) {
                catalog.principals.push(AccessPrincipal {
                    name: name.into(),
                    host: cell(&row, 1).map(str::to_owned),
                    kind: match cell(&row, 2) {
                        Some("role") => PrincipalKind::Role,
                        Some("login") => PrincipalKind::Login,
                        _ => PrincipalKind::User,
                    },
                    enabled: match cell(&row, 3) {
                        Some("true") => Some(true),
                        Some("false") => Some(false),
                        _ => None,
                    },
                    default_schema: cell(&row, 4).map(str::to_owned),
                    identity: cell(&row, 5).map(str::to_owned),
                });
            }
        }
    }
    if engine == Engine::SqlServer {
        catalog
            .principals
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.kind.label().cmp(b.kind.label())));
    }
}

fn set_names(output: &mut QueryOutput, names: &[&str]) {
    for set in &mut output.sets {
        set.columns = names.iter().map(|name| (*name).into()).collect();
    }
}

const MYSQL_PRINCIPALS: &str = r#"SELECT CAST(User AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,CAST(Host AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,'user',CASE WHEN account_locked='Y' THEN 'false' ELSE 'true' END,NULL,CONCAT(CAST(plugin AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,':',COALESCE(CAST(password_last_changed AS CHAR),'unknown')) FROM mysql.user ORDER BY User,Host LIMIT 1001"#;
const PG_PRINCIPALS: &str = r#"SELECT rolname::text,NULL,CASE WHEN rolcanlogin THEN 'user' ELSE 'role' END,rolcanlogin::text,NULL,oid::text FROM pg_roles ORDER BY rolname LIMIT 1001"#;
const MSSQL_PRINCIPALS: &str = r#"SELECT TOP (1001) name,NULL,CASE WHEN type='R' THEN 'role' ELSE 'user' END,NULL,default_schema_name,'user:'+type+':'+CONVERT(nvarchar(20),principal_id)+':'+CONVERT(nvarchar(33),create_date,126) FROM sys.database_principals WHERE type IN ('R','S','U','G','E','X','C','K') ORDER BY name"#;
const MSSQL_LOGINS: &str = r#"SELECT TOP (1001) name,NULL,'login',CASE WHEN is_disabled=1 THEN 'false' ELSE 'true' END,NULL,'login:'+type+':'+CONVERT(nvarchar(20),principal_id)+':'+CONVERT(nvarchar(33),create_date,126) FROM sys.server_principals WHERE type IN ('S','U','G','E','X') ORDER BY name"#;

const PG_MEMBERSHIPS: &str = r#"SELECT CASE WHEN member.rolname=$1 THEN 'member_of' ELSE 'has_member' END,role.rolname::text,member.rolname::text,m.admin_option::text FROM pg_auth_members m JOIN pg_roles role ON role.oid=m.roleid JOIN pg_roles member ON member.oid=m.member WHERE member.rolname=$1 OR role.rolname=$1 ORDER BY role.rolname,member.rolname LIMIT 1001"#;
const MYSQL_MEMBERSHIPS: &str = r#"SELECT CASE WHEN e.TO_USER=? AND e.TO_HOST=? THEN 'member_of' ELSE 'has_member' END,
    CAST(e.FROM_USER AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,CAST(e.FROM_HOST AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(e.TO_USER AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,CAST(e.TO_HOST AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(e.WITH_ADMIN_OPTION AS CHAR),CASE WHEN d.USER IS NOT NULL THEN 'true' ELSE 'false' END
    FROM mysql.role_edges e LEFT JOIN mysql.default_roles d ON d.USER=e.TO_USER AND d.HOST=e.TO_HOST AND d.DEFAULT_ROLE_USER=e.FROM_USER AND d.DEFAULT_ROLE_HOST=e.FROM_HOST
    WHERE (e.TO_USER=? AND e.TO_HOST=?) OR (e.FROM_USER=? AND e.FROM_HOST=?) ORDER BY e.FROM_USER,e.FROM_HOST,e.TO_USER,e.TO_HOST LIMIT 1001"#;
const PG_GRANTS: &str = r#"WITH grants AS (
    SELECT CASE WHEN c.relkind='S' THEN 'SEQUENCE' ELSE 'RELATION' END AS class,n.nspname::text AS schema_name,c.relname::text AS object_name,x.*
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    CROSS JOIN LATERAL aclexplode(COALESCE(c.relacl,acldefault(CASE WHEN c.relkind='S' THEN 's'::"char" ELSE 'r'::"char" END,c.relowner))) x
    WHERE c.relkind IN ('r','p','v','m','f','S')
    UNION ALL SELECT 'SCHEMA',n.nspname::text,n.nspname::text,x.* FROM pg_namespace n CROSS JOIN LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) x
    UNION ALL SELECT 'DATABASE',NULL,d.datname::text,x.* FROM pg_database d CROSS JOIN LATERAL aclexplode(COALESCE(d.datacl,acldefault('d',d.datdba))) x WHERE d.datname=current_database()
    UNION ALL SELECT 'ROUTINE',n.nspname::text,p.proname::text||'('||pg_get_function_identity_arguments(p.oid)||')',x.* FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace CROSS JOIN LATERAL aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) x
    UNION ALL SELECT 'COLUMN',n.nspname::text,c.relname::text||'.'||a.attname::text,x.* FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace CROSS JOIN LATERAL aclexplode(a.attacl) x WHERE a.attnum>0 AND NOT a.attisdropped
    ) SELECT g.class,grantor.rolname::text,grantee.rolname::text,g.schema_name,g.object_name,g.privilege_type::text,g.is_grantable::text
    FROM grants g JOIN pg_roles grantee ON grantee.oid=g.grantee LEFT JOIN pg_roles grantor ON grantor.oid=g.grantor
    WHERE grantee.rolname=$1 ORDER BY g.class,g.schema_name,g.object_name,g.privilege_type LIMIT 1001"#;
const MSSQL_MEMBERSHIPS: &str = r#"SELECT TOP (1001) CASE WHEN member.name=@P1 THEN 'member_of' ELSE 'has_member' END,role.name,member.name FROM sys.database_role_members m JOIN sys.database_principals role ON role.principal_id=m.role_principal_id JOIN sys.database_principals member ON member.principal_id=m.member_principal_id WHERE role.name=@P1 OR member.name=@P1 ORDER BY role.name,member.name"#;
const MSSQL_GRANTS: &str = r#"SELECT TOP (1001) p.class_desc,CASE WHEN p.class=1 THEN OBJECT_SCHEMA_NAME(p.major_id) WHEN p.class=3 THEN SCHEMA_NAME(p.major_id) END,CASE WHEN p.class=1 THEN OBJECT_NAME(p.major_id) END,CASE WHEN p.class=1 AND p.minor_id>0 THEN COL_NAME(p.major_id,p.minor_id) END,p.state_desc,p.permission_name FROM sys.database_permissions p JOIN sys.database_principals u ON u.principal_id=p.grantee_principal_id WHERE u.name=@P1 ORDER BY p.class,p.major_id,p.minor_id,p.permission_name"#;
const MSSQL_SERVER_MEMBERSHIPS: &str = r#"SELECT TOP (1001) CASE WHEN member.name=@P1 THEN 'member_of' ELSE 'has_member' END,role.name,member.name FROM sys.server_role_members m JOIN sys.server_principals role ON role.principal_id=m.role_principal_id JOIN sys.server_principals member ON member.principal_id=m.member_principal_id WHERE role.name=@P1 OR member.name=@P1 ORDER BY role.name,member.name"#;
const MSSQL_SERVER_GRANTS: &str = r#"SELECT TOP (1001) p.class_desc,NULL,NULL,NULL,p.state_desc,p.permission_name FROM sys.server_permissions p JOIN sys.server_principals u ON u.principal_id=p.grantee_principal_id WHERE u.name=@P1 ORDER BY p.class,p.major_id,p.permission_name"#;
