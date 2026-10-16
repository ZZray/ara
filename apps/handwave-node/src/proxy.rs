use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig},
    MaybeTlsStream, WebSocketStream,
};
use url::Url;

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub fn bypass_proxy(host: &str, port: u16, rules: &str) -> bool {
    rules.split(',').map(str::trim).any(|rule| {
        if rule == "*" {
            return true;
        }
        if rule.is_empty() {
            return false;
        }
        let (name, specified_port) = match rule.rsplit_once(':') {
            Some((name, p)) if p.parse::<u16>().is_ok() => (name, p.parse::<u16>().ok()),
            _ => (rule, None),
        };
        if specified_port.is_some_and(|p| p != port) {
            return false;
        }
        let name = name.trim_start_matches('.').to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        host == name || host.ends_with(&format!(".{name}"))
    })
}

fn proxy_for(url: &Url) -> Option<String> {
    // The plaintext development exception means this computer, never a
    // proxy's loopback interface. Secure public connections may still proxy.
    if crate::transport_security::plaintext_loopback(url) {
        return None;
    }
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    let no_proxy = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")).unwrap_or_default();
    if bypass_proxy(host, port, &no_proxy) {
        return None;
    }
    let key = if url.scheme() == "wss" { "HTTPS_PROXY" } else { "HTTP_PROXY" };
    std::env::var(key.to_ascii_lowercase())
        .or_else(|_| std::env::var(key))
        .or_else(|_| std::env::var("all_proxy"))
        .or_else(|_| std::env::var("ALL_PROXY"))
        .ok()
        .filter(|p| !p.is_empty())
}

pub async fn connect(url: &Url, token: &str) -> Result<Socket> {
    crate::transport_security::validate(url)?;
    let host = url.host_str().context("missing hub host")?;
    let port = url.port_or_known_default().context("missing hub port")?;
    let socket = match proxy_for(url) {
        None => TcpStream::connect((host, port)).await.context("hub TCP connection failed")?,
        Some(proxy) => {
            let proxy = Url::parse(&proxy).context("invalid proxy URL")?;
            let username = percent_encoding::percent_decode_str(proxy.username()).decode_utf8()?.into_owned();
            let password =
                percent_encoding::percent_decode_str(proxy.password().unwrap_or("")).decode_utf8()?.into_owned();
            let phost = proxy.host_str().context("missing proxy host")?;
            let pport = proxy
                .port_or_known_default()
                .or_else(|| matches!(proxy.scheme(), "socks5" | "socks5h").then_some(1080))
                .context("missing proxy port")?;
            match proxy.scheme() {
                "socks5" | "socks5h" => {
                    let conn = if username.is_empty() {
                        tokio_socks::tcp::Socks5Stream::connect((phost, pport), (host, port)).await?
                    } else {
                        tokio_socks::tcp::Socks5Stream::connect_with_password(
                            (phost, pport),
                            (host, port),
                            &username,
                            &password,
                        )
                        .await?
                    };
                    conn.into_inner()
                }
                "http" => {
                    let mut socket = TcpStream::connect((phost, pport)).await.context("proxy connection failed")?;
                    let authority =
                        if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
                    let auth = if username.is_empty() {
                        String::new()
                    } else {
                        format!("Proxy-Authorization: Basic {}\r\n", STANDARD.encode(format!("{username}:{password}")))
                    };
                    socket
                        .write_all(
                            format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{auth}\r\n").as_bytes(),
                        )
                        .await?;
                    let mut header = Vec::new();
                    loop {
                        anyhow::ensure!(header.len() < 16 * 1024, "proxy header too large");
                        header.push(socket.read_u8().await.context("proxy closed CONNECT tunnel")?);
                        if header.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&header);
                    anyhow::ensure!(
                        text.lines().next().and_then(|s| s.split_whitespace().nth(1)) == Some("200"),
                        "proxy refused CONNECT tunnel"
                    );
                    socket
                }
                _ => bail!("WebSocket proxy must use http or socks5"),
            }
        }
    };
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().insert("Authorization", HeaderValue::from_str(&format!("Bearer {token}"))?);
    request.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_static("handwave"));
    let config = WebSocketConfig::default()
        .max_message_size(Some(crate::wire::MAX_FRAME * 2))
        .max_frame_size(Some(crate::wire::MAX_FRAME * 2));
    let (socket, _) = client_async_tls_with_config(request, socket, Some(config), None)
        .await
        .context("HandWave WebSocket handshake failed")?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_proxy_is_domain_scoped() {
        assert!(bypass_proxy("logs.example.com", 443, ".example.com,localhost"));
        assert!(!bypass_proxy("badexample.com", 443, "example.com"));
        assert!(!bypass_proxy("localhost", 81, "localhost:80"));
    }
}
