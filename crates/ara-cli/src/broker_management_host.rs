//! Reference CLI binding for Broker management; normal main teardown owns storage settlement.

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

use super::{ara_home, listen_for_interrupts, open_codex_auth};
use anyhow::Result;
use ara_cli::{
    auth_broker_discover::{BrokerConfigResolver, BrokerDiscoveryHost},
    auth_broker_management::{
        BrokerArgs, BrokerConsole, BrokerDeviceLoginPort, BrokerLocalAuthFactory, BrokerManagementError,
        BrokerManagementHost, ProcessBrokerConsole, run_auth_broker_command,
    },
    auth_storage::{AuthStorage, AuthStorageOptions, ConfigKeyResolver, Environment},
    credential_store::AuthCredential,
    openai_codex_auth::OpenAiCodexDeviceLogin,
};
use async_trait::async_trait;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;

struct LocalFactory {
    client: reqwest::Client,
    resolver: Arc<dyn ConfigKeyResolver>,
    environment: Arc<dyn Environment>,
}
#[async_trait]
impl BrokerLocalAuthFactory for LocalFactory {
    async fn open(&self) -> Result<Arc<AuthStorage>, BrokerManagementError> {
        let codex = open_codex_auth(self.client.clone()).await.map_err(|e| BrokerManagementError(e.to_string()))?;
        let storage = AuthStorage::for_codex(
            codex,
            AuthStorageOptions {
                config_key_resolver: self.resolver.clone(),
                environment: self.environment.clone(),
                ..Default::default()
            },
        )
        .map_err(|e| BrokerManagementError(e.to_string()))?;
        Ok(Arc::new(storage))
    }
}
struct DeviceLogin {
    client: reqwest::Client,
    console: Arc<dyn BrokerConsole>,
}
#[async_trait]
impl BrokerDeviceLoginPort for DeviceLogin {
    async fn issue(&self, cancel: &CancellationToken) -> Result<AuthCredential, BrokerManagementError> {
        #[cfg(feature = "test-fixture")]
        let issuer = match std::env::var("ARA_TEST_CODEX_AUTH_BASE_URL").ok() {
            Some(base) => OpenAiCodexDeviceLogin::with_endpoints(self.client.clone(), &base)
                .map_err(|e| BrokerManagementError(e.to_string()))?,
            None => OpenAiCodexDeviceLogin::new(self.client.clone()),
        };
        #[cfg(not(feature = "test-fixture"))]
        let issuer = OpenAiCodexDeviceLogin::new(self.client.clone());
        issuer
            .issue(cancel, |info| {
                let _ = self.console.write(&format!(
                    "\nOpen this URL in your browser:\n{}\nEnter code: {}\n\n",
                    info.verification_url, info.user_code
                ));
            })
            .await
            .map_err(|e| BrokerManagementError(e.to_string()))
    }
}
pub(super) async fn run(
    args: BrokerArgs,
    project_dir: &Path,
    account_auth: &mut Option<Arc<AuthStorage>>,
    cancel: &CancellationToken,
) -> Result<i32> {
    let resolver = Arc::new(ara_cli::config_request_auth::ConfigStorageKeyResolver::new(
        project_dir.to_path_buf(),
        Arc::new(ara_cli::model_config_values::ConfigValueResolver::new()),
        Arc::new(ara_cli::model_config_values::ProcessConfigEnvironment),
    ));
    let discovery = BrokerDiscoveryHost::for_process(ara_home(), Some(resolver.clone()));
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
    let environment: Arc<dyn Environment> =
        Arc::new(ara_cli::auth_storage_registry::CatalogAuthEnvironment(Arc::new(|name: &str| {
            std::env::var(name).ok().map(Into::into)
        })));
    let console: Arc<dyn BrokerConsole> = Arc::new(ProcessBrokerConsole);
    let host = BrokerManagementHost {
        config_root: ara_home(),
        version: env!("CARGO_PKG_VERSION").into(),
        client: client.clone(),
        clock: Arc::new(|| chrono::Utc::now().timestamp_millis() as f64),
        config: BrokerConfigResolver::new(discovery),
        local_auth: Arc::new(LocalFactory { client: client.clone(), resolver, environment: environment.clone() }),
        device_login: Arc::new(DeviceLogin { client, console: console.clone() }),
        environment,
        console,
        callback_ports: BTreeMap::new(),
    };
    let mut interrupts = listen_for_interrupts()?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let stop = cancel.clone();
    let listener = tokio::spawn(async move {
        #[cfg(unix)]
        let received = tokio::select! {
            received = interrupts.recv() => received,
            received = terminate.recv() => received,
        };
        #[cfg(not(unix))]
        let received = interrupts.recv().await;
        if received.is_some() {
            stop.cancel();
        }
    });
    let result = run_auth_broker_command(args, &host, account_auth, cancel).await;
    listener.abort();
    Ok(result?)
}
