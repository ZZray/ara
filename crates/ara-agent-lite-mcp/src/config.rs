//! Claude Code `.mcp.json` configuration (requirements 42.2, 42.8).
//!
//! Parsing checks structure only; `${VAR}` / `${VAR:-default}` expansion runs
//! per server at connect time so one invalid server only fails that server.
//! Errors name servers, fields and variables, never configured values.
use std::{collections::BTreeMap, fmt};

use serde_json::{Map, Value};

use crate::text::Redactor;

/// `{"mcpServers": {<name>: <server>}}`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    pub mcp_servers: BTreeMap<String, ServerConfig>,
}

/// One server entry. `Debug` shows `env`/`headers` keys only.
#[derive(Clone, PartialEq, Eq)]
pub enum ServerConfig {
    /// `type` omitted or `"stdio"`.
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String> },
    /// `type: "http"` (Streamable HTTP).
    Http { url: String, headers: BTreeMap<String, String> },
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio { args, env, .. } => f
                .debug_struct("Stdio")
                .field("args", &args.len())
                .field("env_keys", &env.keys().collect::<Vec<_>>())
                .finish_non_exhaustive(),
            Self::Http { headers, .. } => f
                .debug_struct("Http")
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .finish_non_exhaustive(),
        }
    }
}

impl fmt::Debug for McpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpConfig").field("mcp_servers", &self.mcp_servers).finish()
    }
}

/// Configuration error. The text contains server names, field names and
/// variable names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Invalid { server: Option<String>, problem: String },
    UnsupportedTransport { server: String, transport: String },
    UnsupportedField { server: String, field: String },
    MissingVariable { server: String, variables: Vec<String> },
}

impl ConfigError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "invalid_config",
            Self::UnsupportedTransport { .. } => "unsupported_transport",
            Self::UnsupportedField { .. } => "unsupported_field",
            Self::MissingVariable { .. } => "missing_variable",
        }
    }

    pub fn server(&self) -> Option<&str> {
        match self {
            Self::Invalid { server, .. } => server.as_deref(),
            Self::UnsupportedTransport { server, .. }
            | Self::UnsupportedField { server, .. }
            | Self::MissingVariable { server, .. } => Some(server),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { server: Some(server), problem } => {
                write!(f, "invalid_config: server \"{server}\": {problem}")
            }
            Self::Invalid { server: None, problem } => write!(f, "invalid_config: {problem}"),
            Self::UnsupportedTransport { server, transport } => {
                write!(f, "unsupported_transport: server \"{server}\" uses transport \"{transport}\"")
            }
            Self::UnsupportedField { server, field } => {
                write!(f, "unsupported_field: server \"{server}\" sets unsupported field \"{field}\"")
            }
            Self::MissingVariable { server, variables } => write!(
                f,
                "missing_variable: server \"{server}\" references unset environment variable(s) without default: {}",
                variables.join(", ")
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Transports Claude Code knows but Lite does not implement.
const KNOWN_UNSUPPORTED: [&str; 6] = ["sse", "sse-ide", "ws", "ws-ide", "sdk", "claudeai-proxy"];

fn name(text: &str) -> String {
    crate::text::bounded(&crate::text::sanitize(text), 64)
}

impl McpConfig {
    /// Parse `.mcp.json` text.
    pub fn from_json_str(text: &str) -> Result<Self, ConfigError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|_| ConfigError::Invalid { server: None, problem: "configuration is not valid JSON".into() })?;
        Self::from_value(&value)
    }

    /// Parse a `.mcp.json` document `{"mcpServers": {...}}`.
    pub fn from_value(value: &Value) -> Result<Self, ConfigError> {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigError::Invalid { server: None, problem: "configuration must be an object".into() })?;
        if let Some(field) = object.keys().find(|key| key.as_str() != "mcpServers") {
            return Err(ConfigError::Invalid {
                server: None,
                problem: format!("unknown top-level field \"{}\"", name(field)),
            });
        }
        let servers = object
            .get("mcpServers")
            .ok_or_else(|| ConfigError::Invalid { server: None, problem: "missing \"mcpServers\"".into() })?;
        Self::from_servers(servers)
    }

    /// Parse the inner `{<name>: <server>}` map (the JSONL `mcp_servers` field).
    pub fn from_servers(servers: &Value) -> Result<Self, ConfigError> {
        let servers = servers
            .as_object()
            .ok_or_else(|| ConfigError::Invalid { server: None, problem: "\"mcpServers\" must be an object".into() })?;
        let mut mcp_servers = BTreeMap::new();
        for (server, entry) in servers {
            if server.is_empty() {
                return Err(ConfigError::Invalid { server: None, problem: "server name must not be empty".into() });
            }
            mcp_servers.insert(server.clone(), parse_server(server, entry)?);
        }
        Ok(Self { mcp_servers })
    }
}

fn parse_server(server: &str, entry: &Value) -> Result<ServerConfig, ConfigError> {
    let invalid = |problem: &str| ConfigError::Invalid { server: Some(server.into()), problem: problem.into() };
    let object = entry.as_object().ok_or_else(|| invalid("server entry must be an object"))?;
    let transport = match object.get("type") {
        None => "stdio",
        Some(Value::String(kind)) => kind.as_str(),
        Some(_) => return Err(invalid("\"type\" must be a string")),
    };
    let allowed: &[&str] = match transport {
        "stdio" => &["type", "command", "args", "env"],
        "http" => &["type", "url", "headers"],
        other => {
            let transport = if KNOWN_UNSUPPORTED.contains(&other) { other.to_string() } else { name(other) };
            return Err(ConfigError::UnsupportedTransport { server: server.into(), transport });
        }
    };
    if let Some(field) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ConfigError::UnsupportedField { server: server.into(), field: name(field) });
    }
    let string = |field: &str| match object.get(field) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(Value::String(_)) => Err(invalid(&format!("\"{field}\" must not be empty"))),
        Some(_) => Err(invalid(&format!("\"{field}\" must be a string"))),
        None => Err(invalid(&format!("missing \"{field}\""))),
    };
    let map = |field: &str| -> Result<BTreeMap<String, String>, ConfigError> {
        let Some(value) = object.get(field) else { return Ok(BTreeMap::new()) };
        let entries: &Map<String, Value> =
            value.as_object().ok_or_else(|| invalid(&format!("\"{field}\" must be an object of strings")))?;
        entries
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) if !key.is_empty() && !key.contains(['=', '\0']) => Ok((key.clone(), text.clone())),
                Value::String(_) => Err(invalid(&format!("\"{field}\" contains an invalid key"))),
                _ => Err(invalid(&format!("\"{field}\" values must be strings"))),
            })
            .collect()
    };
    Ok(match transport {
        "stdio" => {
            let args = match object.get("args") {
                None => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string).ok_or_else(|| invalid("\"args\" must be strings")))
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(invalid("\"args\" must be an array of strings")),
            };
            ServerConfig::Stdio { command: string("command")?, args, env: map("env")? }
        }
        _ => ServerConfig::Http { url: string("url")?, headers: map("headers")? },
    })
}

/// Expand `${VAR}` and `${VAR:-default}` like Claude Code
/// `expandEnvVarsInString`: the pattern is `\$\{([^}]+)\}`, the content is
/// split on `:-` keeping at most two parts, a set variable wins (even when
/// empty), then the default; otherwise the reference is kept and reported
/// missing. `used` receives every substituted variable value.
pub fn expand_str(
    template: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
    used: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) if end > 0 => {
                let content = &after[..end];
                let mut parts = content.splitn(3, ":-");
                let variable = parts.next().unwrap_or_default();
                let default = parts.next();
                match (lookup(variable), default) {
                    (Some(value), _) => {
                        out.push_str(&value);
                        used.push(value);
                    }
                    (None, Some(default)) => out.push_str(default),
                    (None, None) => {
                        if !missing.iter().any(|known| known == variable) {
                            missing.push(variable.to_string());
                        }
                        out.push_str(&rest[start..start + 2 + end + 1]);
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('$');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Host process environment lookup used at connect time.
pub fn process_env(variable: &str) -> Option<String> {
    if variable.is_empty() || variable.contains(['=', '\0']) {
        return None;
    }
    std::env::var(variable).ok()
}

/// Expand every expandable field of one server (`command`, `args`, `env`
/// values, `url`, `headers` values).
pub fn expand_server(
    server: &str,
    config: &ServerConfig,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<ServerConfig, ConfigError> {
    expand_with_secrets(server, config, lookup).map(|(config, _)| config)
}

pub(crate) fn expand_with_secrets(
    server: &str,
    config: &ServerConfig,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(ServerConfig, Redactor), ConfigError> {
    let mut missing = Vec::new();
    let mut used = Vec::new();
    let mut expand = |text: &str| expand_str(text, lookup, &mut missing, &mut used);
    let (expanded, mut secrets) = match config {
        ServerConfig::Stdio { command, args, env } => {
            let command = expand(command);
            let args = args.iter().map(|arg| expand(arg)).collect();
            let env: BTreeMap<_, _> = env.iter().map(|(key, value)| (key.clone(), expand(value))).collect();
            let secrets = env.values().cloned().collect::<Vec<_>>();
            (ServerConfig::Stdio { command, args, env }, secrets)
        }
        ServerConfig::Http { url, headers } => {
            let url = expand(url);
            let headers: BTreeMap<_, _> = headers.iter().map(|(key, value)| (key.clone(), expand(value))).collect();
            let secrets = headers.values().cloned().collect::<Vec<_>>();
            (ServerConfig::Http { url, headers }, secrets)
        }
    };
    if !missing.is_empty() {
        return Err(ConfigError::MissingVariable {
            server: server.into(),
            variables: missing.iter().map(|variable| name(variable)).collect(),
        });
    }
    secrets.extend(used);
    // Authorization-style values often carry the credential after a scheme word.
    let words = secrets
        .iter()
        .flat_map(|secret| secret.split_whitespace().skip(1).map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    secrets.extend(words);
    Ok((expanded, Redactor::new(secrets)))
}
