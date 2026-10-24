//! PostgreSQL CancelRequest transports close without a response. These tests
//! cover EOF and Windows-style reset after a complete request, not whether a
//! running SQL statement was cancelled; the original query must confirm that.
use anyhow::Result;
use sqlx::{PgConnection, postgres::PgConnectOptions};
use std::{sync::atomic::AtomicBool, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Clone, Copy)]
enum ServerClose {
    Eof,
    Reset,
    UnexpectedReply,
}

async fn cancel_request(close: ServerClose) -> Result<bool> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = [0; 16];
        socket.read_exact(&mut request).await?;
        assert_eq!(&request[..4], &16u32.to_be_bytes());
        assert_eq!(&request[4..8], &80877102u32.to_be_bytes());
        assert_eq!(&request[8..12], &123u32.to_be_bytes());
        assert_eq!(&request[12..], &456u32.to_be_bytes());
        match close {
            ServerClose::Eof => {}
            ServerClose::Reset => {
                // A zero linger discards the socket with RST; it cannot block
                // the runtime while waiting for unsent data to drain.
                #[allow(deprecated)]
                socket.set_linger(Some(Duration::ZERO))?;
            }
            ServerClose::UnexpectedReply => socket.write_all(b"!").await?,
        }
        drop(socket);
        Ok::<_, std::io::Error>(())
    });
    let options = PgConnectOptions::new()
        .host("127.0.0.1")
        .port(port)
        .ssl_mode(sqlx::postgres::PgSslMode::Disable);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        PgConnection::cancel_request_if_active(&options, (123, 456), &AtomicBool::new(true)),
    )
    .await?;
    server.await??;
    Ok(result?)
}

#[tokio::test]
async fn cancel_transport_accepts_eof_after_request_flush() -> Result<()> {
    assert!(cancel_request(ServerClose::Eof).await?);
    Ok(())
}

#[tokio::test]
async fn cancel_transport_accepts_reset_after_request_flush() -> Result<()> {
    assert!(cancel_request(ServerClose::Reset).await?);
    Ok(())
}

#[tokio::test]
async fn cancel_transport_rejects_unexpected_reply() {
    let error = cancel_request(ServerClose::UnexpectedReply)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<sqlx::Error>(),
            Some(sqlx::Error::Protocol(_))
        ),
        "{error:#}"
    );
}
