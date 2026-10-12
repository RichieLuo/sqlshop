use anyhow::{Context, Result};
use russh::{
    Channel, ChannelOpenFailure,
    keys::{Algorithm, HashAlg, PrivateKey, PublicKey, key::safe_rng},
    server::{self, Auth, Server as _},
};
use sqlshop::{
    model::{SshAuth, SshConfig},
    ssh::Tunnel,
};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

#[derive(Clone)]
struct TestServer {
    client_key: PublicKey,
    target: SocketAddr,
    auth_attempts: Arc<AtomicUsize>,
    allow_forward: bool,
}

impl server::Server for TestServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for TestServer {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth> {
        self.auth_attempts.fetch_add(1, Ordering::SeqCst);
        Ok(
            if user == "sqlshop-test" && password == "local-test-password" {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth> {
        self.auth_attempts.fetch_add(1, Ordering::SeqCst);
        Ok(if user == "sqlshop-test" && key == &self.client_key {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<()> {
        if !self.allow_forward || host != "127.0.0.1" || port != u32::from(self.target.port()) {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let mut target = TcpStream::connect(self.target).await?;
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut target).await;
        });
        Ok(())
    }
}

struct Fixture {
    config: SshConfig,
    target: SocketAddr,
    client_key: PrivateKey,
    auth_attempts: Arc<AtomicUsize>,
    known_hosts: String,
    server_task: JoinHandle<()>,
    echo_task: JoinHandle<()>,
    directory: tempfile::TempDir,
}

impl Fixture {
    async fn start(allow_forward: bool) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let echo = TcpListener::bind(("127.0.0.1", 0)).await?;
        let target = echo.local_addr()?;
        let echo_task = tokio::spawn(async move {
            while let Ok((stream, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let (mut reader, mut writer) = stream.into_split();
                    let _ = tokio::io::copy(&mut reader, &mut writer).await;
                    let _ = writer.shutdown().await;
                });
            }
        });
        let host_key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519)?;
        let client_key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519)?;
        let ssh = TcpListener::bind(("127.0.0.1", 0)).await?;
        let ssh_addr = ssh.local_addr()?;
        let known_hosts = format!(
            "[127.0.0.1]:{} {}\n",
            ssh_addr.port(),
            host_key.public_key().to_openssh()?
        );
        let config = SshConfig {
            enabled: true,
            host: "127.0.0.1".into(),
            port: ssh_addr.port(),
            username: "sqlshop-test".into(),
            fingerprint: host_key
                .public_key()
                .fingerprint(HashAlg::Sha256)
                .to_string(),
            known_hosts: directory.path().join("known_hosts").display().to_string(),
            ..Default::default()
        };
        let options = server::Config {
            keys: vec![host_key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        };
        let auth_attempts = Arc::new(AtomicUsize::new(0));
        let mut server = TestServer {
            client_key: client_key.public_key().clone(),
            target,
            auth_attempts: auth_attempts.clone(),
            allow_forward,
        };
        let server_task = tokio::spawn(async move {
            let _ = server.run_on_socket(Arc::new(options), &ssh).await;
        });
        Ok(Self {
            config,
            target,
            client_key,
            auth_attempts,
            known_hosts,
            server_task,
            echo_task,
            directory,
        })
    }

    async fn open(&self, config: &SshConfig, secret: &str) -> Result<Tunnel> {
        tokio::time::timeout(
            Duration::from_secs(10),
            Tunnel::open(config, secret, "127.0.0.1", self.target.port()),
        )
        .await
        .context("Test tunnel timed out")?
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server_task.abort();
        self.echo_task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_stalled_handshake_closes_the_ssh_socket() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let observer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut received = Vec::new();
        socket.read_to_end(&mut received).await?;
        Ok::<_, std::io::Error>(())
    });
    let config = SshConfig {
        enabled: true,
        host: "127.0.0.1".into(),
        port: address.port(),
        ..Default::default()
    };
    let cancelled = tokio::time::timeout(
        Duration::from_millis(150),
        Tunnel::open(&config, "", "127.0.0.1", 3306),
    )
    .await;
    assert!(cancelled.is_err());
    tokio::time::timeout(Duration::from_secs(2), observer).await???;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_tunnel_forwards_large_binary_payload_and_closes_on_drop() -> Result<()> {
    let fixture = Fixture::start(true).await?;
    let tunnel = fixture.open(&fixture.config, "local-test-password").await?;
    let address = tunnel.local_addr;
    assert!(address.ip().is_loopback());
    let mut client = TcpStream::connect(address).await?;
    let payload: Vec<u8> = (0..262_144).map(|i| (i % 256) as u8).collect();
    let mut echoed = vec![0; payload.len()];
    let (mut reader, mut writer) = client.split();
    tokio::time::timeout(Duration::from_secs(10), async {
        let (sent, read) = tokio::join!(writer.write_all(&payload), reader.read_exact(&mut echoed));
        sent?;
        read?;
        Ok::<_, std::io::Error>(())
    })
    .await??;
    assert_eq!(payload, echoed);
    assert_eq!(fixture.auth_attempts.load(Ordering::SeqCst), 1);
    // The single-use listener closes after the database has connected.
    assert!(TcpStream::connect(address).await.is_err());
    drop(tunnel);
    let mut byte = [0];
    let read = tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte)).await?;
    assert!(matches!(read, Ok(0) | Err(_)));
    assert!(TcpStream::connect(address).await.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_key_authentication_supports_encrypted_and_unencrypted_keys() -> Result<()> {
    let fixture = Fixture::start(true).await?;
    let path = fixture.directory.path().join("id_ed25519");
    let mut config = fixture.config.clone();
    config.auth = SshAuth::PrivateKey;
    config.private_key = path.display().to_string();
    fixture
        .client_key
        .write_openssh_file(&path, russh::keys::ssh_key::LineEnding::LF)?;
    let unencrypted = fixture.open(&config, "").await?;
    drop(unencrypted);
    fixture
        .client_key
        .encrypt(&mut safe_rng(), "key-passphrase")?
        .write_openssh_file(&path, russh::keys::ssh_key::LineEnding::LF)?;
    let encrypted = fixture.open(&config, "key-passphrase").await?;
    drop(encrypted);
    assert_eq!(fixture.auth_attempts.load(Ordering::SeqCst), 2);
    assert!(fixture.open(&config, "wrong-passphrase").await.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_hosts_accepts_matching_host_and_rejects_changed_key_before_auth() -> Result<()> {
    let fixture = Fixture::start(true).await?;
    let mut config = fixture.config.clone();
    config.fingerprint.clear();
    std::fs::write(&config.known_hosts, &fixture.known_hosts)?;
    drop(fixture.open(&config, "local-test-password").await?);
    let wrong_key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519)?;
    std::fs::write(
        &config.known_hosts,
        format!(
            "[127.0.0.1]:{} {}\n",
            config.port,
            wrong_key.public_key().to_openssh()?
        ),
    )?;
    let error = fixture
        .open(&config, "local-test-password")
        .await
        .err()
        .context("Expected host key mismatch")?;
    assert!(format!("{error:#}").contains("SSH 主机密钥校验失败"));
    assert_eq!(fixture.auth_attempts.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_host_and_wrong_fingerprint_never_send_credentials() -> Result<()> {
    let fixture = Fixture::start(true).await?;
    let mut config = fixture.config.clone();
    config.fingerprint.clear();
    let unknown = fixture
        .open(&config, "local-test-password")
        .await
        .err()
        .context("Expected unknown host rejection")?;
    assert!(format!("{unknown:#}").contains(&fixture.config.fingerprint));
    config.fingerprint = "SHA256:wrong".into();
    let mismatch = fixture
        .open(&config, "local-test-password")
        .await
        .err()
        .context("Expected fingerprint rejection")?;
    assert!(format!("{mismatch:#}").contains("指纹不匹配"));
    assert_eq!(fixture.auth_attempts.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_password_and_disabled_forwarding_are_reported() -> Result<()> {
    let fixture = Fixture::start(false).await?;
    let auth = fixture
        .open(&fixture.config, "wrong-password")
        .await
        .err()
        .context("Expected authentication rejection")?;
    assert!(format!("{auth:#}").contains("SSH 认证被服务器拒绝"));
    let forwarding = fixture
        .open(&fixture.config, "local-test-password")
        .await
        .err()
        .context("Expected forwarding rejection")?;
    assert!(format!("{forwarding:#}").contains("SSH 无法访问目标数据库"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_dropped_before_database_connection_releases_listener() -> Result<()> {
    let fixture = Fixture::start(true).await?;
    let tunnel = fixture.open(&fixture.config, "local-test-password").await?;
    let address = tunnel.local_addr;
    drop(tunnel);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if TcpListener::bind(address).await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}
