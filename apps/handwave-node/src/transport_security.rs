//! Bearer credentials must never leave this computer over a plaintext Hub link.
use anyhow::{ensure, Result};
use url::{Host, Url};

pub fn validate(url: &Url) -> Result<()> {
    ensure!(url.username().is_empty() && url.password().is_none(), "Hub URL must not contain credentials");
    ensure!(
        matches!(url.scheme(), "https" | "wss") || plaintext_loopback(url),
        "public HandWave connections require HTTPS/WSS with a valid certificate; HTTP/WS is allowed only for literal loopback development endpoints"
    );
    Ok(())
}

pub fn plaintext_loopback(url: &Url) -> bool {
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        // Only literal loopback addresses qualify; DNS and proxies must not
        // turn an external hostname into a development transport exception.
        _ => false,
    };
    loopback && matches!(url.scheme(), "http" | "ws")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plaintext_before_network_and_keeps_literal_loopback_fixtures() {
        for value in [
            "http://example.com",
            "ws://192.0.2.1/ws",
            "http://localhost",
            "http://127.0.0.1.evil.test",
            "http://[::ffff:c000:201]",
            "ftp://127.0.0.1",
        ] {
            assert!(validate(&Url::parse(value).unwrap()).is_err(), "accepted {value}");
        }
        for value in ["https://example.com", "wss://example.com/ws", "http://127.0.0.1:1234", "ws://[::1]:1234/ws"] {
            assert!(validate(&Url::parse(value).unwrap()).is_ok(), "rejected {value}");
        }
        assert!(validate(&Url::parse("https://user:secret@example.com").unwrap()).is_err());
    }
}
