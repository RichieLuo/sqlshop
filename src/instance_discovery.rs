//! SQL Browser unicast instance discovery. No credentials are sent over UDP.
use anyhow::{Context, Result};
use std::time::Duration;
use tokio::net::{TcpStream, UdpSocket};

pub async fn connect_instance(host: &str, instance: &str) -> Result<(TcpStream, u16)> {
    let mut last = None;
    for address in tokio::net::lookup_host((host, 1434)).await? {
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            let bind = if address.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let socket = UdpSocket::bind(bind).await?;
            // A connected UDP socket accepts only this peer's reply.
            socket.connect(address).await?;
            let mut request = vec![4];
            request.extend_from_slice(instance.as_bytes());
            request.push(0);
            socket.send(&request).await?;
            let mut reply = [0u8; 4096];
            let length = socket.recv(&mut reply).await?;
            anyhow::ensure!(length >= 3 && reply[0] == 5, "SQL Browser 返回无效响应");
            let payload_length = u16::from_le_bytes([reply[1], reply[2]]) as usize;
            anyhow::ensure!(
                payload_length == length - 3,
                "SQL Browser 响应长度不符或被截断"
            );
            let payload = std::str::from_utf8(&reply[3..length])?.trim_end_matches('\0');
            let parts: Vec<_> = payload.split(';').collect();
            let mut name = None;
            let mut port = None;
            for pair in parts.chunks_exact(2) {
                if pair[0].eq_ignore_ascii_case("InstanceName") {
                    name = Some(pair[1]);
                }
                if pair[0].eq_ignore_ascii_case("tcp") {
                    port = Some(pair[1].parse::<u16>()?);
                }
            }
            anyhow::ensure!(
                name.is_some_and(|name| name.eq_ignore_ascii_case(instance)),
                "SQL Browser 返回其他实例，连接未建立"
            );
            let port = port
                .filter(|port| *port > 0)
                .context("实例未公布有效 TCP 端口")?;
            let mut target = address;
            target.set_port(port);
            Ok::<_, anyhow::Error>((TcpStream::connect(target).await?, port))
        })
        .await;
        match result {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => last = Some(error),
            Err(_) => last = Some(anyhow::anyhow!("SQL Browser 发现或连接超时")),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("无法解析 SQL Server 地址")))
}
