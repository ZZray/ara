//! Fixed OMP Broker CLI management; local ownership is lazy and Host-bound.

// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use crate::auth_broker_client::{AuthBrokerClient, AuthBrokerClientOptions};
use crate::auth_broker_discover::BrokerConfigResolver;
use crate::auth_broker_server::{AuthBrokerServerOptions, start_auth_broker};
use crate::auth_broker_wire::BrokerSnapshotResult;
use crate::auth_storage::{AuthStorage, BrokerStorageFacade, Environment};
use crate::credential_store::AuthCredential;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::rand::SecureRandom;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

#[derive(clap::Args, Debug, Clone)]
pub struct BrokerArgs {
    #[command(subcommand)]
    pub command: Option<BrokerCommand>,
}
#[derive(clap::Subcommand, Debug, Clone)]
pub enum BrokerCommand {
    Serve {
        #[arg(short = 'b', long)]
        bind: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Token {
        #[arg(long)]
        regenerate: bool,
        #[arg(long)]
        json: bool,
    },
    Login {
        provider: Option<String>,
        #[arg(long = "provider")]
        provider_flag: Option<String>,
        #[arg(long)]
        via: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    Logout {
        provider: Option<String>,
        #[arg(long = "provider")]
        provider_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Import {
        source: PathBuf,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        include_disabled: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    Migrate {
        #[arg(long)]
        from_local: bool,
        #[arg(long)]
        include_env: bool,
        #[arg(long)]
        include_oauth: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}
#[derive(Debug)]
pub struct BrokerManagementError(pub String);
impl std::fmt::Display for BrokerManagementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BrokerManagementError {}
fn error(message: impl Into<String>) -> BrokerManagementError {
    BrokerManagementError(message.into())
}
#[async_trait]
pub trait BrokerLocalAuthFactory: Send + Sync {
    async fn open(&self) -> Result<Arc<AuthStorage>, BrokerManagementError>;
}
#[async_trait]
pub trait BrokerDeviceLoginPort: Send + Sync {
    async fn issue(&self, cancel: &CancellationToken) -> Result<AuthCredential, BrokerManagementError>;
}
#[async_trait]
pub trait BrokerConsole: Send + Sync {
    fn write(&self, text: &str) -> Result<(), BrokerManagementError>;
    async fn prompt(&self, text: &str, cancel: &CancellationToken) -> Result<String, BrokerManagementError>;
}
pub struct ProcessBrokerConsole;
#[async_trait]
impl BrokerConsole for ProcessBrokerConsole {
    fn write(&self, text: &str) -> Result<(), BrokerManagementError> {
        let mut out = std::io::stdout().lock();
        out.write_all(text.as_bytes()).and_then(|_| out.flush()).map_err(|_| error("broker stdout failed"))
    }
    async fn prompt(&self, text: &str, cancel: &CancellationToken) -> Result<String, BrokerManagementError> {
        self.write(text)?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // An OS input thread is detached on cancellation; unlike spawn_blocking,
        // it cannot hold Tokio runtime shutdown waiting for another input line.
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::stdin()
                .read_line(&mut line)
                .map_err(|_| error("broker prompt input failed"))
                .and_then(|size| {
                    if size == 0 || line.contains('\u{1b}') {
                        Err(error("Login cancelled"))
                    } else {
                        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
                    }
                });
            let _ = sender.send(result);
        });
        tokio::select! {biased;_=cancel.cancelled()=>Err(error("Login cancelled")),result=receiver=>result.map_err(|_|error("broker prompt stopped"))?}
    }
}
pub struct BrokerManagementHost {
    pub config_root: PathBuf,
    pub version: String,
    pub client: reqwest::Client,
    pub clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub config: BrokerConfigResolver,
    pub local_auth: Arc<dyn BrokerLocalAuthFactory>,
    pub device_login: Arc<dyn BrokerDeviceLoginPort>,
    pub environment: Arc<dyn Environment>,
    pub console: Arc<dyn BrokerConsole>,
    // Ports belong to implemented callback flows. The current OpenAI Host
    // offers device login, so it supplies no fabricated loopback callback.
    pub callback_ports: BTreeMap<String, u16>,
}
fn line(host: &BrokerManagementHost, value: impl std::fmt::Display) -> Result<(), BrokerManagementError> {
    host.console.write(&format!("{value}\n"))
}
fn token_path(host: &BrokerManagementHost) -> PathBuf {
    host.config_root.join("auth-broker.token")
}
async fn read_token(host: &BrokerManagementHost) -> Result<Option<String>, BrokerManagementError> {
    match tokio::fs::read_to_string(token_path(host)).await {
        Ok(raw) => {
            let token = ara_prompt::js::trim(&raw);
            Ok((!token.is_empty()).then(|| token.to_owned()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(error("broker token file unreadable")),
    }
}
async fn write_token(host: &BrokerManagementHost, token: &str) -> Result<(), BrokerManagementError> {
    tokio::fs::create_dir_all(&host.config_root).await.map_err(|_| error("broker token directory write failed"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&host.config_root, std::fs::Permissions::from_mode(0o700)).await;
    }
    tokio::fs::write(token_path(host), token).await.map_err(|_| error("broker token file write failed"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(token_path(host), std::fs::Permissions::from_mode(0o600)).await;
    }
    Ok(())
}
async fn ensure_token(host: &BrokerManagementHost, regenerate: bool) -> Result<String, BrokerManagementError> {
    if !regenerate && let Some(token) = read_token(host).await? {
        return Ok(token);
    }
    let mut bytes = [0_u8; 32];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| error("broker token generation failed"))?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    write_token(host, &token).await?;
    Ok(token)
}
fn providers() -> Vec<Value> {
    crate::catalog_behavior::auth_providers()
        .iter()
        .filter(|p| !p["login"].is_null() && p["showInLoginList"] != false)
        .map(|p| json!({"id":p["id"],"name":p["name"]}))
        .collect()
}
async fn select_provider(
    host: &BrokerManagementHost,
    ids: &[String],
    logout: bool,
    cancel: &CancellationToken,
) -> Result<String, BrokerManagementError> {
    line(host, if logout { "Select a provider to logout:\n" } else { "Select a provider to login:\n" })?;
    for (index, id) in ids.iter().enumerate() {
        line(host, format!("  {}. {id}", index + 1))?;
    }
    let choice = host.console.prompt(&format!("\nEnter number (1-{}): ", ids.len()), cancel).await?;
    let value = ara_prompt::js::trim_start(&choice);
    let sign = usize::from(value.starts_with('+') || value.starts_with('-'));
    let digits = sign + value[sign..].bytes().take_while(u8::is_ascii_digit).count();
    let index =
        value[..digits].parse::<isize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| usize::try_from(n).ok());
    index.and_then(|i| ids.get(i)).cloned().ok_or_else(|| error(format!("Invalid selection: {choice}")))
}
fn client(host: &BrokerManagementHost, url: &str, token: &str) -> Result<AuthBrokerClient, BrokerManagementError> {
    AuthBrokerClient::new(url, token, AuthBrokerClientOptions { client: host.client.clone(), ..Default::default() })
        .map_err(|e| error(e.to_string()))
}
async fn local(
    host: &BrokerManagementHost,
    retained: &mut Option<Arc<AuthStorage>>,
) -> Result<Arc<AuthStorage>, BrokerManagementError> {
    let storage = host.local_auth.open().await?;
    *retained = Some(storage.clone());
    Ok(storage)
}

pub async fn run_auth_broker_command(
    args: BrokerArgs,
    host: &BrokerManagementHost,
    account_auth: &mut Option<Arc<AuthStorage>>,
    cancel: &CancellationToken,
) -> Result<i32, BrokerManagementError> {
    let Some(command) = args.command else {
        line(host, "Usage: ara auth-broker <serve|token|login|logout|list|status|import|migrate>")?;
        return Ok(0);
    };
    match command {
        BrokerCommand::Token { regenerate, json: as_json } => {
            let token = ensure_token(host, regenerate).await?;
            if as_json {
                line(host, json!({"token":token,"path":token_path(host)}))?;
            } else {
                line(host, token)?;
            }
            Ok(0)
        }
        BrokerCommand::List { json: as_json } => {
            let providers = providers();
            if as_json {
                line(host, json!(providers))?;
            } else {
                line(host, "Available providers:\n")?;
                for p in providers {
                    line(
                        host,
                        format!("  {:20} {}", p["id"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or("")),
                    )?;
                }
            }
            Ok(0)
        }
        BrokerCommand::Serve { bind, json: _ } => {
            let token = ensure_token(host, false).await?;
            let storage = local(host, account_auth).await?;
            let mut options = AuthBrokerServerOptions::new(storage, vec![token]);
            if let Some(bind) = bind {
                options.bind = bind;
            }
            options.version = Some(host.version.clone());
            options.managed_mcp_client = Some(host.client.clone());
            let handle = start_auth_broker(options).await.map_err(|e| error(e.to_string()))?;
            line(host, json!({"event":"auth-broker listening","url":handle.url()}))?;
            line(host, json!({"event":"auth-broker bearer token loaded","path":token_path(host),"mode":"0600"}))?;
            cancel.cancelled().await;
            handle.close().await.map_err(|e| error(e.to_string()))?;
            Ok(0)
        }
        BrokerCommand::Login { provider, provider_flag, via, dry_run, json: _ } => {
            let provider = provider.or(provider_flag).filter(|value| !value.is_empty());
            let via = via.filter(|value| !value.is_empty());
            let known = providers();
            let ids = known.iter().filter_map(|p| p["id"].as_str().map(str::to_owned)).collect::<Vec<_>>();
            let provider = match provider {
                Some(value) => value,
                None if via.is_some() => {
                    return Err(error(
                        "Usage: ara auth-broker login <provider> --via=user@host (provider required for remote login)",
                    ));
                }
                None => select_provider(host, &ids, false, cancel).await?,
            };
            if !ids.contains(&provider) {
                let mut names = ids;
                names.sort();
                return Err(error(format!("Unknown OAuth provider '{provider}'. Known: {}", names.join(", "))));
            }
            if let Some(via) = via {
                let port=host.callback_ports.get(&provider).ok_or_else(||error(format!("No known OAuth callback port for '{provider}'. Use device-code flow on the broker host directly.")))?;
                let ssh_args = vec![
                    "-L".to_owned(),
                    format!("{port}:127.0.0.1:{port}"),
                    "-o".into(),
                    "ExitOnForwardFailure=yes".into(),
                    via,
                    format!("ara auth-broker login {provider}"),
                ];
                if dry_run {
                    line(
                        host,
                        format!(
                            "ssh {}",
                            ssh_args
                                .iter()
                                .map(|v| if v.contains(' ') { format!("'{v}'") } else { v.clone() })
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                    )?;
                    return Ok(0);
                }
                let status = tokio::process::Command::new("ssh")
                    .args(ssh_args)
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .status()
                    .await
                    .map_err(|_| error("ssh binary not found in PATH or failed to start"))?;
                if !status.success() {
                    return Err(error(format!(
                        "ssh exited with code {}",
                        status.code().map_or("unknown".into(), |v| v.to_string())
                    )));
                }
                return Ok(0);
            }
            if provider != "openai-codex" && provider != "openai-codex-device" {
                return Err(error(format!(
                    "Login for '{provider}' is deferred; OpenAI account device login is available."
                )));
            }
            let storage = local(host, account_auth).await?;
            let credential = host.device_login.issue(cancel).await?;
            storage
                .upsert_oauth_credential("openai-codex", &credential, cancel)
                .await
                .map_err(|e| error(e.to_string()))?;
            line(host, format!("\nCredentials saved to {}", host.config_root.join("agent/auth.db").display()))?;
            Ok(0)
        }
        BrokerCommand::Logout { provider, provider_flag, json: _ } => {
            let provider = provider.or(provider_flag).filter(|value| !value.is_empty());
            let storage = local(host, account_auth).await?;
            let facade = BrokerStorageFacade::new(storage);
            let provider = match provider {
                Some(provider) => provider,
                None => {
                    let snapshot = facade.export_snapshot().map_err(|e| error(e.to_string()))?;
                    let mut seen = BTreeSet::new();
                    let providers = snapshot
                        .credentials
                        .into_iter()
                        .filter_map(|row| seen.insert(row.provider.clone()).then_some(row.provider))
                        .collect::<Vec<_>>();
                    if providers.is_empty() {
                        line(host, "No credentials stored.")?;
                        return Ok(0);
                    }
                    select_provider(host, &providers, true, cancel).await?
                }
            };
            facade.logout_provider(&provider, cancel).await.map_err(|e| error(e.to_string()))?;
            line(host, format!("Logged out of {provider}"))?;
            Ok(0)
        }
        BrokerCommand::Status { json: as_json } => {
            let config = host.config.resolve(cancel).await.map_err(|e| error(e.to_string()))?;
            let Some(config) = config else {
                if as_json {
                    line(host, json!({"ok":false,"reason":"not_configured"}))?;
                } else {
                    line(host, "No auth-broker configured (set ARA_AUTH_BROKER_URL to enable).")?;
                }
                return Ok(0);
            };
            let result = client(host, &config.url, &config.token)?.healthz(cancel).await;
            match result {
                Ok(mut health) => {
                    if as_json {
                        health["url"] = json!(config.url);
                        line(host, health)?;
                    } else {
                        line(
                            host,
                            format!("OK {} (version={})", config.url, health["version"].as_str().unwrap_or("unknown")),
                        )?;
                    }
                    Ok(0)
                }
                Err(err) => {
                    if as_json {
                        line(host, json!({"ok":false,"url":config.url,"error":err.to_string()}))?;
                    } else {
                        line(host, format!("FAILED {}: {err}", config.url))?;
                    }
                    Ok(1)
                }
            }
        }
        BrokerCommand::Import { source, provider, include_disabled, dry_run, json: as_json } => {
            run_import(host, source, provider, include_disabled, dry_run, as_json, account_auth, cancel).await
        }
        BrokerCommand::Migrate { from_local, include_env, include_oauth, dry_run, json: as_json } => {
            run_migrate(host, from_local, include_env, include_oauth, dry_run, as_json, account_auth, cancel).await
        }
    }
}
struct ImportEntry {
    file: PathBuf,
    provider: String,
    email: Option<String>,
    account_id: Option<String>,
    expires: f64,
    disabled: bool,
    credential: AuthCredential,
}
impl ImportEntry {
    fn plan_value(&self) -> Value {
        json!({"provider":self.provider,"email":self.email,"accountId":self.account_id,"expiresAt":self.expires,"disabled":self.disabled,"file":self.file})
    }
    fn describe(&self, now: f64) -> String {
        format!(
            "{}: {}{}{} from {}",
            self.provider,
            self.email.as_deref().or(self.account_id.as_deref()).unwrap_or("(no identity)"),
            if self.expires < now { " [expired]" } else { "" },
            if self.disabled { " [disabled]" } else { "" },
            self.file.display()
        )
    }
}
const CLIPROXY_TYPES: [(&str, &str); 5] = [
    ("claude", "anthropic"),
    ("codex", "openai-codex"),
    ("gemini", "google-gemini-cli"),
    ("antigravity", "google-antigravity"),
    ("gemini-cli", "google-gemini-cli"),
];
fn proxy_provider(value: &Value, file: &Path, override_id: Option<&str>) -> Option<String> {
    if let Some(id) = override_id.filter(|id| !id.is_empty()) {
        return Some(id.into());
    }
    if let Some(typ) = value["type"].as_str().map(|v| ara_prompt::js::trim(v).to_lowercase())
        && let Some((_, provider)) = CLIPROXY_TYPES.iter().find(|(known, _)| *known == typ)
    {
        return Some((*provider).into());
    }
    let mut filename = file.file_name()?.to_string_lossy().into_owned();
    if filename.ends_with(".json") {
        filename.truncate(filename.len() - 5);
    }
    let filename = filename.to_lowercase();
    CLIPROXY_TYPES
        .iter()
        .find(|(typ, _)| filename == *typ || filename.starts_with(&format!("{typ}-")))
        .map(|(_, provider)| (*provider).into())
}
fn expiry(raw: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(raw))
        .ok()
        .map(|v| v.timestamp_millis() as f64)
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .and_then(|v| v.and_hms_opt(0, 0, 0))
                .map(|v| v.and_utc().timestamp_millis() as f64)
        })
}
async fn import_plan(
    source: &Path,
    override_id: Option<&str>,
    include_disabled: bool,
) -> Result<(Vec<ImportEntry>, Vec<Value>), BrokerManagementError> {
    let metadata = tokio::fs::metadata(source)
        .await
        .map_err(|_| error(format!("Import source unreadable: {}", source.display())))?;
    let mut files = Vec::new();
    if metadata.is_file() {
        files.push(source.to_path_buf());
    } else if metadata.is_dir() {
        let mut entries = tokio::fs::read_dir(source).await.map_err(|_| error("Import directory unreadable"))?;
        while let Some(entry) = entries.next_entry().await.map_err(|_| error("Import directory unreadable"))? {
            if entry.file_type().await.map_err(|_| error("Import file metadata unreadable"))?.is_file()
                && entry.file_name().to_string_lossy().ends_with(".json")
            {
                files.push(entry.path());
            }
        }
        files.sort_by_cached_key(|p| p.to_string_lossy().encode_utf16().collect::<Vec<_>>());
    } else {
        return Err(error(format!("Import source is neither file nor directory: {}", source.display())));
    }
    let mut plan = Vec::new();
    let mut skipped = Vec::new();
    for file in files {
        let value = match tokio::fs::read(&file).await.ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()) {
            Some(value) => value,
            None => {
                skipped.push(json!({"file":file,"reason":"unreadable JSON"}));
                continue;
            }
        };
        let disabled = value["disabled"] == true;
        if disabled && !include_disabled {
            skipped.push(
                json!({"file":file,"reason":"credential marked disabled (use --include-disabled to import anyway)"}),
            );
            continue;
        }
        let Some(provider) = proxy_provider(&value, &file, override_id) else {
            skipped.push(json!({"file":file,"reason":format!("cannot determine ara provider from type={} (pass --provider to override)",value["type"].as_str().unwrap_or("?"))}));
            continue;
        };
        let access = value["access_token"].as_str().filter(|v| !v.is_empty());
        let refresh = value["refresh_token"].as_str().filter(|v| !v.is_empty());
        let (Some(access), Some(refresh)) = (access, refresh) else {
            skipped.push(json!({"file":file,"reason":"missing access_token or refresh_token"}));
            continue;
        };
        let Some(expires) = value["expired"].as_str().and_then(expiry) else {
            skipped.push(json!({"file":file,"reason":format!("cannot parse expired={}",value["expired"].as_str().unwrap_or("?"))}));
            continue;
        };
        let email = value["email"].as_str().filter(|v| !v.is_empty()).map(str::to_owned);
        let account_id = value["account_id"].as_str().filter(|v| !v.is_empty()).map(str::to_owned);
        let mut fields = Map::new();
        fields.insert("access".into(), json!(access));
        fields.insert("refresh".into(), json!(refresh));
        fields.insert("expires".into(), json!(expires));
        if let Some(email) = &email {
            fields.insert("email".into(), json!(email));
        }
        if let Some(account) = &account_id {
            fields.insert("accountId".into(), json!(account));
        }
        plan.push(ImportEntry {
            file,
            provider,
            email,
            account_id,
            expires,
            disabled,
            credential: AuthCredential::OAuth { fields },
        });
    }
    Ok((plan, skipped))
}
// Keep command options, retained storage ownership, and cancellation explicit.
#[allow(clippy::too_many_arguments, reason = "Import options and retained storage/cancellation are distinct host inputs.")]
async fn run_import(
    host: &BrokerManagementHost,
    source: PathBuf,
    provider: Option<String>,
    include_disabled: bool,
    dry_run: bool,
    as_json: bool,
    retained: &mut Option<Arc<AuthStorage>>,
    cancel: &CancellationToken,
) -> Result<i32, BrokerManagementError> {
    let source = if let Some(raw) = source.to_str().and_then(|v| v.strip_prefix('~')) {
        let home = host
            .environment
            .variable("USERPROFILE")
            .or_else(|| host.environment.variable("HOME"))
            .ok_or_else(|| error("Home directory unavailable"))?;
        PathBuf::from(format!("{home}{raw}"))
    } else {
        source
    };
    let source = if source.is_absolute() {
        source
    } else {
        std::env::current_dir().map_err(|_| error("Current directory unavailable"))?.join(source)
    };
    let (plan, skipped) = import_plan(&source, provider.as_deref(), include_disabled).await?;
    if as_json {
        line(
            host,
            json!({"dryRun":dry_run,"imported":if dry_run {vec![]}else{plan.iter().map(|e|json!({"provider":e.provider,"email":e.email,"file":e.file})).collect::<Vec<_>>()},"plan":plan.iter().map(ImportEntry::plan_value).collect::<Vec<_>>(),"skipped":skipped}),
        )?;
    } else {
        for skip in &skipped {
            line(
                host,
                format!("skip {}: {}", skip["file"].as_str().unwrap_or(""), skip["reason"].as_str().unwrap_or("")),
            )?;
        }
    }
    if plan.is_empty() {
        if !as_json {
            line(host, format!("No importable credentials in {}.", source.display()))?;
        }
        return Ok(0);
    }
    if dry_run {
        if !as_json {
            line(host, format!("Dry run — would import {} credential(s):", plan.len()))?;
            for entry in &plan {
                line(host, format!("  {}", entry.describe((host.clock)())))?;
            }
        }
        return Ok(0);
    }
    let config = host.config.resolve(cancel).await.map_err(|e| error(e.to_string()))?;
    if let Some(config) = config {
        let client = client(host, &config.url, &config.token)?;
        let mut exit = 0;
        for entry in plan {
            match client.upload_credential(&entry.provider, &entry.credential, cancel).await {
                Ok(_) => {
                    if !as_json {
                        line(host, format!("uploaded {} → {}", entry.describe((host.clock)()), config.url))?;
                    }
                }
                Err(err) => {
                    exit = 1;
                    if as_json {
                        line(host, json!({"error":err.to_string(),"file":entry.file}))?;
                    } else {
                        line(host, format!("failed {}: {err}", entry.describe((host.clock)())))?;
                    }
                    if err.is_cancelled() || err.is_outcome_unknown() {
                        break;
                    }
                }
            }
        }
        Ok(exit)
    } else {
        let storage = local(host, retained).await?;
        let facade = BrokerStorageFacade::new(storage);
        for entry in plan {
            facade.upsert(&entry.provider, entry.credential.clone(), cancel).await.map_err(|e| error(e.to_string()))?;
            if !as_json {
                line(host, format!("imported {}", entry.describe((host.clock)())))?;
            }
        }
        Ok(0)
    }
}
struct MigrateEntry {
    source: &'static str,
    provider: String,
    credential: AuthCredential,
    identity: String,
}
fn credential_identity(provider: &str, credential: &AuthCredential) -> String {
    match credential {
        AuthCredential::ApiKey { .. } => "(api key)".into(),
        AuthCredential::OAuth { fields } => {
            let text = |name: &str| fields.get(name).and_then(Value::as_str);
            let base = text("email")
                .or(text("accountId"))
                .or(text("projectId"))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("<{provider} oauth>"));
            match text("orgId").filter(|s| !s.is_empty()) {
                Some(org) => format!("{base} ({})", text("orgName").unwrap_or(org)),
                None => base,
            }
        }
    }
}
fn identity_markers(credential: &AuthCredential) -> BTreeSet<String> {
    match credential {
        AuthCredential::ApiKey { .. } => BTreeSet::from(["@api_key".into()]),
        AuthCredential::OAuth { fields } => {
            let text = |name: &str| fields.get(name).and_then(Value::as_str).filter(|v| !v.is_empty());
            let org = text("orgId");
            let suffix = org.map(|org| format!("|org:{org}")).unwrap_or_default();
            let mut ids = ["email", "accountId", "projectId"]
                .into_iter()
                .filter_map(|name| text(name).map(|value| format!("{name}:{value}{suffix}")))
                .collect::<BTreeSet<_>>();
            if ids.is_empty()
                && let Some(org) = org
            {
                ids.insert(format!("org:{org}"));
            }
            ids
        }
    }
}
// Preserve the separate migration options and retained storage/cancellation boundary.
#[allow(clippy::too_many_arguments, reason = "Migration options and retained storage/cancellation are distinct host inputs.")]
async fn run_migrate(
    host: &BrokerManagementHost,
    from_local: bool,
    include_env: bool,
    include_oauth: bool,
    dry_run: bool,
    as_json: bool,
    retained: &mut Option<Arc<AuthStorage>>,
    cancel: &CancellationToken,
) -> Result<i32, BrokerManagementError> {
    let config=host.config.resolve(cancel).await.map_err(|e|error(e.to_string()))?.ok_or_else(||error("ARA_AUTH_BROKER_URL must be set (or `auth.broker.url` in config.yml). `migrate` uploads local credentials to a configured broker."))?;
    if !from_local {
        return Err(error(
            "`ara auth-broker migrate` requires an explicit source. Pass `--from-local` to migrate from the local SQLite store and env vars.",
        ));
    }
    let client = client(host, &config.url, &config.token)?;
    let snapshot = match client.fetch_snapshot(None, None, cancel).await.map_err(|e| error(e.to_string()))? {
        BrokerSnapshotResult::Snapshot { snapshot, .. } => snapshot,
        BrokerSnapshotResult::NotModified { .. } => return Err(error("Auth broker returned no snapshot")),
    };
    let mut existing = BTreeMap::<String, BTreeSet<String>>::new();
    for entry in snapshot.credentials {
        existing.entry(entry.provider).or_default().extend(identity_markers(&entry.credential));
    }
    let has = |provider: &str, credential: &AuthCredential| {
        existing.get(provider).is_some_and(|ids| !identity_markers(credential).is_disjoint(ids))
    };
    let storage = local(host, retained).await?;
    let rows = BrokerStorageFacade::new(storage).migration_rows().await.map_err(|e| error(e.to_string()))?;
    let mut plan = Vec::<MigrateEntry>::new();
    let mut skipped = Vec::new();
    let mut planned_api = BTreeSet::new();
    for row in rows {
        let identity = credential_identity(&row.provider, &row.credential);
        let reason = if matches!(&row.credential,AuthCredential::ApiKey {key,..} if key=="<authenticated>") {
            Some("placeholder sentinel '<authenticated>' is not a real key")
        } else if matches!(&row.credential, AuthCredential::OAuth { .. }) && !include_oauth {
            Some("OAuth from local SQLite skipped by default (use --include-oauth)")
        } else if has(&row.provider, &row.credential) {
            Some("already on broker")
        } else if matches!(&row.credential, AuthCredential::ApiKey { .. }) && !planned_api.insert(row.provider.clone())
        {
            Some("another local api_key for this provider already planned")
        } else {
            None
        };
        if let Some(reason) = reason {
            skipped.push(json!({"source":"local-sqlite","provider":row.provider,"identity":identity,"reason":reason}));
        } else {
            plan.push(MigrateEntry {
                source: "local-sqlite",
                provider: row.provider,
                credential: row.credential,
                identity,
            });
        }
    }
    if include_env {
        for descriptor in crate::provider_models::catalog_providers()
            .iter()
            .filter(|p| p.env_vars.as_ref().is_some_and(|v| !v.is_empty()))
        {
            let Ok(provider) = descriptor.id.to_utf8() else {
                continue;
            };
            let Some(key) = host
                .environment
                .api_key(&provider)
                .map(|key| key.value)
                .filter(|v| !v.is_empty() && v != "<authenticated>")
            else {
                continue;
            };
            let credential = AuthCredential::api_key(key);
            let reason = if has(&provider, &credential) {
                Some("already on broker (provider has an api_key)")
            } else if planned_api.contains(&provider) {
                Some("local SQLite already supplied an api_key for this provider")
            } else {
                None
            };
            if let Some(reason) = reason {
                skipped.push(json!({"source":"env","provider":provider,"identity":"(api key)","reason":reason}));
            } else {
                plan.push(MigrateEntry { source: "env", provider, credential, identity: "(api key)".into() });
            }
        }
    }
    if as_json {
        line(
            host,
            json!({"dryRun":dry_run,"plan":plan.iter().map(|entry|json!({"source":entry.source,"provider":entry.provider,"identity":entry.identity})).collect::<Vec<_>>(),"skipped":skipped}),
        )?;
    } else {
        for skip in &skipped {
            line(
                host,
                format!(
                    "skip [{}] {} {}: {}",
                    skip["source"].as_str().unwrap_or(""),
                    skip["provider"].as_str().unwrap_or(""),
                    skip["identity"].as_str().unwrap_or(""),
                    skip["reason"].as_str().unwrap_or("")
                ),
            )?;
        }
    }
    if plan.is_empty() {
        if !as_json {
            line(host, "Nothing to migrate.")?;
        }
        return Ok(0);
    }
    if dry_run {
        if !as_json {
            line(host, format!("Dry run — would upload {} credential(s):", plan.len()))?;
            for entry in plan {
                line(host, format!("  [{}] {} {}", entry.source, entry.provider, entry.identity))?;
            }
        }
        return Ok(0);
    }
    let mut exit = 0;
    for entry in plan {
        match client.upload_credential(&entry.provider, &entry.credential, cancel).await {
            Ok(_) => {
                if !as_json {
                    line(host, format!("uploaded [{}] {} {}", entry.source, entry.provider, entry.identity))?;
                }
            }
            Err(err) => {
                exit = 1;
                if as_json {
                    line(host, json!({"error":err.to_string(),"provider":entry.provider}))?;
                } else {
                    line(host, format!("failed [{}] {} {}: {err}", entry.source, entry.provider, entry.identity))?;
                }
                if err.is_cancelled() || err.is_outcome_unknown() {
                    break;
                }
            }
        }
    }
    Ok(exit)
}
