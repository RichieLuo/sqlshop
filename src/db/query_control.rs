use super::{Driver, Session};
use crate::{
    model::{Credentials, Engine, Profile, QueryOutput},
    query_control::{QueryTransactionState, TransactionCommand},
    ssh::Tunnel,
};
use anyhow::{Context, Result};
use sqlx::Connection;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
enum CancelTarget {
    MySql { id: u32, server_uuid: String },
    Postgres { process: u32, secret: u32 },
    SqlServer(tiberius::AttentionHandle),
}

/// Bound to the original server session and one execution generation.
/// Credentials/backend secrets are intentionally omitted from Debug and are
/// never persisted. The worker must finish cancellation before reusing the
/// session, even if the query response finishes first.
#[derive(Clone)]
pub struct QueryCancelHandle {
    target: CancelTarget,
    profile: Profile,
    credentials: Credentials,
    armed: Arc<AtomicBool>,
}

impl std::fmt::Debug for QueryCancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCancelHandle")
            .field("engine", &self.profile.engine)
            .finish_non_exhaustive()
    }
}

impl QueryCancelHandle {
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
        if let CancelTarget::SqlServer(handle) = &self.target {
            handle.disarm();
        }
    }

    pub async fn cancel(&self) -> Result<()> {
        self.cancel_if_active(Arc::new(AtomicBool::new(true)))
            .await?;
        Ok(())
    }

    /// Returns whether a cancellation request was sent. Only the response on
    /// the original connection confirms whether the server stopped execution.
    /// Gate again after network/authentication setup to avoid cancelling the
    /// next request after a fast completion. The worker owns this atomic and
    /// clears it immediately when the associated query future completes.
    pub async fn cancel_if_active(&self, active: Arc<AtomicBool>) -> Result<bool> {
        if !active.load(Ordering::SeqCst) || !self.armed.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if let CancelTarget::SqlServer(handle) = &self.target {
            return Ok(handle.request());
        }
        let profile = &self.profile;
        // Query tunnels accept one transport, so a cancellation connection
        // must own its own authenticated SSH channel to the same remote host.
        let tunnel = if profile.ssh.enabled {
            Some(
                Tunnel::open(
                    &profile.ssh,
                    &self.credentials.ssh_secret,
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
        if !active.load(Ordering::SeqCst) || !self.armed.load(Ordering::SeqCst) {
            return Ok(false);
        }
        match &self.target {
            CancelTarget::Postgres { process, secret } => {
                use sqlx::postgres::{PgConnectOptions, PgSslMode};
                let mode = match (profile.tls, profile.trust_certificate) {
                    (false, _) => PgSslMode::Disable,
                    (true, true) => PgSslMode::Require,
                    (true, false) => PgSslMode::VerifyFull,
                };
                let options = PgConnectOptions::new().host(host).port(port).ssl_mode(mode);
                sqlx::PgConnection::cancel_request_if_active(&options, (*process, *secret), &active)
                    .await
                    .context("发送 PostgreSQL CancelRequest 失败")
            }
            CancelTarget::MySql { id, server_uuid } => {
                use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
                let mode = match (profile.tls, profile.trust_certificate) {
                    (false, _) => MySqlSslMode::Disabled,
                    (true, true) => MySqlSslMode::Required,
                    (true, false) => MySqlSslMode::VerifyIdentity,
                };
                let options = MySqlConnectOptions::new()
                    .host(host)
                    .port(port)
                    .username(&profile.username)
                    .password(&self.credentials.database_password)
                    .ssl_mode(mode);
                let mut control = sqlx::MySqlConnection::connect_with(&options)
                    .await
                    .context("连接 MySQL 查询控制通道失败")?;
                if !active.load(Ordering::SeqCst) || !self.armed.load(Ordering::SeqCst) {
                    return Ok(false);
                }
                // A load balancer may route the control connection to a different
                // instance, where the same numeric id can name another session.
                let actual: String = sqlx::query_scalar("SELECT @@server_uuid")
                    .fetch_one(&mut control)
                    .await
                    .context("控制连接无法核实 MySQL 实例，未发送停止请求")?;
                anyhow::ensure!(
                    actual == *server_uuid,
                    "查询连接与控制连接的 MySQL 实例不同，未发送停止请求"
                );
                if !active.load(Ordering::SeqCst) || !self.armed.load(Ordering::SeqCst) {
                    return Ok(false);
                }
                // The numeric id comes from the original handshake, never from
                // a process-list search or untrusted SQL text. KILL QUERY keeps
                // that server connection and its transaction alive.
                sqlx::raw_sql(&format!("KILL QUERY {id}"))
                    .execute(&mut control)
                    .await
                    .context("MySQL 拒绝停止查询；检查该账号停止自身查询的权限")?;
                // The control result confirms receipt. A shutdown failure must
                // not hide a successful KILL response.
                let _ = control.close().await;
                Ok(true)
            }
            CancelTarget::SqlServer(_) => unreachable!(),
        }
    }
}

impl Session {
    /// Once at query-document connection startup, before the first user SQL.
    /// Regular management/table operations do not force tracker configuration.
    pub async fn prepare_query_context(&mut self) -> Result<()> {
        if let Driver::MySql(connection) = &mut self.driver
            && connection.enable_session_context_tracking().await?
        {
            let database = sqlx::query_scalar::<_, Option<String>>(
                "SELECT CAST(DATABASE() AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci",
            )
            .fetch_one(&mut *connection)
            .await?;
            connection.confirm_current_database(database);
        }
        Ok(())
    }

    /// Read original response protocol state. No SQL or transaction probes.
    /// None is unobserved/absent context and must not inherit an older database.
    pub async fn observed_query_database(&mut self) -> Result<Option<String>> {
        Ok(match &mut self.driver {
            Driver::MySql(connection) => connection.observed_current_database().await?,
            Driver::SqlServer(client) => client.observed_current_database().map(str::to_owned),
            // PostgreSQL cannot switch databases through SQL; the actor retains
            // its already confirmed connection target instead of probing.
            Driver::Postgres(_) => None,
        })
    }

    /// Called only after an explicit successful database/context inspection.
    pub fn confirm_observed_query_database(&mut self, database: Option<String>) {
        if let Driver::MySql(connection) = &mut self.driver {
            connection.confirm_current_database(database);
        }
    }

    /// Initialize identity once, before the first user statement. The result
    /// cannot be inferred from a numeric connection id behind a load balancer.
    /// MariaDB variants lacking @@server_uuid may still execute user SQL, but
    /// cancellation remains unavailable instead of using an ambiguous id.
    pub async fn prepare_query_control(&mut self) -> Result<()> {
        if self.mysql_server_uuid.is_some() {
            return Ok(());
        }
        if let Driver::MySql(connection) = &mut self.driver {
            let server_uuid: String = sqlx::query_scalar("SELECT @@server_uuid")
                .fetch_one(connection)
                .await
                .context("无法核实 MySQL 实例标识，取消不可用；查询仍可执行")?;
            uuid::Uuid::parse_str(&server_uuid).context("MySQL 实例 UUID 无效，取消不可用")?;
            self.mysql_server_uuid = Some(server_uuid);
        }
        Ok(())
    }

    /// Call once for each execution, before borrowing the session for its SQL.
    pub fn cancel_handle(&mut self) -> Result<QueryCancelHandle> {
        let target = match &mut self.driver {
            Driver::MySql(connection) => CancelTarget::MySql {
                id: connection.server_connection_id(),
                server_uuid: self
                    .mysql_server_uuid
                    .clone()
                    .context("未核实 MySQL 实例标识，当前会话不能安全取消；查询仍可执行")?,
            },
            Driver::Postgres(connection) => {
                let (process, secret) = connection.cancel_key();
                anyhow::ensure!(process != 0, "服务器未提供 PostgreSQL 取消标识");
                CancelTarget::Postgres { process, secret }
            }
            Driver::SqlServer(client) => CancelTarget::SqlServer(client.attention_handle()),
        };
        Ok(QueryCancelHandle {
            target,
            profile: self.profile.clone(),
            credentials: self.credentials.clone(),
            armed: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Server state observed from response packets, without executing SQL.
    /// SQL Server's descriptor proves presence but cannot prove committability.
    /// MySQL ERR packets lack status and therefore produce Unknown until a new
    /// successful status-bearing response is received.
    pub async fn transaction_state(&mut self) -> Result<QueryTransactionState> {
        use QueryTransactionState::*;
        Ok(match &mut self.driver {
            Driver::MySql(connection) => match connection.observed_in_transaction().await? {
                Some(true) => Active,
                Some(false) => Idle,
                None => Unknown,
            },
            Driver::Postgres(connection) => match connection.observed_transaction_status().await? {
                b'I' => Idle,
                b'T' => Active,
                b'E' => Failed,
                _ => Unknown,
            },
            Driver::SqlServer(client) => {
                if client.has_server_transaction() {
                    Active
                } else {
                    Idle
                }
            }
        })
    }

    /// Explicit SQL Server inspection. This read-only query preserves the
    /// transaction but changes server statement diagnostics such as @@ROWCOUNT.
    pub async fn inspect_transaction_state(&mut self) -> Result<QueryTransactionState> {
        if let Driver::MySql(connection) = &mut self.driver {
            // A MySQL ERR packet omits status. An explicit protocol PING obtains
            // a fresh status-bearing OK without running a SQL statement.
            connection.ping().await?;
            return self.transaction_state().await;
        }
        let Driver::SqlServer(client) = &mut self.driver else {
            return self.transaction_state().await;
        };
        let row = client
            .simple_query("SELECT CAST(XACT_STATE() AS int) AS sqlshop_xact_state")
            .await?
            .into_row()
            .await?
            .context("服务器未返回事务状态")?;
        Ok(match row.try_get::<i32, _>(0)?.context("事务状态为空")? {
            0 => QueryTransactionState::Idle,
            1 => QueryTransactionState::Active,
            -1 => QueryTransactionState::Failed,
            _ => QueryTransactionState::Unknown,
        })
    }

    /// Execute only the user's requested transaction operation on this session.
    /// Begin is guarded: MySQL START TRANSACTION can implicitly commit an
    /// existing transaction and SQL Server BEGIN can increase nesting depth.
    pub async fn transaction_command(
        &mut self,
        command: TransactionCommand,
    ) -> Result<QueryOutput> {
        let state = self.transaction_state().await?;
        if command == TransactionCommand::Begin && state != QueryTransactionState::Idle {
            return Err(
                super::EditRejected("当前事务状态不是空闲；请先检查并完成现有事务".into()).into(),
            );
        }
        let sql = match (self.profile.engine, command) {
            (Engine::MySql, TransactionCommand::Begin) => "START TRANSACTION",
            (Engine::SqlServer, TransactionCommand::Begin) => "BEGIN TRANSACTION",
            (_, TransactionCommand::Begin) => "BEGIN",
            (_, TransactionCommand::Commit) => "COMMIT",
            (_, TransactionCommand::Rollback) => "ROLLBACK",
        };
        self.query(sql).await
    }
}

pub fn is_query_cancelled(error: &anyhow::Error) -> bool {
    if let Some(sqlx::Error::Database(error)) = error.downcast_ref::<sqlx::Error>() {
        if let Some(mysql) = error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>() {
            return mysql.number() == 1317;
        }
        return error.code().as_deref() == Some("57014");
    }
    matches!(
        error.downcast_ref::<tiberius::error::Error>(),
        Some(tiberius::error::Error::Cancelled)
    )
}
