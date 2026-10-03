//! Fixed OMP broker discovery, config and encrypted-cache startup.
//! Source: packages/ai/src/auth-broker/discover.ts and coding-agent/src/session/
//! auth-broker-config.ts at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT;
//! Copyright 2025 Mario Zechner, 2025-2026 Can Bölük, 2026 Stencil Labs, Inc.;
//! full license in THIRD_PARTY_NOTICES.md). Host paths/environment stay here.

use crate::{
    auth_broker_client::{AuthBrokerClient, AuthBrokerClientOptions, BrokerError},
    auth_broker_store::{ClientUsageIdentity, RemoteAuthCredentialStore, RemoteAuthCredentialStoreOptions},
    auth_broker_usage::AuthBrokerAccountPool,
    auth_broker_wire::BrokerSnapshotResult,
    auth_storage::{AuthStorage, AuthStorageError, AuthStorageOptions, ConfigKeyResolver},
    model_config_values::{ConfigValueEnvironment, ProcessConfigEnvironment},
};
use async_trait::async_trait;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AuthBrokerConfig {
    pub url: String,
    pub token: String,
}

#[derive(Clone, Debug)]
pub enum BrokerDiscoveryError {
    Configuration(&'static str),
    Auth(AuthStorageError),
    Broker(BrokerError),
}
impl fmt::Display for BrokerDiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => f.write_str(message),
            Self::Auth(error) => error.fmt(f),
            Self::Broker(error) => write!(
                f,
                "Auth broker startup failed ({error}). Start the configured broker or unset ARA_AUTH_BROKER_URL and reset auth.broker.url; local credentials are not used while a broker is configured."
            ),
        }
    }
}
impl std::error::Error for BrokerDiscoveryError {}
impl From<AuthStorageError> for BrokerDiscoveryError {
    fn from(error: AuthStorageError) -> Self {
        Self::Auth(error)
    }
}
impl From<BrokerError> for BrokerDiscoveryError {
    fn from(error: BrokerError) -> Self {
        Self::Broker(error)
    }
}

#[derive(Clone)]
pub struct BrokerDiscoveryHost {
    pub config_root: PathBuf,
    pub agent_dir: PathBuf,
    pub environment: Arc<dyn ConfigValueEnvironment + Send + Sync>,
    pub config_value_resolver: Option<Arc<dyn ConfigKeyResolver>>,
}
impl BrokerDiscoveryHost {
    pub fn for_process(config_root: PathBuf, config_value_resolver: Option<Arc<dyn ConfigKeyResolver>>) -> Self {
        Self {
            agent_dir: config_root.join("agent"),
            config_root,
            environment: Arc::new(ProcessConfigEnvironment),
            config_value_resolver,
        }
    }
}

/// Concurrent calls share the same resolution; successful values, including
/// disabled broker, stay memoized for this Host's process lifetime. Errors evict.
#[derive(Clone)]
pub struct BrokerConfigResolver {
    host: BrokerDiscoveryHost,
    memo: Arc<Mutex<ConfigMemo>>,
}
#[derive(Default)]
struct ConfigMemo {
    next: u64,
    entry: Option<ConfigMemoEntry>,
}
struct ConfigMemoEntry {
    key: (Option<String>, Option<String>),
    id: u64,
    receiver: watch::Receiver<Option<Result<Option<AuthBrokerConfig>, BrokerDiscoveryError>>>,
}
impl BrokerConfigResolver {
    pub fn new(host: BrokerDiscoveryHost) -> Self {
        Self { host, memo: Arc::new(Mutex::new(ConfigMemo::default())) }
    }
    pub async fn resolve(&self, cancel: &CancellationToken) -> Result<Option<AuthBrokerConfig>, BrokerDiscoveryError> {
        if cancel.is_cancelled() {
            return Err(AuthStorageError::Cancelled.into());
        }
        let key =
            (self.host.environment.get("ARA_AUTH_BROKER_URL"), self.host.environment.get("ARA_AUTH_BROKER_TOKEN"));
        let mut receiver = {
            let mut memo = self
                .memo
                .lock()
                .map_err(|_| BrokerDiscoveryError::Configuration("broker configuration owner unavailable"))?;
            if let Some(entry) = memo.entry.as_ref().filter(|entry| entry.key == key) {
                entry.receiver.clone()
            } else {
                memo.next = memo.next.wrapping_add(1);
                let id = memo.next;
                let (sender, receiver) = watch::channel(None);
                memo.entry = Some(ConfigMemoEntry { key: key.clone(), id, receiver: receiver.clone() });
                let this = self.clone();
                tokio::spawn(async move {
                    // The promise owns helpers independently of any waiter.
                    let result = resolve_config(&this.host, key, &CancellationToken::new()).await;
                    if result.is_err()
                        && let Ok(mut memo) = this.memo.lock()
                        && memo.entry.as_ref().is_some_and(|entry| entry.id == id)
                    {
                        memo.entry = None;
                    }
                    sender.send_replace(Some(result));
                });
                receiver
            }
        };
        loop {
            if cancel.is_cancelled() {
                return Err(AuthStorageError::Cancelled.into());
            }
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            tokio::select! { biased;
                _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled.into()),
                changed = receiver.changed() => if changed.is_err() {
                    return Err(BrokerDiscoveryError::Configuration("broker configuration resolution stopped"));
                }
            }
        }
    }
}

fn dotted_string(value: &serde_json::Value, dotted: &str) -> Option<String> {
    let nested = dotted.split('.').try_fold(value, |value, segment| value.as_object()?.get(segment));
    nested.and_then(serde_json::Value::as_str).or_else(|| value.get(dotted)?.as_str()).map(str::to_owned)
}

async fn config_yaml(agent_dir: &Path) -> (Option<String>, Option<String>) {
    for filename in ["config.yml", "config.yaml"] {
        let raw = match tokio::fs::read_to_string(agent_dir.join(filename)).await {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                eprintln!("ara: warning: auth-broker config unreadable");
                return (None, None);
            }
        };
        let Ok(value) = ara_discovery::frontmatter::parse_yaml(&raw) else {
            eprintln!("ara: warning: auth-broker config unreadable");
            return (None, None);
        };
        return (dotted_string(&value, "auth.broker.url"), dotted_string(&value, "auth.broker.token"));
    }
    (None, None)
}

async fn resolve_value(
    host: &BrokerDiscoveryHost,
    value: &str,
    cancel: &CancellationToken,
) -> Result<Option<String>, BrokerDiscoveryError> {
    if let Some(resolver) = &host.config_value_resolver {
        return Ok(resolver.resolve(value, cancel).await?);
    }
    if value.starts_with('!') {
        return Ok(None);
    }
    Ok(host.environment.get(value).filter(|value| !value.is_empty()).or_else(|| Some(value.to_owned())))
}

async fn resolve_config(
    host: &BrokerDiscoveryHost,
    key: (Option<String>, Option<String>),
    cancel: &CancellationToken,
) -> Result<Option<AuthBrokerConfig>, BrokerDiscoveryError> {
    let (env_url, env_token) = key;
    let mut url = env_url.filter(|value| !value.is_empty());
    let env_token = env_token.filter(|value| !value.is_empty());
    let mut config_token = None;
    if url.is_none() || env_token.is_none() {
        let (from_url, from_token) = config_yaml(&host.agent_dir).await;
        if url.is_none()
            && let Some(value) = from_url.filter(|value| !value.is_empty())
        {
            url = resolve_value(host, &value, cancel).await?.filter(|value| !value.is_empty());
        }
        if let Some(value) = from_token.filter(|value| !value.is_empty()) {
            config_token = resolve_value(host, &value, cancel).await?.filter(|value| !value.is_empty());
        }
    }
    let Some(url) = url else {
        return Ok(None);
    };
    let token = match env_token.or(config_token) {
        Some(token) => Some(token),
        None => match tokio::fs::read_to_string(host.config_root.join("auth-broker.token")).await {
            Ok(raw) => (!raw.trim().is_empty()).then(|| raw.trim().to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => { eprintln!("ara: warning: auth-broker token file unreadable"); None }
        },
    }.ok_or(BrokerDiscoveryError::Configuration("Auth broker is configured but no bearer token is available. Set ARA_AUTH_BROKER_TOKEN, auth.broker.token or the auth-broker.token file."))?;
    Ok(Some(AuthBrokerConfig { url, token }))
}

pub async fn load_account_pool(
    host: &BrokerDiscoveryHost,
) -> Result<Option<AuthBrokerAccountPool>, BrokerDiscoveryError> {
    let Some(path) = host.environment.get("ARA_AUTH_BROKER_ACCOUNT_POOL_FILE").filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let raw = tokio::fs::read(path.trim())
        .await
        .map_err(|_| BrokerDiscoveryError::Configuration("Unable to read ARA_AUTH_BROKER_ACCOUNT_POOL_FILE"))?;
    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| {
        BrokerDiscoveryError::Configuration("ARA_AUTH_BROKER_ACCOUNT_POOL_FILE must contain a JSON object")
    })?;
    let object = value
        .as_object()
        .ok_or(BrokerDiscoveryError::Configuration("ARA_AUTH_BROKER_ACCOUNT_POOL_FILE must contain a JSON object"))?;
    let mut pool = AuthBrokerAccountPool::new();
    for (provider, entries) in object {
        if provider.is_empty() || provider != provider.trim() {
            return Err(BrokerDiscoveryError::Configuration("account pool contains an invalid provider id"));
        }
        let entries = entries
            .as_array()
            .ok_or(BrokerDiscoveryError::Configuration("account pool entries must be arrays of identity keys"))?;
        let mut identities = std::collections::BTreeSet::new();
        for identity in entries {
            let identity = identity
                .as_str()
                .filter(|value| !value.is_empty() && *value == value.trim())
                .ok_or(BrokerDiscoveryError::Configuration("account pool contains an invalid identity key"))?;
            identities.insert(identity.to_owned());
        }
        pool.insert(provider.clone(), identities);
    }
    Ok(Some(pool))
}

pub struct DiscoverRemoteOptions {
    pub host: BrokerDiscoveryHost,
    pub client_options: AuthBrokerClientOptions,
    pub cache_path: Option<PathBuf>,
    pub account_pool: Option<AuthBrokerAccountPool>,
    pub identity: ClientUsageIdentity,
}

/// Fixed utils/dirs.ts getInstallId/getAppName: exclusive creation, re-read
/// the winner of a first-install race, and keep a process-stable fallback.
pub fn client_identity(host: &BrokerDiscoveryHost) -> ClientUsageIdentity {
    use std::{fs, io::Write};
    static INSTALL_IDS: std::sync::OnceLock<Mutex<std::collections::BTreeMap<PathBuf, String>>> =
        std::sync::OnceLock::new();
    let mut identities = INSTALL_IDS.get_or_init(Mutex::default).lock().unwrap_or_else(|error| error.into_inner());
    fn valid(value: &str) -> bool {
        value.len() == 36 && uuid::Uuid::parse_str(value).is_ok()
    }
    let path = host.config_root.join("install-id");
    let existing = fs::read_to_string(&path).ok().map(|value| value.trim().to_owned());
    let install_id = if let Some(value) = identities.get(&host.config_root) {
        value.clone()
    } else if let Some(value) = existing.as_ref().filter(|value| valid(value)) {
        value.clone()
    } else {
        let next = uuid::Uuid::new_v4().to_string();
        let _ = fs::create_dir_all(&host.config_root);
        if existing.as_ref().is_some_and(|value| !value.is_empty()) {
            let _ = fs::remove_file(&path);
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "{next}");
                next
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => fs::read_to_string(&path)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| valid(value))
                .unwrap_or(next),
            Err(_) => next,
        }
    };
    identities.insert(host.config_root.clone(), install_id.clone());
    drop(identities);
    ClientUsageIdentity {
        install_id,
        hostname: host.environment.get("COMPUTERNAME").or_else(|| host.environment.get("HOSTNAME")),
        app: Some(
            host.environment
                .get("ARA_APP_NAME")
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "ara".into()),
        ),
    }
}

pub async fn discover_remote_auth_storage(
    config: AuthBrokerConfig,
    options: DiscoverRemoteOptions,
    storage_options: AuthStorageOptions,
    cancel: &CancellationToken,
) -> Result<AuthStorage, BrokerDiscoveryError> {
    let account_pool = match options.account_pool {
        Some(pool) => Some(pool),
        None => load_account_pool(&options.host).await?,
    };
    let client = AuthBrokerClient::new(&config.url, &config.token, options.client_options)?;
    let cache_path = options
        .cache_path
        .or_else(|| {
            options
                .host
                .environment
                .get("ARA_AUTH_BROKER_SNAPSHOT_CACHE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| options.host.config_root.join("cache/auth-broker-snapshot.enc"));
    let ttl = options
        .host
        .environment
        .get("ARA_AUTH_BROKER_SNAPSHOT_TTL_MS")
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value.trim().parse::<f64>().ok().filter(|value| value.is_finite() && *value >= 0.0).unwrap_or_else(|| {
                eprintln!("ara: warning: invalid ARA_AUTH_BROKER_SNAPSHOT_TTL_MS; using default");
                3_600_000.0
            })
        })
        .unwrap_or(3_600_000.0);
    let clock = storage_options.clock.clone();
    let writer = (ttl > 0.0).then(|| {
        crate::auth_broker_snapshot_cache::snapshot_writer(cache_path.clone(), config.token.clone(), config.url.clone())
    });
    let cached = if ttl > 0.0 {
        let (path, token, url, now) = (cache_path, config.token, config.url, clock());
        tokio::task::spawn_blocking(move || {
            crate::auth_broker_snapshot_cache::read_snapshot_cache(&path, &token, &url, ttl, now).ok().flatten()
        })
        .await
        .map_err(|_| BrokerDiscoveryError::Configuration("broker snapshot cache read stopped"))?
    } else {
        None
    };
    let initial_snapshot = match cached {
        Some(snapshot) => snapshot,
        None => match client.fetch_snapshot(None, None, cancel).await? {
            BrokerSnapshotResult::Snapshot { snapshot, .. } => {
                if let Some(writer) = &writer {
                    writer(snapshot.clone(), snapshot.generation);
                }
                snapshot
            }
            BrokerSnapshotResult::NotModified { .. } => {
                return Err(BrokerDiscoveryError::Configuration("Auth broker returned no initial snapshot"));
            }
        },
    };
    let remote = Arc::new(RemoteAuthCredentialStore::new(
        client,
        RemoteAuthCredentialStoreOptions {
            initial_snapshot: Some(initial_snapshot),
            account_pool,
            clock,
            on_snapshot: writer,
            default_client_identity: options.identity,
            ..Default::default()
        },
    )?);
    Ok(AuthStorage::for_remote(remote, storage_options)?)
}

// Used by hosts without command indirection; no shell or local grant is started.
pub struct DefaultBrokerConfigValueResolver(pub Arc<dyn ConfigValueEnvironment + Send + Sync>);
#[async_trait]
impl ConfigKeyResolver for DefaultBrokerConfigValueResolver {
    async fn resolve(&self, config: &str, cancel: &CancellationToken) -> Result<Option<String>, AuthStorageError> {
        if cancel.is_cancelled() {
            return Err(AuthStorageError::Cancelled);
        }
        if config.starts_with('!') {
            return Ok(None);
        }
        Ok(self.0.get(config).filter(|value| !value.is_empty()).or_else(|| Some(config.to_owned())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Env(Mutex<BTreeMap<String, String>>);
    impl ConfigValueEnvironment for Env {
        fn get(&self, name: &str) -> Option<String> {
            self.0.lock().unwrap().get(name).cloned()
        }
    }
    impl Env {
        fn set(&self, name: &str, value: &str) {
            self.0.lock().unwrap().insert(name.into(), value.into());
        }
    }
    fn host(root: &Path, env: Arc<Env>) -> BrokerDiscoveryHost {
        BrokerDiscoveryHost {
            config_root: root.into(),
            agent_dir: root.join("agent"),
            environment: env,
            config_value_resolver: None,
        }
    }

    #[tokio::test]
    async fn native_config_precedence_memo_missing_token_and_pool_family() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("agent")).unwrap();
        let env = Arc::new(Env::default());
        env.set("BROKER_TOKEN", "resolved-token");
        let host = host(root.path(), env.clone());
        std::fs::write(
            host.agent_dir.join("config.yaml"),
            "auth.broker.url: https://yaml.fixture\nauth.broker.token: yaml-token\n",
        )
        .unwrap();
        let config = BrokerConfigResolver::new(host.clone()).resolve(&CancellationToken::new()).await.unwrap().unwrap();
        assert_eq!((config.url.as_str(), config.token.as_str()), ("https://yaml.fixture", "yaml-token"));
        std::fs::write(host.agent_dir.join("config.yml"), "auth:\n  broker:\n    url: https://nested.fixture\n    token: BROKER_TOKEN\nauth.broker.url: https://flat.fixture\n").unwrap();
        let resolver = BrokerConfigResolver::new(host.clone());
        let config = resolver.resolve(&CancellationToken::new()).await.unwrap().unwrap();
        assert_eq!((config.url.as_str(), config.token.as_str()), ("https://nested.fixture", "resolved-token"));
        std::fs::write(host.agent_dir.join("config.yml"), "auth.broker.url: https://changed.fixture\n").unwrap();
        assert_eq!(resolver.resolve(&CancellationToken::new()).await.unwrap().unwrap().url, "https://nested.fixture");
        env.set("ARA_AUTH_BROKER_URL", "https://env.fixture");
        env.set("ARA_AUTH_BROKER_TOKEN", "env-token");
        let config = resolver.resolve(&CancellationToken::new()).await.unwrap().unwrap();
        assert_eq!((config.url.as_str(), config.token.as_str()), ("https://env.fixture", "env-token"));
        env.set("ARA_AUTH_BROKER_TOKEN", "");
        let resolver = BrokerConfigResolver::new(host.clone());
        assert!(resolver.resolve(&CancellationToken::new()).await.is_err());
        // Failed memo is evicted so fixing the token file can recover.
        std::fs::write(root.path().join("auth-broker.token"), " file-token \n").unwrap();
        assert_eq!(resolver.resolve(&CancellationToken::new()).await.unwrap().unwrap().token, "file-token");
        let pool_path = root.path().join("pool.json");
        env.set("ARA_AUTH_BROKER_ACCOUNT_POOL_FILE", pool_path.to_str().unwrap());
        for value in
            ["[]", "{\"fixture\":42}", "{\" fixture\":[]}", "{\"fixture\":[\" email:a\"]}", "{\"fixture\":[null]}"]
        {
            std::fs::write(&pool_path, value).unwrap();
            assert!(load_account_pool(&host).await.is_err());
        }
        std::fs::write(&pool_path, "{\"fixture\":[],\"other\":[\"email:a\",\"email:a\"]}").unwrap();
        let pool = load_account_pool(&host).await.unwrap().unwrap();
        assert!(pool["fixture"].is_empty());
        assert_eq!(pool["other"].len(), 1);
    }

    #[tokio::test]
    async fn native_fresh_cache_startup_and_no_local_credential_fallback_family() {
        use crate::auth_broker_wire::{BrokerSnapshot, BrokerSnapshotEntry};
        use crate::credential_store::AuthCredential;
        let root = tempfile::tempdir().unwrap();
        let env = Arc::new(Env::default());
        let host = host(root.path(), env.clone());
        let path = root.path().join("isolated.enc");
        env.set("ARA_AUTH_BROKER_SNAPSHOT_CACHE", path.to_str().unwrap());
        let config = AuthBrokerConfig { url: "http://127.0.0.1:1".into(), token: "fixture-token".into() };
        let snapshot = BrokerSnapshot {
            generation: 3,
            generated_at: 1000.0,
            server_now_ms: 1000.0,
            credentials: vec![BrokerSnapshotEntry {
                id: 7,
                provider: "fixture".into(),
                credential: AuthCredential::api_key("cached-private-key"),
                identity_key: None,
                rotates_in_ms: None,
                blocks: Vec::new(),
            }],
            ..Default::default()
        };
        crate::auth_broker_snapshot_cache::write_snapshot_cache(&path, &config.token, &config.url, &snapshot).unwrap();
        let identity = client_identity(&host);
        assert_eq!(identity.install_id, client_identity(&host).install_id);
        let storage = discover_remote_auth_storage(
            config.clone(),
            DiscoverRemoteOptions {
                host: host.clone(),
                client_options: Default::default(),
                cache_path: None,
                account_pool: None,
                identity: identity.clone(),
            },
            AuthStorageOptions { clock: Arc::new(|| 1000.0), ..Default::default() },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            storage
                .resolve("fixture", &Default::default(), &CancellationToken::new())
                .await
                .unwrap()
                .unwrap()
                .lease
                .api_key(),
            Some("cached-private-key")
        );
        storage.close_and_wait().await;
        crate::auth_broker_snapshot_cache::wait_for_cache_writes().await;
        assert!(!root.path().join("agent/auth.db").exists());
        env.set("ARA_AUTH_BROKER_SNAPSHOT_TTL_MS", "0");
        let outcome = discover_remote_auth_storage(
            config,
            DiscoverRemoteOptions {
                host,
                client_options: Default::default(),
                cache_path: None,
                account_pool: None,
                identity,
            },
            AuthStorageOptions::default(),
            &CancellationToken::new(),
        )
        .await;
        assert!(matches!(outcome, Err(BrokerDiscoveryError::Broker(_))));
        assert!(!root.path().join("agent/auth.db").exists());
    }
}
