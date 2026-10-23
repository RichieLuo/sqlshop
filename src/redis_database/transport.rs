use super::*;
use redis::{Cmd, FromRedisValue, ProtocolVersion};
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, crypto::ring, pki_types::ServerName},
};

// The redis crate owns RESP decoding. This adapter enforces limits BEFORE that
// decoder can allocate a huge bulk string/array (SCAN COUNT is only a hint).
struct BoundedStream<S> {
    inner: S,
    bytes: Arc<AtomicUsize>,
    values: Arc<AtomicUsize>,
    line: Vec<u8>,
    bulk_remaining: usize,
}
impl<S> BoundedStream<S> {
    fn inspect(&mut self, bytes: &[u8]) -> io::Result<()> {
        fn limit() -> io::Error {
            io::Error::other("Redis 响应超过安全上限，连接已关闭")
        }
        if self.bytes.fetch_add(bytes.len(), Ordering::Relaxed) + bytes.len() > MAX_WIRE_BYTES {
            return Err(limit());
        }
        for &byte in bytes {
            if self.bulk_remaining > 0 {
                self.bulk_remaining -= 1;
                continue;
            }
            self.line.push(byte);
            if self.line.len() > 4096 {
                return Err(limit());
            }
            if !self.line.ends_with(b"\r\n") {
                continue;
            }
            if self.values.fetch_add(1, Ordering::Relaxed) >= 4096 {
                return Err(limit());
            }
            let line = &self.line[..self.line.len() - 2];
            let prefix = *line.first().ok_or_else(limit)?;
            if matches!(prefix, b'$' | b'*') {
                let count: i64 = std::str::from_utf8(&line[1..])
                    .map_err(|_| limit())?
                    .parse()
                    .map_err(|_| limit())?;
                if count < -1 {
                    return Err(limit());
                }
                if prefix == b'$' && count >= 0 {
                    if count as u64 > MAX_WIRE_BYTES as u64 {
                        return Err(limit());
                    }
                    self.bulk_remaining = count as usize + 2;
                } else if prefix == b'*' && count > 2048 {
                    return Err(limit());
                }
            } else if !matches!(prefix, b'+' | b'-' | b':') {
                return Err(io::Error::other("仅支持受限 RESP2 响应"));
            }
            self.line.clear();
        }
        Ok(())
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for BoundedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let start = buffer.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buffer) {
            Poll::Ready(Ok(())) => Poll::Ready(self.inspect(&buffer.filled()[start..])),
            other => other,
        }
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for BoundedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub(super) struct NamespaceSnapshot {
    pub token: u64,
    pub database: u32,
    pub prefix: Vec<u8>,
    pub keys: Vec<Vec<u8>>,
}

pub(super) struct RedisConnection {
    connection: MultiplexedConnection,
    driver: JoinHandle<()>,
    _tunnel: Option<Tunnel>,
    bytes: Arc<AtomicUsize>,
    values: Arc<AtomicUsize>,
    pub broken: bool,
    pub write_started: bool,
    // Only an untruncated, complete read is eligible for optimistic string save.
    pub strings: std::collections::HashMap<Vec<u8>, Vec<u8>>,
    pub namespace: Option<NamespaceSnapshot>,
}
impl Drop for RedisConnection {
    fn drop(&mut self) {
        self.driver.abort();
    }
}
impl RedisConnection {
    pub async fn open(profile: &RedisProfile, credentials: &Credentials) -> Result<Self> {
        profile.validate()?;
        let future = async {
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
            let tcp = if let Some(tunnel) = &tunnel {
                TcpStream::connect(tunnel.local_addr).await
            } else {
                TcpStream::connect((profile.host.trim().trim_matches(['[', ']']), profile.port))
                    .await
            }
            .context("连接 Redis TCP 失败")?;
            tcp.set_nodelay(true)?;
            let mut settings = RedisConnectionInfo::default()
                // Legacy profile.database is serialization compatibility only.
                // RESP connections start at DB 0; only SelectDb may change it.
                .set_db(0)
                .set_protocol(ProtocolVersion::RESP2)
                .set_skip_set_lib_name();
            if !profile.username.is_empty() {
                // An ACL account with an empty password still needs explicit AUTH.
                settings = settings
                    .set_username(&profile.username)
                    .set_password(&credentials.database_password);
            } else if !credentials.database_password.is_empty() {
                settings = settings.set_password(&credentials.database_password);
            }
            if profile.tls {
                let mut roots = RootCertStore::empty();
                let certificates = rustls_native_certs::load_native_certs();
                for certificate in certificates.certs {
                    let _ = roots.add(certificate);
                }
                anyhow::ensure!(!roots.is_empty(), "无法加载系统 TLS 根证书，未关闭证书校验");
                let config =
                    ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
                        .with_safe_default_protocol_versions()?
                        .with_root_certificates(roots)
                        .with_no_client_auth();
                let name =
                    ServerName::try_from(profile.host.trim().trim_matches(['[', ']']).to_owned())
                        .context("TLS 主机名无效")?;
                let stream = TlsConnector::from(Arc::new(config))
                    .connect(name, tcp)
                    .await
                    .context("Redis TLS 握手失败（必须通过系统证书链及主机名校验）")?;
                Self::from_stream(stream, settings, tunnel).await
            } else {
                Self::from_stream(tcp, settings, tunnel).await
            }
        };
        tokio::time::timeout(TIMEOUT, future)
            .await
            .context("Redis/SSH 连接超时，未自动重连")?
    }
    async fn from_stream<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        stream: S,
        settings: RedisConnectionInfo,
        tunnel: Option<Tunnel>,
    ) -> Result<Self> {
        let bytes = Arc::new(AtomicUsize::new(0));
        let values = Arc::new(AtomicUsize::new(0));
        let bounded = BoundedStream {
            inner: stream,
            bytes: bytes.clone(),
            values: values.clone(),
            line: Vec::new(),
            bulk_remaining: 0,
        };
        let config = AsyncConnectionConfig::new()
            .set_response_timeout(Some(TIMEOUT))
            .set_connection_timeout(Some(TIMEOUT))
            .set_pipeline_buffer_size(1)
            .set_concurrency_limit(1);
        let (connection, driver) =
            MultiplexedConnection::new_with_config(&settings, bounded, config)
                .await
                // Never echo AUTH arguments or a server-provided credential diagnostic.
                .map_err(|error| {
                    anyhow::anyhow!(
                        "Redis 握手/认证/数据库选择失败（{:?}），请核实 AUTH/ACL 配置",
                        error.kind()
                    )
                })?;
        let driver = tokio::spawn(driver);
        Ok(Self {
            connection,
            driver,
            _tunnel: tunnel,
            bytes,
            values,
            broken: false,
            write_started: false,
            strings: Default::default(),
            namespace: None,
        })
    }
    pub async fn query<T: FromRedisValue>(&mut self, command: &Cmd, write: bool) -> Result<T> {
        anyhow::ensure!(!self.broken, "Redis 连接已失效，请手动重新连接");
        if write {
            // Confirmation tokens never survive any attempted data write.
            self.namespace = None;
        }
        self.bytes.store(0, Ordering::Relaxed);
        self.values.store(0, Ordering::Relaxed);
        self.write_started |= write;
        match tokio::time::timeout(TIMEOUT, command.query_async(&mut self.connection)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                if error.is_io_error()
                    || error.is_timeout()
                    || error.is_unrecoverable_error()
                    || matches!(error.kind(), redis::ErrorKind::Parse)
                {
                    self.broken = true;
                }
                // Bound even server diagnostics, without including the original command.
                Err(anyhow::anyhow!(
                    "Redis 操作失败：{}",
                    error.to_string().chars().take(1024).collect::<String>()
                ))
            }
            Err(_) => {
                self.broken = true;
                bail!("Redis 操作超时，连接已关闭")
            }
        }
    }
    pub async fn kind(&mut self, key: &[u8]) -> Result<RedisKind> {
        let name: String = self.query(redis::cmd("TYPE").arg(key), false).await?;
        anyhow::ensure!(name != "none", "Key 不存在或已过期");
        Ok(RedisKind::from_label(name))
    }
}
