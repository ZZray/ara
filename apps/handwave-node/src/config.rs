use anyhow::{bail, Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, Debug, Parser)]
#[command(version, about = "Connect this computer to the HandWave cloud workbench")]
pub struct Config {
    #[arg(long, env = "HW_HUB")]
    pub hub: Option<String>,
    #[arg(long, env = "HW_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    #[arg(long, env = "HW_NODE_STATE")]
    pub state_dir: Option<PathBuf>,
    /// Existing environment-wide INDEX.md + Markdown knowledge directory.
    #[arg(long, env = "HW_KNOWLEDGE_DIR")]
    pub knowledge_dir: Option<PathBuf>,
    #[arg(long, default_value = ".")]
    pub cwd: PathBuf,
    #[arg(long, env = "HW_PROXY")]
    pub proxy: Option<String>,
    #[arg(long, default_value_t = 24)]
    pub max_calls: usize,
    #[arg(long, default_value_t = 600)]
    pub max_seconds: u64,
    /// Print the device fingerprint and exit without connecting.
    #[arg(long)]
    pub identity: bool,
    /// Exit after one connection (useful for supervised processes).
    #[arg(long)]
    pub once: bool,
}

pub struct Resolved {
    pub config: Config,
    pub hub: Url,
    pub token: String,
    pub state: PathBuf,
    pub cwd: PathBuf,
    pub fp: String,
    identity: crate::node_identity::Identity,
    _lock: std::fs::File,
}

impl Resolved {
    pub fn load(mut config: Config) -> Result<Self> {
        let exe_dir = std::env::current_exe()?.parent().context("executable directory")?.to_path_buf();
        let home = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME")).context("set --state-dir")?;
        let state = config.state_dir.clone().unwrap_or_else(|| PathBuf::from(home).join("handwave-rust-node"));
        std::fs::create_dir_all(&state)?;
        use fs2::FileExt;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join("node.lock"))?;
        lock.try_lock_exclusive()
            .context("this Rust node state directory is already in use; choose a separate --state-dir")?;
        let mut conf = read_conf(&exe_dir.join("node.conf"))?;
        conf.extend(read_conf(&state.join("node.conf"))?);
        let hub = config
            .hub
            .clone()
            .or_else(|| conf.get("hub").cloned())
            .or_else(|| config.identity.then(|| "http://127.0.0.1".to_string()))
            .context("set HW_HUB or hub= in node.conf")?;
        let hub = normalize_hub(&hub)?;
        let token = config.token.take().or_else(|| conf.get("token").cloned()).unwrap_or_default();
        if token.trim().is_empty() && !config.identity {
            bail!("set HW_TOKEN or token= in node.conf (use the node enrollment token)");
        }
        anyhow::ensure!(!token.contains(['\r', '\n']), "invalid enrollment token");
        config.proxy = config.proxy.or_else(|| conf.get("proxy").filter(|s| !s.is_empty()).cloned());
        config.knowledge_dir = config
            .knowledge_dir
            .or_else(|| conf.get("knowledge_dir").filter(|s| !s.trim().is_empty()).map(PathBuf::from));
        if let Some(proxy) = &config.proxy {
            let parsed = Url::parse(proxy).context("invalid proxy URL")?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "socks5" | "socks5h"),
                "proxy must be http:// or socks5://"
            );
            // Do this before building the async runtime or any HTTP client.
            std::env::set_var("HTTP_PROXY", proxy);
            std::env::set_var("HTTPS_PROXY", proxy);
            std::env::set_var("ALL_PROXY", proxy);
            std::env::set_var("http_proxy", proxy);
            std::env::set_var("https_proxy", proxy);
            std::env::set_var("all_proxy", proxy);
        }
        anyhow::ensure!(config.max_calls > 0 && config.max_seconds > 0, "budgets must be positive");
        let cwd = std::fs::canonicalize(&config.cwd).context("working directory does not exist")?;
        anyhow::ensure!(cwd.is_dir(), "cwd must be a directory");
        let identity = crate::node_identity::Identity::load(&state)?;
        let fp = identity.fingerprint();
        Ok(Self { config, hub, token, state, cwd, fp, identity, _lock: lock })
    }

    pub fn signed_join(&self, challenge: &Value) -> Result<Value> {
        self.identity.signed_join(challenge, self.hello())
    }

    pub fn hello(&self) -> Value {
        let os = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            s => s,
        };
        json!({"hostname":std::env::var("COMPUTERNAME").or_else(|_|std::env::var("HOSTNAME")).unwrap_or_else(|_|"rust-node".into()),
            "user":std::env::var("USERNAME").or_else(|_|std::env::var("USER")).unwrap_or_else(|_|"unknown".into()),
            "os":os,"arch":arch,"version":concat!("rust-",env!("CARGO_PKG_VERSION")),
              "caps":["exec","files","agent-task","agent-loop","cancel","cloud-memory-v1","session-v1","run-modes-v1","goal-runner-v1","managed-jobs-v1","interactive-jobs-v1","terminal-jobs-v1","history-journal-v1","agent-queue-v1","web-research-v1","environment-library-v1"]})
    }

    pub fn websocket_url(&self) -> Url {
        let mut url = self.hub.clone();
        url.set_scheme(if self.hub.scheme() == "https" { "wss" } else { "ws" }).expect("valid scheme");
        url.set_path(&format!("{}/ws", self.hub.path().trim_end_matches('/')));
        url.set_query(Some("role=node"));
        url
    }

    pub fn endpoint(&self) -> String {
        format!("{}/v1", self.hub.as_str().trim_end_matches('/'))
    }

    pub fn session_path(&self, owner: &str) -> Result<PathBuf> {
        native_session_path(&self.state, &self.cwd, owner, "")
    }

    pub fn session_path_for(&self, owner: &str, session_key: &str) -> Result<PathBuf> {
        native_session_path(&self.state, &self.cwd, owner, session_key)
    }
}

fn native_session_path(state: &Path, cwd: &Path, owner: &str, session_key: &str) -> Result<PathBuf> {
    if !session_key.is_empty() {
        anyhow::ensure!(session_key.len() <= 193, "invalid web session key");
        let parts: Vec<_> = session_key.split('/').collect();
        anyhow::ensure!(
            parts.len() == 2
                && parts.iter().all(|part| !part.is_empty()
                    && part.len() <= 96
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')),
            "invalid web session key"
        );
    }
    anyhow::ensure!(
        !owner.is_empty()
            && owner.len() <= 128
            && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid controller identity"
    );
    // A separate owner directory prevents a new controller inheriting history.
    let dir = state.join("sessions").join(owner);
    std::fs::create_dir_all(&dir)?;
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(cwd.to_string_lossy().as_bytes());
    if !session_key.is_empty() {
        digest.update(b"\0");
        digest.update(session_key.as_bytes());
    }
    let key = data_encoding::HEXLOWER.encode(&digest.finalize());
    Ok(dir.join(if session_key.is_empty() { format!("{key}.json") } else { format!("ui-{key}.json") }))
}

fn read_conf(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect())
}

pub fn normalize_hub(hub: &str) -> Result<Url> {
    let text = if hub.contains("://") { hub.to_string() } else { format!("https://{hub}") };
    let mut url = Url::parse(&text)?;
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        _ => bail!("hub must be http(s) or ws(s)"),
    };
    url.set_scheme(scheme).map_err(|_| anyhow::anyhow!("invalid hub scheme"))?;
    anyhow::ensure!(
        url.host_str().is_some() && url.username().is_empty() && url.password().is_none(),
        "invalid hub URL"
    );
    crate::transport_security::validate(&url)?;
    let path = url.path().trim_end_matches('/').trim_end_matches("/ws").to_string();
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hub_normalizes_transport_and_path() {
        assert_eq!(
            normalize_hub("wss://host.example/handwave/ws?role=node").unwrap().as_str(),
            "https://host.example/handwave"
        );
        assert!(normalize_hub("https://user:secret@host.example").is_err());
    }
    #[test]
    fn explicit_web_sessions_are_stable_isolated_and_keep_legacy_paths() {
        let root = std::env::temp_dir().join(format!(
            "handwave-session-map-{}-{}",
            std::process::id(),
            crate::wire::fresh_id()
        ));
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let legacy = native_session_path(&root, &cwd, "owner-a", "").unwrap();
        use sha2::{Digest, Sha256};
        let old_key = data_encoding::HEXLOWER.encode(&Sha256::digest(cwd.to_string_lossy().as_bytes()));
        assert_eq!(legacy, root.join("sessions/owner-a").join(format!("{old_key}.json")));
        let a = native_session_path(&root, &cwd, "owner-a", "ws-1/session-a").unwrap();
        let b = native_session_path(&root, &cwd, "owner-a", "ws-1/session-b").unwrap();
        assert_ne!(a, b);
        assert_ne!(a, legacy);
        assert_eq!(a, native_session_path(&root, &cwd, "owner-a", "ws-1/session-a").unwrap());
        assert_ne!(a, native_session_path(&root, &cwd, "owner-b", "ws-1/session-a").unwrap());
        assert_ne!(a, native_session_path(&root, &cwd.join("other"), "owner-a", "ws-1/session-a").unwrap());
        std::fs::write(&a, "native-a").unwrap();
        std::fs::write(a.with_extension("context.json"), "memory-a").unwrap();
        std::fs::write(&b, "native-b").unwrap();
        std::fs::write(b.with_extension("context.json"), "memory-b").unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "native-a");
        assert_eq!(std::fs::read_to_string(a.with_extension("context.json")).unwrap(), "memory-a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "native-b");
        for invalid in ["missing-session", "/empty", "empty/", "../escape", "ws/a/b", "ws/中文", "ws/a.b", "ws/ "] {
            assert!(native_session_path(&root, &cwd, "owner-a", invalid).is_err());
        }
        assert!(native_session_path(&root, &cwd, "owner-a", &format!("ws/{}", "a".repeat(97))).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
