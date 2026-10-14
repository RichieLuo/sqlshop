use std::borrow::Cow;
use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

use futures_core::future::BoxFuture;
use futures_util::FutureExt;
pub(crate) use sqlx_core::connection::*;
pub(crate) use stream::{MySqlStream, Waiting};

use crate::common::StatementCache;
use crate::error::Error;
use crate::protocol::response::Status;
use crate::protocol::statement::StmtClose;
use crate::protocol::text::{Ping, Quit};
use crate::statement::MySqlStatementMetadata;
use crate::transaction::Transaction;
use crate::{MySql, MySqlColumn, MySqlConnectOptions};

mod auth;
mod establish;
mod executor;
mod stream;
mod tls;

const MAX_PACKET_SIZE: u32 = 1024;

/// A connection to a MySQL database.
pub struct MySqlConnection {
    pub(crate) inner: Box<MySqlConnectionInner>,
}

/// Events decoded from the existing query response stream.
///
/// Installing a handler does not execute additional SQL or change the returned
/// query rows/results. Events run synchronously while the query is polled.
#[derive(Debug, Clone)]
pub enum MySqlQueryEvent {
    /// Output columns received during execution, before the first row or completion.
    ///
    /// This event is also emitted for result sets with no rows. Preparing or
    /// describing a statement alone does not emit an event.
    Metadata(Vec<MySqlColumn>),
    /// A command or result set completed successfully.
    Completed {
        rows_affected: u64,
        warnings: u16,
        /// Native info text from the successful OK/result completion packet.
        info: String,
    },
}

pub(crate) struct MySqlConnectionInner {
    // underlying TCP stream,
    // wrapped in a potentially TLS stream,
    // wrapped in a buffered stream
    pub(crate) stream: MySqlStream,

    // transaction status
    pub(crate) transaction_depth: usize,
    status_flags: Status,

    // cache by query string to the statement id and metadata
    cache_statement: StatementCache<(u32, MySqlStatementMetadata)>,

    log_settings: LogSettings,

    query_event_handler: Option<Arc<dyn Fn(MySqlQueryEvent) + Send + Sync>>,
}

impl MySqlConnection {
    /// Enable protocol observation once before the first user statement. This
    /// configures tracker controls only; it never begins or ends a transaction.
    pub async fn enable_session_context_tracking(&mut self) -> Result<bool, Error> {
        if !self
            .inner
            .stream
            .capabilities
            .contains(crate::protocol::Capabilities::SESSION_TRACK)
        {
            return Ok(false);
        }
        self.inner.stream.database_confirmed = false;
        self.inner.stream.context_tracking_initialized = false;
        use crate::executor::Executor;
        self.execute(
            "SET @@session.session_track_schema=ON, @@session.session_track_system_variables='*'",
        )
        .await?;
        self.inner.stream.context_tracking_initialized = true;
        self.inner.stream.schema_tracking_enabled = true;
        self.inner.stream.system_tracking_all = true;
        Ok(true)
    }

    /// Record the value obtained by an explicit database inspection on this
    /// connection. Caller must have verified that query successfully completed.
    pub fn confirm_current_database(&mut self, database: Option<String>) {
        self.inner.stream.current_database = database;
        self.inner.stream.database_confirmed = true;
    }

    /// Current database from successful schema-change packets, after draining
    /// the pending response. None means absent/unobserved; never executes SQL.
    pub async fn observed_current_database(&mut self) -> Result<Option<String>, Error> {
        self.inner.stream.wait_until_ready().await?;
        let stream = &self.inner.stream;
        if !stream.context_tracking_initialized
            || !stream.schema_tracking_enabled
            || !stream.system_tracking_all
            || !stream.database_confirmed
        {
            return Ok(None);
        }
        Ok(stream.current_database.clone())
    }

    /// The immutable server connection id decoded from the original handshake.
    pub fn server_connection_id(&self) -> u32 {
        self.inner.stream.connection_id
    }

    /// Latest server status from the response protocol, without issuing SQL.
    /// ERR packets do not carry status, so return None after an error rather
    /// than guessing whether a deadlock or cancellation rolled back a transaction.
    pub async fn observed_in_transaction(&mut self) -> Result<Option<bool>, Error> {
        self.inner.stream.wait_until_ready().await?;
        Ok(self
            .inner
            .stream
            .observed_status
            .map(|status| status.contains(Status::SERVER_STATUS_IN_TRANS)))
    }

    /// Observes metadata and successful completions of subsequent query streams.
    ///
    /// The handler is called synchronously and should return promptly. It must
    /// not panic. Pass `None` to stop observing events. The handler remains set
    /// until replaced or the connection is dropped.
    pub fn set_query_event_handler(
        &mut self,
        handler: Option<Arc<dyn Fn(MySqlQueryEvent) + Send + Sync>>,
    ) {
        self.inner.query_event_handler = handler;
    }

    pub(crate) fn emit_query_event(&self, event: MySqlQueryEvent) {
        if let Some(handler) = &self.inner.query_event_handler {
            handler(event);
        }
    }

    pub(crate) fn in_transaction(&self) -> bool {
        self.inner
            .status_flags
            .intersects(Status::SERVER_STATUS_IN_TRANS)
    }
}

impl Debug for MySqlConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MySqlConnection").finish()
    }
}

impl Connection for MySqlConnection {
    type Database = MySql;

    type Options = MySqlConnectOptions;

    fn close(mut self) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(async move {
            self.inner.stream.send_packet(Quit).await?;
            self.inner.stream.shutdown().await?;

            Ok(())
        })
    }

    fn close_hard(mut self) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(async move {
            self.inner.stream.shutdown().await?;
            Ok(())
        })
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.inner.stream.wait_until_ready().await?;
            self.inner.stream.send_packet(Ping).await?;
            self.inner.stream.recv_ok().await?;

            Ok(())
        })
    }

    #[doc(hidden)]
    fn flush(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        self.inner.stream.wait_until_ready().boxed()
    }

    fn cached_statements_size(&self) -> usize {
        self.inner.cache_statement.len()
    }

    fn clear_cached_statements(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            while let Some((statement_id, _)) = self.inner.cache_statement.remove_lru() {
                self.inner
                    .stream
                    .send_packet(StmtClose {
                        statement: statement_id,
                    })
                    .await?;
            }

            Ok(())
        })
    }

    #[doc(hidden)]
    fn should_flush(&self) -> bool {
        !self.inner.stream.write_buffer().is_empty()
    }

    fn begin(&mut self) -> BoxFuture<'_, Result<Transaction<'_, Self::Database>, Error>>
    where
        Self: Sized,
    {
        Transaction::begin(self, None)
    }

    fn begin_with(
        &mut self,
        statement: impl Into<Cow<'static, str>>,
    ) -> BoxFuture<'_, Result<Transaction<'_, Self::Database>, Error>>
    where
        Self: Sized,
    {
        Transaction::begin(self, Some(statement.into()))
    }

    fn shrink_buffers(&mut self) {
        self.inner.stream.shrink_buffers();
    }
}
