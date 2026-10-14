use crate::collation::{CharSet, Collation};
use crate::connection::{MySqlStream, Waiting};
use crate::error::Error;
use crate::net::tls::TlsConfig;
use crate::net::{tls, BufferedSocket, Socket, WithSocket};
use crate::protocol::connect::SslRequest;
use crate::protocol::Capabilities;
use crate::{MySqlConnectOptions, MySqlSslMode};
use std::collections::VecDeque;

struct MapStream {
    server_version: (u16, u16, u16),
    capabilities: Capabilities,
    sequence_id: u8,
    waiting: VecDeque<Waiting>,
    charset: CharSet,
    collation: Collation,
    connection_id: u32,
    observed_status: Option<crate::protocol::response::Status>,
    current_database: Option<String>,
    database_confirmed: bool,
    schema_tracking_enabled: bool,
    system_tracking_all: bool,
    context_tracking_initialized: bool,
}

pub(super) async fn maybe_upgrade<S: Socket>(
    mut stream: MySqlStream<S>,
    options: &MySqlConnectOptions,
) -> Result<MySqlStream, Error> {
    let server_supports_tls = stream.capabilities.contains(Capabilities::SSL);

    if matches!(options.ssl_mode, MySqlSslMode::Disabled) || !tls::available() {
        // remove the SSL capability if SSL has been explicitly disabled
        stream.capabilities.remove(Capabilities::SSL);
    }

    // https://www.postgresql.org/docs/12/libpq-ssl.html#LIBPQ-SSL-SSLMODE-STATEMENTS
    match options.ssl_mode {
        MySqlSslMode::Disabled => return Ok(stream.boxed_socket()),

        MySqlSslMode::Preferred => {
            if !tls::available() {
                // Client doesn't support TLS
                tracing::debug!("not performing TLS upgrade: TLS support not compiled in");
                return Ok(stream.boxed_socket());
            }

            if !server_supports_tls {
                // Server doesn't support TLS
                tracing::debug!("not performing TLS upgrade: unsupported by server");
                return Ok(stream.boxed_socket());
            }
        }

        MySqlSslMode::Required | MySqlSslMode::VerifyIdentity | MySqlSslMode::VerifyCa => {
            tls::error_if_unavailable()?;

            if !server_supports_tls {
                // upgrade failed, die
                return Err(Error::Tls("server does not support TLS".into()));
            }
        }
    }

    let tls_config = TlsConfig {
        accept_invalid_certs: !matches!(
            options.ssl_mode,
            MySqlSslMode::VerifyCa | MySqlSslMode::VerifyIdentity
        ),
        accept_invalid_hostnames: !matches!(options.ssl_mode, MySqlSslMode::VerifyIdentity),
        hostname: &options.host,
        root_cert_path: options.ssl_ca.as_ref(),
        client_cert_path: options.ssl_client_cert.as_ref(),
        client_key_path: options.ssl_client_key.as_ref(),
    };

    // Request TLS upgrade
    stream.write_packet(SslRequest {
        max_packet_size: super::MAX_PACKET_SIZE,
        collation: stream.collation as u8,
    })?;

    stream.flush().await?;

    tls::handshake(
        stream.socket.into_inner(),
        tls_config,
        MapStream {
            server_version: stream.server_version,
            capabilities: stream.capabilities,
            sequence_id: stream.sequence_id,
            waiting: stream.waiting,
            charset: stream.charset,
            collation: stream.collation,
            connection_id: stream.connection_id,
            observed_status: stream.observed_status,
            current_database: stream.current_database,
            database_confirmed: stream.database_confirmed,
            schema_tracking_enabled: stream.schema_tracking_enabled,
            system_tracking_all: stream.system_tracking_all,
            context_tracking_initialized: stream.context_tracking_initialized,
        },
    )
    .await
}

impl WithSocket for MapStream {
    type Output = MySqlStream;

    async fn with_socket<S: Socket>(self, socket: S) -> Self::Output {
        MySqlStream {
            socket: BufferedSocket::new(Box::new(socket)),
            server_version: self.server_version,
            capabilities: self.capabilities,
            sequence_id: self.sequence_id,
            waiting: self.waiting,
            charset: self.charset,
            collation: self.collation,
            is_tls: true,
            connection_id: self.connection_id,
            observed_status: self.observed_status,
            current_database: self.current_database,
            database_confirmed: self.database_confirmed,
            schema_tracking_enabled: self.schema_tracking_enabled,
            system_tracking_all: self.system_tracking_all,
            context_tracking_initialized: self.context_tracking_initialized,
        }
    }
}
