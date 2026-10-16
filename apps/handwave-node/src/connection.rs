//! Connection liveness is separate from task authority: reconnect never replays work.
use crate::proxy;
use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use std::{future::Future, time::Duration};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_util::sync::CancellationToken;

const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const PING_INTERVAL: Duration = Duration::from_secs(15);
const PONG_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Socket {
    inner: proxy::Socket,
    shutdown: CancellationToken,
    connected: Instant,
}

async fn bounded<T>(
    shutdown: &CancellationToken,
    wait: Duration,
    operation: impl Future<Output = Result<T, Error>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => bail!("connection shutdown requested; delivery is not confirmed"),
        result = tokio::time::timeout(wait, operation) => result.context("connection write timed out; delivery is not confirmed")?.map_err(Into::into),
    }
}

impl Socket {
    pub async fn connect(url: &url::Url, token: &str, shutdown: CancellationToken) -> Result<Self> {
        let inner = tokio::select! {
            biased;
            _ = shutdown.cancelled() => bail!("connection shutdown requested"),
            result = tokio::time::timeout(Duration::from_secs(20), proxy::connect(url, token)) => result.context("hub connection timed out")??,
        };
        Ok(Self { inner, shutdown, connected: Instant::now() })
    }

    pub async fn send(&mut self, message: Message) -> Result<()> {
        bounded(&self.shutdown, WRITE_TIMEOUT, self.inner.send(message)).await
    }

    pub async fn next(&mut self) -> Option<Result<Message, Error>> {
        self.inner.next().await
    }

    pub async fn close(&mut self) -> Result<()> {
        // Cleanup has its own outer bound, even when shutdown has already fired.
        self.inner.close(None).await.map_err(Into::into)
    }

    pub fn was_healthy(&self) -> bool {
        self.connected.elapsed() >= Duration::from_secs(20)
    }
}

pub struct Heartbeat {
    next_ping: Instant,
    pending: Option<(u64, Instant)>,
    sequence: u64,
    proven: bool,
}

impl Heartbeat {
    pub fn new(now: Instant) -> Self {
        Self { next_ping: now + PING_INTERVAL, pending: None, sequence: 0, proven: false }
    }

    pub fn tick(&mut self, now: Instant) -> Result<Option<Vec<u8>>> {
        if let Some((_, deadline)) = self.pending {
            anyhow::ensure!(now < deadline, "hub heartbeat timed out; reconnecting without replaying pending work");
            return Ok(None);
        }
        if now < self.next_ping {
            return Ok(None);
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.pending = Some((self.sequence, now + PONG_TIMEOUT));
        Ok(Some(self.sequence.to_be_bytes().to_vec()))
    }

    pub fn pong(&mut self, payload: &[u8], now: Instant) {
        if self.pending.is_some_and(|(sequence, deadline)| now < deadline && payload == sequence.to_be_bytes()) {
            self.pending = None;
            self.next_ping = now + PING_INTERVAL;
            self.proven = true;
        }
    }

    pub fn proven(&self) -> bool {
        self.proven
    }
}

#[derive(Default)]
pub struct Backoff(u32);

impl Backoff {
    pub fn next(&mut self, healthy: bool, jitter: u64) -> Duration {
        if healthy {
            self.0 = 0;
        }
        let base_ms = (1000u64 << self.0.min(5)).min(30_000);
        self.0 = self.0.saturating_add(1);
        Duration::from_millis((base_ms + jitter % (base_ms / 4 + 1)).min(30_000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_matching_pong_renews_liveness() {
        let now = Instant::now();
        let mut heartbeat = Heartbeat::new(now);
        assert!(heartbeat.tick(now).unwrap().is_none());
        let ping = heartbeat.tick(now + PING_INTERVAL).unwrap().unwrap();
        heartbeat.pong(b"other connection or old ping", now + PING_INTERVAL);
        assert!(heartbeat.tick(now + PING_INTERVAL + PONG_TIMEOUT).is_err());
        // A late matching response cannot resurrect a timed-out connection.
        heartbeat.pong(&ping, now + PING_INTERVAL + PONG_TIMEOUT);
        assert!(!heartbeat.proven());
        assert!(heartbeat.tick(now + PING_INTERVAL + PONG_TIMEOUT).is_err());
        let mut heartbeat = Heartbeat::new(now);
        let ping = heartbeat.tick(now + PING_INTERVAL).unwrap().unwrap();
        heartbeat.pong(&ping, now + PING_INTERVAL + Duration::from_secs(1));
        assert!(heartbeat.proven());
        assert!(heartbeat.tick(now + PING_INTERVAL + Duration::from_secs(2)).unwrap().is_none());
        let next = heartbeat.tick(now + PING_INTERVAL * 2 + Duration::from_secs(1)).unwrap().unwrap();
        assert_ne!(ping, next);
        heartbeat.pong(&ping, now + PING_INTERVAL * 2 + Duration::from_secs(2));
        assert!(heartbeat.tick(now + PING_INTERVAL * 2 + Duration::from_secs(1) + PONG_TIMEOUT).is_err());
    }

    #[test]
    fn failed_dials_back_off_and_only_a_healthy_connection_resets() {
        let mut backoff = Backoff::default();
        let sequence: Vec<_> = (0..8).map(|_| backoff.next(false, 0).as_secs()).collect();
        assert_eq!(sequence, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff.next(true, 0).as_secs(), 1);
        assert!(backoff.next(false, u64::MAX) >= Duration::from_secs(2));
        for _ in 0..100 {
            assert!(backoff.next(false, u64::MAX) <= Duration::from_secs(30));
        }
    }

    #[tokio::test]
    async fn stalled_writes_have_a_bound_and_shutdown_wakes_them() {
        let shutdown = CancellationToken::new();
        let result = bounded::<()>(&shutdown, Duration::from_millis(5), std::future::pending()).await;
        assert!(result.unwrap_err().to_string().contains("delivery is not confirmed"));
        shutdown.cancel();
        let result = bounded::<()>(&shutdown, WRITE_TIMEOUT, std::future::pending()).await;
        assert!(result.unwrap_err().to_string().contains("shutdown requested"));
    }

    #[tokio::test]
    async fn shutdown_does_not_start_another_dial() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let result = Socket::connect(&url::Url::parse("ws://127.0.0.1:1/ws").unwrap(), "fixture", shutdown).await;
        assert!(result.err().unwrap().to_string().contains("shutdown requested"));
    }
}
