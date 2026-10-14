//! Native backup utilities and SQL Server's server-side backup statements.
//! Plans are previews; only execute() starts work. No shell interpolation.
use crate::{
    db::Session,
    model::{Credentials, Engine, Profile},
    ssh::Tunnel,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{io::AsyncReadExt, process::Command};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupOperation {
    Create,
    Restore,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupFormat {
    Sql,
    PostgresCustom,
    SqlServerNative,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupLocation {
    LocalFile(PathBuf),
    ServerFile(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupFileMove {
    pub logical_name: String,
    pub server_path: String,
}
fn first_backup_set() -> u32 {
    1
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DraftBackupConfig {
    pub profile: Uuid,
    pub database: String,
    pub operation: BackupOperation,
    pub format: BackupFormat,
    pub location: BackupLocation,
    /// Path to the operation's tool; None uses PATH. No automatic installation.
    pub tool_path: Option<PathBuf>,
    /// SQL Server media can contain multiple appended backup sets.
    #[serde(default = "first_backup_set")]
    pub backup_set: u32,
    #[serde(default)]
    pub file_moves: Vec<BackupFileMove>,
}

#[derive(Clone, Debug)]
pub struct BackupResult {
    pub exit_code: Option<i32>,
    pub summary: String,
    pub artifact: BackupLocation,
    /// Verified local file size, absent for server-owned files.
    pub local_bytes: Option<u64>,
}

impl DraftBackupConfig {
    pub fn for_database(profile: &Profile, database: impl Into<String>) -> Self {
        Self {
            profile: profile.id,
            database: database.into(),
            operation: BackupOperation::Create,
            format: if profile.engine == Engine::SqlServer {
                BackupFormat::SqlServerNative
            } else if profile.engine == Engine::Postgres {
                BackupFormat::PostgresCustom
            } else {
                BackupFormat::Sql
            },
            location: if profile.engine == Engine::SqlServer {
                BackupLocation::ServerFile(String::new())
            } else {
                BackupLocation::LocalFile(PathBuf::new())
            },
            tool_path: None,
            backup_set: 1,
            file_moves: Vec::new(),
        }
    }
    pub fn validate(&self, profile: &Profile) -> Result<()> {
        anyhow::ensure!(profile.id == self.profile, "备份连接与实际连接不一致");
        profile.validate()?;
        anyhow::ensure!(
            !self.database.trim().is_empty() && !self.database.contains('\0'),
            "数据库名称无效"
        );
        match (&self.location, profile.engine, self.format) {
            (
                BackupLocation::ServerFile(path),
                Engine::SqlServer,
                BackupFormat::SqlServerNative,
            ) => {
                anyhow::ensure!(
                    !path.trim().is_empty() && !path.contains('\0'),
                    "请输入 SQL Server 主机上的备份文件路径"
                );
            }
            (BackupLocation::LocalFile(path), Engine::MySql, BackupFormat::Sql)
            | (
                BackupLocation::LocalFile(path),
                Engine::Postgres,
                BackupFormat::Sql | BackupFormat::PostgresCustom,
            ) => {
                anyhow::ensure!(
                    !path.as_os_str().is_empty() && path.file_name().is_some(),
                    "请输入本地备份文件路径"
                );
                anyhow::ensure!(path.to_str().is_some(), "备份文件路径必须能以 UTF-8 表示");
            }
            _ => bail!("此数据库、备份格式与文件位置的组合不受支持"),
        }
        if let Some(path) = &self.tool_path {
            anyhow::ensure!(!path.as_os_str().is_empty(), "工具路径不能为空");
            anyhow::ensure!(path.to_str().is_some(), "工具路径必须能以 UTF-8 表示");
        }
        anyhow::ensure!(
            (1..=i32::MAX as u32).contains(&self.backup_set),
            "备份集序号应为正整数"
        );
        let mut names = std::collections::HashSet::new();
        for mapping in &self.file_moves {
            anyhow::ensure!(
                profile.engine == Engine::SqlServer && self.operation == BackupOperation::Restore,
                "文件映射只用于 SQL Server 恢复"
            );
            anyhow::ensure!(
                !mapping.logical_name.trim().is_empty()
                    && !mapping.logical_name.contains('\0')
                    && !mapping.server_path.trim().is_empty()
                    && !mapping.server_path.contains('\0'),
                "恢复文件映射无效"
            );
            anyhow::ensure!(names.insert(&mapping.logical_name), "逻辑文件名映射重复");
        }
        Ok(())
    }
}

pub fn native_tool(profile: &Profile, config: &DraftBackupConfig) -> Result<PathBuf> {
    anyhow::ensure!(profile.id == config.profile, "工具配置与连接不一致");
    let default = match (profile.engine, config.operation, config.format) {
        (Engine::MySql, BackupOperation::Create, BackupFormat::Sql) => "mysqldump",
        (Engine::MySql, BackupOperation::Restore, BackupFormat::Sql) => "mysql",
        (Engine::Postgres, BackupOperation::Create, _) => "pg_dump",
        (Engine::Postgres, BackupOperation::Restore, BackupFormat::PostgresCustom) => "pg_restore",
        (Engine::Postgres, BackupOperation::Restore, BackupFormat::Sql) => "psql",
        (Engine::SqlServer, _, _) => bail!("SQL Server 备份由服务器执行，不使用本机备份工具"),
        _ => bail!("不支持的备份格式"),
    };
    Ok(config
        .tool_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(default)))
}

fn sqlserver_sql(config: &DraftBackupConfig) -> Result<String> {
    let BackupLocation::ServerFile(path) = &config.location else {
        bail!("SQL Server 需要服务器文件路径");
    };
    let target = Engine::SqlServer.quote(&config.database);
    let path = path.replace('\'', "''");
    Ok(match config.operation {
        // NOINIT avoids overwriting an existing backup media set.
        BackupOperation::Create => format!(
            "BACKUP DATABASE {target} TO DISK = N'{path}' WITH COPY_ONLY, NOINIT, CHECKSUM, STATS = 10;"
        ),
        BackupOperation::Restore => {
            let moves = config
                .file_moves
                .iter()
                .map(|m| {
                    format!(
                        ", MOVE N'{}' TO N'{}'",
                        m.logical_name.replace('\'', "''"),
                        m.server_path.replace('\'', "''")
                    )
                })
                .collect::<String>();
            format!(
                "RESTORE DATABASE {target} FROM DISK = N'{path}' WITH FILE = {}, CHECKSUM, STATS = 10{moves};",
                config.backup_set
            )
        }
    })
}

pub fn preview(profile: &Profile, config: &DraftBackupConfig) -> Result<String> {
    config.validate(profile)?;
    let action = if config.operation == BackupOperation::Create {
        "创建备份"
    } else {
        "恢复备份"
    };
    let mut text = format!(
        "{action}\n连接：{} ({})\n数据库：{}\n",
        profile.name,
        profile.engine.label(),
        config.database
    );
    match &config.location {
        BackupLocation::ServerFile(path) => {
            text.push_str(&format!(
                "文件位置：SQL Server 主机\n路径：{path}\n\n{}",
                sqlserver_sql(config)?
            ));
            if config.operation == BackupOperation::Create {
                text.push_str("\n服务器原备份文件存在时追加 COPY_ONLY 备份，不覆盖介质。");
            } else {
                text.push_str("\n仅恢复至不存在或空的目标数据库；不使用 REPLACE，不断开其他会话。需目标独占访问；未设置 MOVE 的文件使用备份中的原位置。");
            }
        }
        BackupLocation::LocalFile(path) => {
            text.push_str(&format!(
                "文件位置：本机\n路径：{}\n工具：{}\n",
                path.display(),
                native_tool(profile, config)?.display()
            ));
            if config.operation == BackupOperation::Create {
                text.push_str("创建新文件，已存在的目标文件会拒绝覆盖。\n");
            } else {
                text.push_str("仅恢复至现有可访问的空数据库；不添加 DROP、--clean 或 --create。\nSQL 文件由原生工具执行，文件中的 SQL 或客户端命令也会执行。\n");
            }
            if profile.tls {
                text.push_str(if profile.trust_certificate {
                    "TLS：加密，信任服务器证书\n"
                } else {
                    "TLS：加密并校验证书及主机名\n"
                });
            } else {
                text.push_str("TLS：按连接配置关闭\n");
            }
            if profile.ssh.enabled {
                text.push_str("SSH：使用此连接的受信任 SSH 配置建立独立隧道。\n");
            }
        }
    }
    Ok(text)
}

pub async fn detect_tool(profile: &Profile, config: &DraftBackupConfig) -> Result<String> {
    if profile.engine == Engine::SqlServer {
        return Ok("SQL Server 原生服务器命令（无需本机工具；执行时核对服务器支持与权限）".into());
    }
    let program = native_tool(profile, config)?;
    let output = Command::new(&program)
        .arg("--version")
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| {
            format!(
                "无法运行 {}；请安装对应原生数据库工具或指定完整路径",
                program.display()
            )
        })?;
    anyhow::ensure!(output.status.success(), "原生工具版本检测失败");
    Ok(String::from_utf8_lossy(&output.stdout)
        .chars()
        .take(2000)
        .collect())
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn private_file(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("无法创建本地临时文件：{}", path.display()))
}
fn option_value(value: &str) -> Result<String> {
    anyhow::ensure!(!value.contains('\0'), "数据库凭据包含无效字符");
    Ok(value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t"))
}
fn redact(text: &str, credentials: &Credentials) -> String {
    let mut text = text.to_owned();
    for secret in [&credentials.database_password, &credentials.ssh_secret] {
        if !secret.is_empty() {
            text = text.replace(secret, "[已隐去凭据]");
            if let Ok(escaped) = option_value(secret)
                && !escaped.is_empty()
            {
                text = text.replace(&escaped, "[已隐去凭据]");
            }
        }
    }
    text.chars().take(8000).collect()
}

pub async fn execute(
    profile: &Profile,
    credentials: &Credentials,
    config: &DraftBackupConfig,
) -> Result<BackupResult> {
    config.validate(profile)?;
    // Restore checks use a dedicated management connection, never a document's
    // persistent SQL session. They do not automatically delete/create objects.
    if config.operation == BackupOperation::Restore {
        let mut preflight = Session::connect(profile, credentials).await?;
        preflight
            .ensure_restore_target_empty(&config.database)
            .await?;
    }
    if profile.engine == Engine::SqlServer {
        let mut session = Session::connect(profile, credentials).await?;
        session.select_database("master").await?;
        let output = session.query(&sqlserver_sql(config)?).await?;
        let messages = output
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(BackupResult {
            exit_code: None,
            summary: format!("服务器原生命令已完成。\n{}", redact(&messages, credentials)),
            artifact: config.location.clone(),
            local_bytes: None,
        });
    }
    let BackupLocation::LocalFile(path) = &config.location else {
        bail!("本机工具需要本地文件路径");
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    anyhow::ensure!(parent.is_dir(), "备份目录不存在");
    if config.operation == BackupOperation::Create {
        anyhow::ensure!(!path.exists(), "目标文件已存在，未覆盖");
    } else {
        anyhow::ensure!(
            path.is_file() && fs::metadata(path)?.len() > 0,
            "恢复文件不存在或为空"
        );
    }
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
    let mut command = Command::new(native_tool(profile, config)?);
    command.kill_on_drop(true).stderr(Stdio::piped());
    // Remove inherited connection overrides; selected profile fully defines the
    // target. Credentials are temporary child state, never argv or Debug.
    for name in [
        "PGSERVICE",
        "PGSERVICEFILE",
        "PGOPTIONS",
        "PGDATABASE",
        "PGHOST",
        "PGHOSTADDR",
        "PGPORT",
        "PGUSER",
        "PGPASSWORD",
        "PGPASSFILE",
        "MYSQL_PWD",
    ] {
        command.env_remove(name);
    }
    let mut credential_file = None;
    match profile.engine {
        Engine::MySql => {
            let temporary = TemporaryFile(
                std::env::temp_dir().join(format!("sqlshop-backup-{}.cnf", Uuid::new_v4())),
            );
            let mut file = private_file(&temporary.0)?;
            writeln!(
                file,
                "[client]\npassword=\"{}\"",
                option_value(&credentials.database_password)?
            )?;
            file.sync_all()?;
            command
                .arg(format!("--defaults-file={}", temporary.0.display()))
                .arg("--no-login-paths");
            command
                .arg(format!("--host={host}"))
                .arg(format!("--port={port}"))
                .arg(format!("--user={}", profile.username))
                .arg("--protocol=TCP")
                .arg(format!(
                    "--ssl-mode={}",
                    match (profile.tls, profile.trust_certificate) {
                        (false, _) => "DISABLED",
                        (true, true) => "REQUIRED",
                        (true, false) => "VERIFY_IDENTITY",
                    }
                ));
            if config.operation == BackupOperation::Create {
                command.args([
                    "--single-transaction",
                    "--routines",
                    "--events",
                    "--triggers",
                    "--no-tablespaces",
                    "--set-gtid-purged=OFF",
                    "--hex-blob",
                    "--skip-add-drop-table",
                    "--skip-add-locks",
                ]);
            } else {
                command.arg("--binary-mode=1");
            }
            command.arg("--").arg(&config.database);
            credential_file = Some(temporary);
        }
        Engine::Postgres => {
            command
                .env("PGHOST", host)
                .env("PGPORT", port.to_string())
                .env("PGUSER", &profile.username)
                .env("PGDATABASE", &config.database)
                .env("PGPASSWORD", &credentials.database_password)
                .env(
                    "PGSSLMODE",
                    match (profile.tls, profile.trust_certificate) {
                        (false, _) => "disable",
                        (true, true) => "require",
                        (true, false) => "verify-full",
                    },
                );
            command.arg("--no-password");
            let name = config.database.replace('\\', "\\\\").replace('\'', "\\'");
            command.arg(format!("--dbname=dbname='{name}'"));
            match (config.operation, config.format) {
                (BackupOperation::Create, BackupFormat::PostgresCustom) => {
                    command.arg("--format=custom");
                }
                (BackupOperation::Create, BackupFormat::Sql) => {
                    command.args(["--format=plain", "--no-owner", "--no-privileges"]);
                }
                (BackupOperation::Restore, BackupFormat::PostgresCustom) => {
                    command.args([
                        "--format=custom",
                        "--no-owner",
                        "--no-privileges",
                        "--single-transaction",
                        "--exit-on-error",
                    ]);
                }
                (BackupOperation::Restore, BackupFormat::Sql) => {
                    command.args([
                        "-X",
                        "--set=ON_ERROR_STOP=on",
                        "--single-transaction",
                        "--file=-",
                    ]);
                }
                _ => bail!("不支持的 PostgreSQL 备份格式"),
            }
            // Database names are quoted as conninfo values so unusual names
            // cannot expand into host, password or other target overrides.
            if config.operation == BackupOperation::Restore
                && config.format == BackupFormat::PostgresCustom
            {
                // pg_restore otherwise emits SQL to stdout. Set conninfo with
                // only a safely quoted dbname; other settings remain env vars.
                command.arg("--").arg(path);
            }
        }
        Engine::SqlServer => unreachable!(),
    }
    let output_file = if config.operation == BackupOperation::Create {
        let temporary =
            TemporaryFile(parent.join(format!(".sqlshop-backup-{}.tmp", Uuid::new_v4())));
        let file = private_file(&temporary.0)?;
        command.stdout(Stdio::from(file));
        command.stdin(Stdio::null());
        Some(temporary)
    } else {
        command.stdout(Stdio::null());
        if profile.engine == Engine::MySql || config.format == BackupFormat::Sql {
            command.stdin(Stdio::from(fs::File::open(path)?));
        } else {
            command.stdin(Stdio::null());
        }
        None
    };
    let mut child = command.spawn().map_err(|error| {
        anyhow::anyhow!(
            "启动原生备份工具失败：{}",
            redact(&error.to_string(), credentials)
        )
    })?;
    let mut stderr_pipe = child.stderr.take().context("无法读取原生工具消息")?;
    let read_stderr = async {
        let mut kept = Vec::new();
        let mut chunk = [0_u8; 4096];
        let mut truncated = false;
        loop {
            let count = stderr_pipe.read(&mut chunk).await?;
            if count == 0 {
                break;
            }
            let room = 65536_usize.saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..count.min(room)]);
            truncated |= count > room;
        }
        Ok::<_, std::io::Error>((kept, truncated))
    };
    // Drain stderr continuously so a verbose utility cannot block on a full
    // pipe. Keep a bounded message while the backup itself streams to disk.
    let (status, captured) = tokio::join!(child.wait(), read_stderr);
    let status = status.map_err(|error| {
        anyhow::anyhow!(
            "等待原生工具结果失败：{}",
            redact(&error.to_string(), credentials)
        )
    })?;
    let (captured, truncated) = captured?;
    drop(credential_file);
    let mut stderr = redact(&String::from_utf8_lossy(&captured), credentials);
    if truncated {
        stderr.push_str("\n原生工具消息过多，已截断显示。\n");
    }
    anyhow::ensure!(
        status.success(),
        "原生工具失败（退出码 {:?}）：{stderr}；恢复可能已执行部分操作，请核对目标，未自动重试",
        status.code()
    );
    let local_bytes = if let Some(temporary) = output_file {
        let file = fs::OpenOptions::new().read(true).open(&temporary.0)?;
        file.sync_all()?;
        let length = file.metadata()?.len();
        anyhow::ensure!(length > 0, "工具返回成功但备份文件为空，未生成最终备份文件");
        // Unlike rename, hard_link cannot replace a target created meanwhile.
        fs::hard_link(&temporary.0, path).context("落盘失败；目标文件可能已存在，未覆盖")?;
        Some(length)
    } else {
        None
    };
    Ok(BackupResult {
        exit_code: status.code(),
        summary: format!("原生工具已完成。\n{stderr}"),
        artifact: config.location.clone(),
        local_bytes,
    })
}
