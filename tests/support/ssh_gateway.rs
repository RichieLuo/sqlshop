//! A real SSH endpoint forwarding only to the selected local test database.
use anyhow::Result;
use russh::{
    Channel, ChannelOpenFailure,
    keys::{Algorithm, HashAlg, PrivateKey, key::safe_rng},
    server::{self, Auth, Server as _},
};
use sqlshop::model::SshConfig;
use std::{sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

pub const PASSWORD: &str = "sqlshop-local-ssh-test";

#[derive(Clone)]
struct Handler {
    host: String,
    port: u16,
}

impl server::Server for Handler {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for Handler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth> {
        Ok(if user == "sqlshop-test" && password == PASSWORD {
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
        if host != self.host || port != u32::from(self.port) {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let mut target = TcpStream::connect((self.host.as_str(), self.port)).await?;
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut target).await;
        });
        Ok(())
    }
}

pub struct Gateway {
    pub config: SshConfig,
    task: JoinHandle<()>,
}

impl Gateway {
    pub async fn start(host: &str, port: u16) -> Result<Self> {
        let key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519)?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let config = SshConfig {
            enabled: true,
            host: "127.0.0.1".into(),
            port: listener.local_addr()?.port(),
            username: "sqlshop-test".into(),
            fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
            known_hosts: String::new(),
            remember_secret: false,
            ..Default::default()
        };
        let options = server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        };
        let mut handler = Handler {
            host: host.into(),
            port,
        };
        let task = tokio::spawn(async move {
            let _ = handler.run_on_socket(Arc::new(options), &listener).await;
        });
        Ok(Self { config, task })
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}
