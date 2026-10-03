//! Encrypted snapshot cache for fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Source: packages/ai/src/auth-broker/snapshot-cache.ts (MIT;
//! Copyright 2025 Mario Zechner, 2025-2026 Can Bölük, 2026 Stencil Labs, Inc.;
//! full license in THIRD_PARTY_NOTICES.md).
//! OMPS/version/AAD are an existing file contract, retained across the Rust port.

use crate::auth_broker_wire::{BrokerSnapshot, parse_snapshot};
use ring::{
    aead, digest,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

const HEADER: &[u8; 5] = b"OMPS\x02";
const IV_LENGTH: usize = 12;
const HEADER_LENGTH: usize = 17;
static PENDING: AtomicUsize = AtomicUsize::new(0);
static SETTLED: Notify = Notify::const_new();

struct WriteSettlement;
impl Drop for WriteSettlement {
    fn drop(&mut self) {
        PENDING.fetch_sub(1, Ordering::AcqRel);
        SETTLED.notify_waiters();
    }
}

/// Drain the Host's already-started cache writes before a normal runtime exit.
pub async fn wait_for_cache_writes() {
    loop {
        let changed = SETTLED.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if PENDING.load(Ordering::Acquire) == 0 {
            return;
        }
        changed.await;
    }
}

/// Secrets stay in this Host callback and never enter Debug, journal or errors.
pub fn snapshot_writer(path: PathBuf, token: String, url: String) -> Arc<dyn Fn(BrokerSnapshot, i64) + Send + Sync> {
    Arc::new(move |snapshot, _generation| {
        let (path, token, url) = (path.clone(), token.clone(), url.clone());
        PENDING.fetch_add(1, Ordering::AcqRel);
        let settlement = WriteSettlement;
        tokio::task::spawn_blocking(move || {
            let _settlement = settlement;
            // Native persistence is best effort; never fail a credential change
            // because the optional local encrypted cache is unavailable.
            let _ = write_snapshot_cache(&path, &token, &url, &snapshot);
        });
    })
}

fn cache_key(token: &str) -> io::Result<aead::LessSafeKey> {
    let hash = digest::digest(&digest::SHA256, token.as_bytes());
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, hash.as_ref())
        .map_err(|_| io::Error::other("broker cache key unavailable"))?;
    Ok(aead::LessSafeKey::new(key))
}

fn additional_data(url: &str) -> Vec<u8> {
    [HEADER.as_slice(), url.as_bytes()].concat()
}

// Private serialization only for authenticated encrypted file bytes.
fn snapshot_value(snapshot: &BrokerSnapshot) -> Value {
    json!({
        "generation": snapshot.generation,
        "generatedAt": snapshot.generated_at,
        "serverNowMs": snapshot.server_now_ms,
        "refresher": {"enabled": snapshot.refresher.enabled,
            "intervalMs": snapshot.refresher.interval_ms, "skewMs": snapshot.refresher.skew_ms,
            "nextSweepInMs": snapshot.refresher.next_sweep_in_ms},
        "credentials": snapshot.credentials.iter().map(|entry| {
            let mut value = json!({"id": entry.id, "provider": entry.provider,
                "credential": entry.credential, "identityKey": entry.identity_key,
                "rotatesInMs": entry.rotates_in_ms});
            if !entry.blocks.is_empty() { value["blocks"] = json!(entry.blocks); }
            value
        }).collect::<Vec<_>>()
    })
}

pub fn read_snapshot_cache(
    path: &Path,
    token: &str,
    url: &str,
    ttl_ms: f64,
    now_ms: f64,
) -> io::Result<Option<BrokerSnapshot>> {
    if ttl_ms <= 0.0 {
        return Ok(None);
    }
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if data.len() <= HEADER_LENGTH || &data[..HEADER.len()] != HEADER {
        return Ok(None);
    }
    let mut ciphertext = data[HEADER_LENGTH..].to_vec();
    let nonce: [u8; IV_LENGTH] = data[HEADER.len()..HEADER_LENGTH].try_into().expect("fixed cache header");
    let key = cache_key(token)?;
    let Ok(plaintext) = key.open_in_place(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(additional_data(url)),
        &mut ciphertext,
    ) else {
        return Ok(None);
    };
    // Like native, authenticated shape/version drift is a cache miss. Rust
    // additionally validates the current wire projection before constructing it.
    let Ok(value) = serde_json::from_slice::<Value>(plaintext) else {
        return Ok(None);
    };
    let Ok(snapshot) = parse_snapshot(&value) else {
        return Ok(None);
    };
    if now_ms - snapshot.generated_at > ttl_ms {
        return Ok(None);
    }
    Ok(Some(snapshot))
}

pub fn write_snapshot_cache(path: &Path, token: &str, url: &str, snapshot: &BrokerSnapshot) -> io::Result<()> {
    let key = cache_key(token)?;
    let random = SystemRandom::new();
    let mut nonce = [0; IV_LENGTH];
    random.fill(&mut nonce).map_err(|_| io::Error::other("broker cache entropy unavailable"))?;
    let mut ciphertext = serde_json::to_vec(&snapshot_value(snapshot))
        .map_err(|_| io::Error::other("broker cache serialization failed"))?;
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(additional_data(url)),
        &mut ciphertext,
    )
    .map_err(|_| io::Error::other("broker cache encryption failed"))?;
    let payload = [HEADER.as_slice(), nonce.as_slice(), ciphertext.as_slice()].concat();
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut suffix = [0; 8];
    random.fill(&mut suffix).map_err(|_| io::Error::other("broker cache entropy unavailable"))?;
    let hex = suffix.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let basename = path.file_name().ok_or_else(|| io::Error::other("invalid broker cache path"))?.to_string_lossy();
    let temp = parent.join(format!("{basename}.{}.{hex}.tmp", std::process::id()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&payload)?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    sweep_stale_temps(parent, &basename);
    Ok(())
}

fn sweep_stale_temps(parent: &Path, basename: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs(3600)).unwrap_or(UNIX_EPOCH);
    let prefix = format!("{basename}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix)
            && name.ends_with(".tmp")
            && entry.metadata().and_then(|value| value.modified()).is_ok_and(|mtime| mtime <= cutoff)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_broker_wire::{BrokerRefresher, BrokerSnapshotEntry};
    use crate::credential_store::AuthCredential;

    #[test]
    fn native_encryption_identity_ttl_and_atomic_replacement_family() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.enc");
        let mut snapshot = BrokerSnapshot {
            generation: 3,
            generated_at: 1000.0,
            server_now_ms: 1000.0,
            refresher: BrokerRefresher::default(),
            credentials: vec![BrokerSnapshotEntry {
                id: 1,
                provider: "fixture".into(),
                credential: AuthCredential::ApiKey { key: "secret-key-fixture".into(), source: None },
                identity_key: None,
                rotates_in_ms: None,
                blocks: Vec::new(),
            }],
        };
        write_snapshot_cache(&path, "token", "https://fixture/v1", &snapshot).unwrap();
        let data = fs::read(&path).unwrap();
        assert_eq!(&data[..5], HEADER);
        assert!(!data.windows(b"secret-key-fixture".len()).any(|bytes| bytes == b"secret-key-fixture"));
        for (token, url, ttl, now) in [
            ("wrong", "https://fixture/v1", 1000.0, 1000.0),
            ("token", "https://fixture/v2", 1000.0, 1000.0),
            ("token", "https://fixture/v1", 1000.0, 2001.0),
            ("token", "https://fixture/v1", 0.0, 1000.0),
        ] {
            assert!(read_snapshot_cache(&path, token, url, ttl, now).unwrap().is_none());
        }
        // Native TTL boundary is inclusive; a future generatedAt stays usable.
        assert_eq!(
            read_snapshot_cache(&path, "token", "https://fixture/v1", 1000.0, 2000.0).unwrap().unwrap().generation,
            3
        );
        snapshot.generation = 4;
        write_snapshot_cache(&path, "token", "https://fixture/v1", &snapshot).unwrap();
        assert_eq!(
            read_snapshot_cache(&path, "token", "https://fixture/v1", 1000.0, 900.0).unwrap().unwrap().generation,
            4
        );
        let mut data = fs::read(&path).unwrap();
        data[HEADER_LENGTH] ^= 1;
        fs::write(&path, data).unwrap();
        assert!(read_snapshot_cache(&path, "token", "https://fixture/v1", 1000.0, 1000.0).unwrap().is_none());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
