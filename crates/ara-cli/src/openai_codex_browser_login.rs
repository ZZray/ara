//! Fixed OMP OpenAI authorization-code issuance; persistence remains Host-owned.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d, openai-codex.kdl,
//! registry/engine/oauth-code.ts and registry/oauth/{callback-server,pkce}.ts.
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

use super::{AuthCredential, CLIENT_ID, CodexAuthError, exchange_code_credential};
use base64::Engine as _;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::json;
use std::{
    convert::Infallible,
    io::ErrorKind,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

const CALLBACK_PATH: &str = "/auth/callback";
const HTML: &str = include_str!("../data/oauth/openai-codex.html");
type Completion = Result<String, BrowserLoginError>;
type CompletionSender = oneshot::Sender<Completion>;
type ConnectionReceipt = Arc<Mutex<Option<(CompletionSender, Completion)>>>;

/// Browser-only safe categories do not expand the shared refresh error contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserLoginError {
    Authentication(CodexAuthError),
    CallbackPortInUse(u16),
    AuthorizationDenied,
}
impl From<CodexAuthError> for BrowserLoginError {
    fn from(error: CodexAuthError) -> Self {
        Self::Authentication(error)
    }
}
impl std::fmt::Display for BrowserLoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authentication(error) => std::fmt::Display::fmt(error, f),
            Self::CallbackPortInUse(port) => write!(
                f,
                "OAuth callback port {port} is in use. OpenAI requires http://localhost:{port}/auth/callback; release that port before signing in"
            ),
            Self::AuthorizationDenied => f.write_str("OpenAI authorization was denied"),
        }
    }
}
impl std::error::Error for BrowserLoginError {}

/// Public authorization URLs and guidance; no issued token or verifier.
#[derive(Clone)]
pub struct BrowserLoginInfo {
    pub url: String,
    pub launch_url: String,
    pub instructions: &'static str,
}

/// A no-DB issuer for the standard fixed OpenAI callback policy.
#[derive(Clone)]
pub struct OpenAiCodexBrowserLogin {
    client: reqwest::Client,
    auth_base_url: String,
    port: u16,
    timeout: Duration,
}

impl OpenAiCodexBrowserLogin {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            auth_base_url: "https://auth.openai.com".into(),
            port: 1455,
            timeout: Duration::from_millis(300_000),
        }
    }

    /// Controlled loopback transport and callback port, absent in ordinary builds.
    /// Port zero is fixture allocation, never production port fallback.
    #[cfg(feature = "test-fixture")]
    pub fn with_endpoints(
        client: reqwest::Client,
        auth_base_url: &str,
        callback_port: u16,
    ) -> Result<Self, CodexAuthError> {
        let url = reqwest::Url::parse(auth_base_url).map_err(|_| CodexAuthError::InvalidEndpoint)?;
        if !super::loopback_url(&url) || !matches!(url.path(), "" | "/") {
            return Err(CodexAuthError::InvalidEndpoint);
        }
        Ok(Self {
            client,
            auth_base_url: auth_base_url.trim_end_matches('/').into(),
            port: callback_port,
            timeout: Duration::from_millis(300_000),
        })
    }

    #[cfg(feature = "test-fixture")]
    pub fn with_callback_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Host owns browser/manual input and credential publication. Optional
    /// manual input competes with HTTP through the same first-valid slot.
    /// Cancel and await for a receipt; dropping does not prove grant rejection.
    pub async fn issue(
        &self,
        cancel: &CancellationToken,
        mut manual: Option<mpsc::UnboundedReceiver<String>>,
        mut on_auth: impl FnMut(BrowserLoginInfo) + Send,
        mut on_progress: impl FnMut(&str) + Send,
    ) -> Result<AuthCredential, BrowserLoginError> {
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled.into());
        }
        let rng = SystemRandom::new();
        let mut state_bytes = [0_u8; 16];
        let mut verifier_bytes = [0_u8; 96];
        rng.fill(&mut state_bytes).map_err(|_| CodexAuthError::Transport)?;
        rng.fill(&mut verifier_bytes).map_err(|_| CodexAuthError::Transport)?;
        let state: String = state_bytes.iter().map(|b| format!("{b:02x}")).collect();
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref());
        let (mut owner, mut completion, port) = CallbackOwner::start(self.port, state.clone(), cancel).await?;
        let redirect = format!("http://localhost:{port}{CALLBACK_PATH}");
        let result = async {
            if cancel.is_cancelled() {
                return Err(CodexAuthError::Cancelled.into());
            }
            let mut url = reqwest::Url::parse(&format!("{}/oauth/authorize", self.auth_base_url))
                .map_err(|_| CodexAuthError::InvalidEndpoint)?;
            url.query_pairs_mut().extend_pairs([
                ("client_id", CLIENT_ID),
                ("response_type", "code"),
                ("redirect_uri", redirect.as_str()),
                ("scope", "openid profile email offline_access api.connectors.read api.connectors.invoke"),
                ("code_challenge", challenge.as_str()),
                ("code_challenge_method", "S256"),
                ("state", state.as_str()),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
                ("originator", "omp"),
            ]);
            let url = url.to_string();
            *owner.shared.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(url.clone());
            // Receiver and first-winner owner already exist before Host code can
            // synchronously deliver a multi-threaded loopback callback.
            on_auth(BrowserLoginInfo {
                url,
                launch_url: format!("http://localhost:{port}/launch"),
                instructions: "A browser window should open. Complete login to finish.",
            });
            on_progress("Waiting for browser authentication...");
            let timeout = tokio::time::sleep(self.timeout);
            tokio::pin!(timeout);
            let code = loop {
                tokio::select! { biased;
                    _ = cancel.cancelled() => return Err(CodexAuthError::Cancelled.into()),
                    _ = &mut timeout => return Err(CodexAuthError::TimedOut.into()),
                    result = &mut completion => break result.map_err(|_| CodexAuthError::Transport)??,
                    input = async { match &mut manual {
                        Some(input) => input.recv().await,
                        None => std::future::pending().await,
                    }} => {
                        match input {
                            None => manual = None,
                            Some(input) => {
                                let (code, input_state) = parse_callback_input(&input);
                                if let Some(code) = code.filter(|code| !code.is_empty()) {
                                    if input_state.as_deref().is_none_or(|value| value.is_empty() || value == state) {
                                        if let Some(sender) = owner.shared.reserve() { let _ = sender.send(Ok(code)); }
                                        continue;
                                    }
                                }
                                on_progress("Paste the authorization code or redirect URL again.");
                            }
                        }
                    }
                }
            };
            if cancel.is_cancelled() {
                return Err(CodexAuthError::Cancelled.into());
            }
            on_progress("Exchanging authorization code for tokens...");
            // The fixed engine also strips an echoed code#state fragment.
            let exchange_code = code.split('#').next().unwrap_or_default();
            exchange_code_credential(&self.client, &self.auth_base_url, &redirect, exchange_code, &verifier, cancel)
                .await
                .map_err(BrowserLoginError::from)
        }
        .await;
        owner.close().await;
        result
    }
}

struct CallbackShared {
    state: String,
    pending: Mutex<Option<String>>,
    winner: Mutex<Option<CompletionSender>>,
    stop: CancellationToken,
}
impl CallbackShared {
    fn reserve(&self) -> Option<CompletionSender> {
        self.winner.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
    fn deactivate(&self) {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.stop.cancel();
    }
}

struct CallbackOwner {
    shared: Arc<CallbackShared>,
    tasks: JoinSet<()>,
}
impl CallbackOwner {
    async fn start(
        port: u16,
        state: String,
        cancel: &CancellationToken,
    ) -> Result<(Self, oneshot::Receiver<Completion>, u16), BrowserLoginError> {
        let bind = async {
            let primary = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .map_err(|error| bind_error(error.kind(), port))?;
            let port = primary.local_addr().map_err(|_| CodexAuthError::Transport)?.port();
            let companion = match TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await {
                Ok(listener) => Some(listener),
                Err(error) if error.kind() == ErrorKind::AddrInUse => {
                    return Err(BrowserLoginError::CallbackPortInUse(port));
                }
                // Fixed source preserves IPv4 when the companion is unavailable;
                // only a genuine companion collision invalidates the fixed port.
                Err(_) => None,
            };
            Ok((primary, companion, port))
        };
        let (primary, companion, port) = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(CodexAuthError::Cancelled.into()),
            bound = bind => bound?,
        };
        let (sender, receiver) = oneshot::channel();
        let shared = Arc::new(CallbackShared {
            state,
            pending: Mutex::new(None),
            winner: Mutex::new(Some(sender)),
            stop: CancellationToken::new(),
        });
        let mut tasks = JoinSet::new();
        tasks.spawn(serve(primary, shared.clone()));
        if let Some(listener) = companion {
            tasks.spawn(serve(listener, shared.clone()));
        }
        Ok((Self { shared, tasks }, receiver, port))
    }
    async fn close(&mut self) {
        self.shared.deactivate();
        // Accept loops join their connection owners before returning.
        while self.tasks.join_next().await.is_some() {}
    }
}
impl Drop for CallbackOwner {
    fn drop(&mut self) {
        self.shared.deactivate();
    }
}

fn bind_error(kind: ErrorKind, port: u16) -> BrowserLoginError {
    if kind == ErrorKind::AddrInUse {
        BrowserLoginError::CallbackPortInUse(port)
    } else {
        CodexAuthError::Transport.into()
    }
}

async fn serve(listener: TcpListener, shared: Arc<CallbackShared>) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! { biased;
            _ = shared.stop.cancelled() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            socket = listener.accept() => {
                let Ok((socket, _)) = socket else { break };
                let shared = shared.clone();
                connections.spawn(async move {
                    let receipt: ConnectionReceipt = Arc::new(Mutex::new(None));
                    let response_receipt = receipt.clone();
                    let service_shared = shared.clone();
                    let service = service_fn(move |request| {
                        let response = handle(request, &service_shared, &response_receipt);
                        async { Ok::<_, Infallible>(response) }
                    });
                    // No keepalive: the connection future settles after the
                    // complete feedback response is written, not just its header.
                    let _ = http1::Builder::new().keep_alive(false)
                        .serve_connection(TokioIo::new(socket), service).await;
                    if let Some((sender, result)) = receipt.lock().unwrap_or_else(|e| e.into_inner()).take() {
                        let _ = sender.send(result);
                    }
                });
            }
        }
    }
    connections.shutdown().await;
}

fn handle(request: Request<Incoming>, shared: &CallbackShared, receipt: &ConnectionReceipt) -> Response<Full<Bytes>> {
    if request.uri().path() != CALLBACK_PATH {
        if request.uri().path() == "/launch" {
            let pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            return match pending.as_deref() {
                Some(url) => Response::builder()
                    .status(302)
                    .header("Location", url)
                    .body(Full::new(Bytes::new()))
                    .expect("encoded authorization URL"),
                None => response(503, "text/plain", "OAuth launch URL is no longer active".into()),
            };
        }
        return response(404, "text/plain", "Not Found".into());
    }
    let url = reqwest::Url::parse(&format!(
        "http://localhost{}",
        request.uri().path_and_query().map_or("/", |value| value.as_str())
    ))
    .expect("HTTP URI path and query");
    let param = |name: &str| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
    let code = param("code");
    let state = param("state").unwrap_or_default();
    let error = param("error").unwrap_or_default();
    let error_description =
        param("error_description").filter(|value| !value.is_empty()).unwrap_or_else(|| error.clone());
    let valid = error.is_empty() && code.as_ref().is_some_and(|value| !value.is_empty()) && state == shared.state;
    let feedback = if !error.is_empty() {
        json!({"ok":false,"error":format!("Authorization failed: {error_description}")})
    } else if code.as_ref().is_none_or(|value| value.is_empty()) {
        json!({"ok":false,"error":"Missing authorization code"})
    } else if state != shared.state {
        json!({"ok":false,"error":"State mismatch - possible CSRF attack"})
    } else {
        json!({"ok":true,"code":code.as_deref(),"state":state.as_str()})
    };
    let completion = if valid {
        Some(Ok(code.unwrap_or_default()))
    } else if !error.is_empty() && state == shared.state {
        Some(Err(BrowserLoginError::AuthorizationDenied))
    } else {
        None
    };
    if let Some(result) = completion {
        if let Some(sender) = shared.reserve() {
            // Arm the result on this request but publish only after its complete
            // response settles. The winner's HTML cannot be aborted by itself.
            *receipt.lock().unwrap_or_else(|e| e.into_inner()) = Some((sender, result));
        }
    }
    let encoded = feedback.to_string().replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026");
    response(if valid { 200 } else { 500 }, "text/html", HTML.replace("__OAUTH_STATE__", &encoded))
}

fn response(status: u16, content_type: &str, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .body(Full::new(Bytes::from(body)))
        .expect("static HTTP feedback")
}

/// Fixed parser: complete URL, query or raw code#state; no added host/character gate.
fn parse_callback_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    let url = reqwest::Url::parse(value).ok().or_else(|| {
        value
            .contains("code=")
            .then(|| reqwest::Url::parse(&format!("http://localhost/?{}", value.trim_start_matches(['?', '#']))).ok())
            .flatten()
    });
    if let Some(url) = url {
        let param = |name: &str| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
        return (param("code"), param("state"));
    }
    let mut fields = value.splitn(2, '#');
    (fields.next().map(str::to_owned), fields.next().map(str::to_owned))
}
