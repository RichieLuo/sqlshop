use crate::model::{SshAuth, SshConfig};
use anyhow::{Context, Result, bail};
use russh::{
    client,
    keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use std::{
    net::{Shutdown, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

pub fn expand_path(path: &str) -> Result<PathBuf> {
    let path = path.trim();
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        let home = directories::UserDirs::new().context("无法定位用户目录")?;
        Ok(home.home_dir().join(path.get(2..).unwrap_or("")))
    } else {
        Ok(PathBuf::from(path))
    }
}

pub fn private_key_path(path: &str) -> Result<PathBuf> {
    let path = expand_path(path)?;
    anyhow::ensure!(
        path.extension().is_none_or(|extension| extension != "pub"),
        "请选择 SSH 私钥文件，而不是 .pub 公钥文件：{}",
        path.display()
    );
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(
            "SSH 私钥文件不存在：{}；请使用“选择文件…”重新选择",
            path.display()
        ),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法访问 SSH 私钥文件：{}", path.display()));
        }
    };
    anyhow::ensure!(
        metadata.is_file(),
        "SSH 私钥路径必须是文件：{}",
        path.display()
    );
    Ok(path)
}

struct HostVerifier {
    config: SshConfig,
}

impl client::Handler for HostVerifier {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            bail!("暂不支持 SSH 主机证书，请使用普通 SSH 主机公钥");
        };
        let actual = key.fingerprint(HashAlg::Sha256).to_string();
        let expected = self.config.fingerprint.trim();
        if !expected.is_empty() {
            anyhow::ensure!(
                actual == expected,
                "SSH 服务器指纹不匹配；预期 {expected}，实际 {actual}"
            );
            return Ok(true);
        }
        let path = expand_path(&self.config.known_hosts)?;
        match keys::check_known_hosts_path(&self.config.host, self.config.port, key, &path) {
            Ok(true) => Ok(true),
            Ok(false) => bail!(
                "SSH 主机尚未受信任，服务器指纹为 {actual}。核实后在 SSH 配置中填写此指纹，或将服务器加入 {}。",
                path.display()
            ),
            Err(error) => {
                Err(error).context(format!("SSH 主机密钥校验失败，服务器指纹为 {actual}"))
            }
        }
    }
}

// Holding a duplicate descriptor lets cancellation close the underlying SSH
// transport immediately, even while russh's own session task is running.
struct SocketGuard(std::net::TcpStream);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

pub struct Tunnel {
    pub local_addr: SocketAddr,
    task: JoinHandle<()>,
    _transport: SocketGuard,
}

impl Tunnel {
    pub async fn open(
        config: &SshConfig,
        secret: &str,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<Self> {
        config.validate()?;
        anyhow::ensure!(
            !remote_host.trim().is_empty() && remote_port > 0,
            "数据库目标地址或端口无效"
        );
        let key_path = if config.auth == SshAuth::PrivateKey {
            Some(private_key_path(&config.private_key)?)
        } else {
            None
        };
        let tcp = TcpStream::connect((config.host.as_str(), config.port))
            .await
            .context("连接 SSH 服务器失败")?;
        tcp.set_nodelay(true)?;
        let tcp = tcp.into_std()?;
        let transport = SocketGuard(tcp.try_clone()?);
        let tcp = TcpStream::from_std(tcp)?;
        let options = client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        };
        let mut session = client::connect_stream(
            Arc::new(options),
            tcp,
            HostVerifier {
                config: config.clone(),
            },
        )
        .await
        .context("SSH 握手失败")?;
        let auth = match config.auth {
            SshAuth::Password => session
                .authenticate_password(&config.username, secret)
                .await
                .context("SSH 密码认证失败")?,
            SshAuth::PrivateKey => {
                let path = key_path.context("请指定 SSH 私钥文件")?;
                let passphrase = (!secret.is_empty()).then(|| secret.to_owned());
                let display_path = path.display().to_string();
                let key = tokio::task::spawn_blocking(move || {
                    keys::load_secret_key(path, passphrase.as_deref())
                })
                .await
                .context("SSH 私钥读取任务失败")?
                .with_context(|| {
                    format!("读取 SSH 私钥失败：{display_path}；请检查文件和私钥口令")
                })?;
                let hash = session
                    .best_supported_rsa_hash()
                    .await
                    .context("协商 SSH 密钥算法失败")?
                    .flatten();
                session
                    .authenticate_publickey(
                        &config.username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                    .context("SSH 私钥认证失败")?
            }
        };
        anyhow::ensure!(
            auth.success(),
            "SSH 认证被服务器拒绝，请检查用户名和认证信息"
        );
        let channel = session
            .channel_open_direct_tcpip(remote_host, u32::from(remote_port), "127.0.0.1", 0)
            .await
            .context("SSH 无法访问目标数据库，请检查数据库地址及服务器的 TCP 转发权限")?;
        // A database Session owns one TCP connection. Accept just that connection,
        // then close the listener to keep other processes from reusing the tunnel.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            if let Ok((mut local, _)) = listener.accept().await {
                drop(listener);
                let mut remote = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
            }
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;
        });
        Ok(Self {
            local_addr,
            task,
            _transport: transport,
        })
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}
