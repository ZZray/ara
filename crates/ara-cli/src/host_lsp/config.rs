// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
// Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
// The upstream MIT permission and disclaimer are reproduced in types.rs.

use ara_discovery::config_files::ConfigLevel;
use ara_discovery::{Discovery, HostDirs, ProviderPolicy};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use yaml_rust2::{Yaml, YamlLoader};

use super::types::{absolute, error};

#[derive(Clone, Debug, PartialEq)]
pub struct ServerConfig {
    pub name: String,
    pub custom: bool,
    pub command: String,
    pub resolved_command: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub file_types: Vec<String>,
    pub root_markers: Vec<String>,
    pub language_id: Option<String>,
    pub is_linter: bool,
    pub init_options: Value,
    pub settings: Value,
    pub warmup_timeout_ms: Option<u64>,
    pub workspace_ready_timings: Value,
    /// Includes optional upstream fields, so snapshot comparison notices changes.
    pub raw: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LspConfig {
    pub servers: Vec<ServerConfig>,
    pub idle_timeout_ms: Option<u64>,
    pub source_snapshot: Vec<(PathBuf, Value)>,
    pub unresolved_plugin_sources: bool,
}

pub(super) fn yaml_value(value: &Yaml) -> Value {
    match value {
        Yaml::Boolean(v) => Value::Bool(*v),
        Yaml::Integer(v) => Value::from(*v),
        Yaml::Real(v) => {
            v.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::Null)
        }
        Yaml::String(v) => Value::String(v.clone()),
        Yaml::Array(v) => Value::Array(v.iter().map(yaml_value).collect()),
        Yaml::Hash(v) => {
            Value::Object(v.iter().filter_map(|(k, v)| k.as_str().map(|k| (k.into(), yaml_value(v)))).collect())
        }
        _ => Value::Null,
    }
}
pub(super) fn read_json_yaml(path: &Path) -> Option<Value> {
    let content = fs::read_to_string(path).ok()?;
    if matches!(path.extension().and_then(|v| v.to_str()), Some("yml" | "yaml")) {
        YamlLoader::load_from_str(&content).ok()?.first().map(yaml_value)
    } else {
        serde_json::from_str(&content).ok()
    }
}
pub(super) fn home_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}
fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let list = value?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!list.is_empty()).then_some(list)
}
fn normalized(raw: &Value) -> Option<Value> {
    let mut raw = raw.as_object()?.clone();
    let command = raw.get("command")?.as_str()?;
    if command.is_empty() {
        return None;
    }
    let file_types = string_array(raw.get("fileTypes")).or_else(|| {
        let value = raw.get("extensionToLanguage")?.as_object()?;
        let keys = value.keys().filter(|v| !v.is_empty()).cloned().collect::<Vec<_>>();
        (!keys.is_empty()).then_some(keys)
    })?;
    let root_markers = string_array(raw.get("rootMarkers"))
        .or_else(|| raw.contains_key("extensionToLanguage").then(|| vec![".".into()]))?;
    raw.insert("fileTypes".into(), Value::from(file_types));
    raw.insert("rootMarkers".into(), Value::from(root_markers));
    if let Some(args) = raw.get("args").and_then(Value::as_array) {
        let args = args.iter().filter(|v| v.is_string()).cloned().collect::<Vec<_>>();
        raw.insert("args".into(), Value::Array(args));
    }
    let init = raw
        .get("initOptions")
        .filter(|v| v.is_object())
        .or_else(|| raw.get("initializationOptions").filter(|v| v.is_object()))
        .cloned();
    if let Some(init) = init {
        raw.insert("initOptions".into(), init);
    }
    Some(Value::Object(raw))
}

pub fn has_root_markers(cwd: &Path, markers: &[String]) -> bool {
    let mut entries = None;
    for marker in markers {
        if marker.contains('*') {
            let entries = entries.get_or_insert_with(|| {
                fs::read_dir(cwd)
                    .map(|list| list.filter_map(Result::ok).map(|entry| entry.file_name()).collect::<Vec<_>>())
                    .unwrap_or_default()
            });
            if let Ok(glob) = globset::Glob::new(marker) {
                let matcher = glob.compile_matcher();
                if entries.iter().any(|entry| matcher.is_match(Path::new(entry))) {
                    return true;
                }
            }
        } else if cwd.join(marker).exists() {
            return true;
        }
    }
    false
}
pub(super) fn has_root_marker_ancestor(path: &Path, markers: &[String]) -> bool {
    let path = absolute(path);
    let mut dir = path.parent();
    while let Some(current) = dir {
        if has_root_markers(current, markers) {
            return true;
        }
        dir = current.parent();
    }
    false
}
fn local_executable(base: PathBuf) -> Option<PathBuf> {
    if base.exists() {
        return Some(base);
    }
    if cfg!(windows) {
        for ext in [".exe", ".cmd", ".bat"] {
            let path = PathBuf::from(format!("{}{ext}", base.to_string_lossy()));
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}
pub(super) fn resolve_command(command: &str, cwd: &Path) -> Option<PathBuf> {
    let python = [
        "pyproject.toml",
        "ty.toml",
        "requirements.txt",
        "setup.py",
        "setup.cfg",
        "Pipfile",
        "pyrightconfig.json",
        "ruff.toml",
        ".ruff.toml",
    ];
    let mut bins =
        vec![(vec!["package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"], "node_modules/.bin")];
    for dir in [".venv/bin", ".venv/Scripts", "venv/bin", "venv/Scripts", ".env/bin", ".env/Scripts"] {
        bins.push((python.to_vec(), dir));
    }
    bins.extend([
        (vec!["Gemfile", "Gemfile.lock"], "vendor/bundle/bin"),
        (vec!["Gemfile", "Gemfile.lock"], "bin"),
        (vec!["go.mod", "go.sum", "go.work"], "bin"),
    ]);
    for (markers, bin) in bins {
        if has_root_markers(cwd, &markers.into_iter().map(str::to_string).collect::<Vec<_>>()) {
            if let Some(found) = local_executable(cwd.join(bin).join(command)) {
                return Some(absolute(&found));
            }
        }
    }
    let path = Path::new(command);
    if path.is_absolute() || path.components().count() > 1 {
        return local_executable(cwd.join(path)).map(|v| absolute(&v));
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if let Some(found) = local_executable(dir.join(command)) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(&found).ok()?.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Some(absolute(&found));
        }
    }
    None
}
fn native_typescript(path: &Path) -> bool {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let bin = path.parent().unwrap_or(Path::new("."));
    let mut candidates = Vec::new();
    if real.parent().and_then(Path::file_name).is_some_and(|v| v == "bin") {
        if let Some(dir) = real.parent().and_then(Path::parent) {
            candidates.push(dir.to_owned());
        }
    }
    candidates.extend([bin.join("../typescript"), bin.join("node_modules/typescript")]);
    candidates
        .into_iter()
        .find(|v| v.join("package.json").exists())
        .is_some_and(|v| !v.join("lib/tsserver.js").exists())
}

pub fn load_config(cwd: &Path, agent_dir: &Path) -> io::Result<LspConfig> {
    let home = home_dir();
    let mut dirs = HostDirs::ara(&home).with_env(|name| std::env::var(name).ok());
    dirs.native_user_dir = absolute(agent_dir);
    let mut policy = ProviderPolicy::default();
    let mut settings = None;
    for name in ["config.yml", "config.yaml", "settings.json"] {
        if let Some(value) = read_json_yaml(&agent_dir.join(name)) {
            settings = Some(value);
            break;
        }
    }
    if let Some(value) = &settings {
        policy.enabled_user_sources =
            string_array(value.get("enabledProviders")).unwrap_or_default().into_iter().collect();
        policy.disabled = string_array(value.get("disabledProviders")).unwrap_or_default().into_iter().collect();
    }
    let discovery = Discovery::new(&home, dirs, policy);
    let filenames = ["lsp.json", ".lsp.json", "lsp.yaml", ".lsp.yaml", "lsp.yml", ".lsp.yml"];
    let mut roots = vec![absolute(cwd)];
    roots.extend(discovery.config_dirs(cwd, ConfigLevel::Project));
    roots.extend(discovery.config_dirs(cwd, ConfigLevel::User));
    roots.push(home);
    let sources = roots.into_iter().flat_map(|dir| filenames.map(|name| dir.join(name))).collect::<Vec<_>>();
    let defaults: Value = serde_json::from_str(include_str!("defaults.json")).map_err(io::Error::other)?;
    let mut servers = defaults
        .as_object()
        .ok_or_else(|| error("invalid bundled LSP defaults"))?
        .iter()
        .filter_map(|(name, value)| normalized(value).map(|value| (name.clone(), value)))
        .collect::<Map<_, _>>();
    let mut snapshot = Vec::new();
    if let Some(settings) = settings {
        let leaves = ["enabledProviders", "disabledProviders"]
            .into_iter()
            .filter_map(|key| settings.get(key).map(|value| (key.to_string(), value.clone())))
            .collect::<Map<_, _>>();
        snapshot.push((absolute(agent_dir), Value::Object(leaves)));
    }
    let mut idle_timeout_ms = None;
    for path in sources.into_iter().rev() {
        let Some(value) = read_json_yaml(&path).filter(Value::is_object) else {
            continue;
        };
        snapshot.push((path, value.clone()));
        if let Some(value) = value.get("idleTimeoutMs").and_then(Value::as_u64) {
            idle_timeout_ms = Some(value);
        }
        let overrides = value.get("servers").and_then(Value::as_object).or_else(|| value.as_object()).unwrap();
        for (name, raw) in overrides {
            if name == "idleTimeoutMs" || !raw.is_object() {
                continue;
            }
            let mut candidate = servers.get(name).and_then(Value::as_object).cloned().unwrap_or_default();
            candidate.extend(raw.as_object().unwrap().clone());
            if let Some(value) = normalized(&Value::Object(candidate)) {
                servers.insert(name.clone(), value);
            }
        }
    }
    let mut configs = Vec::new();
    for (name, raw) in servers {
        if raw.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let markers = string_array(raw.get("rootMarkers")).unwrap_or_default();
        if !has_root_markers(cwd, &markers) {
            continue;
        }
        let command = raw["command"].as_str().unwrap().to_string();
        let Some(resolved_command) = resolve_command(&command, cwd) else {
            continue;
        };
        let mut args = raw
            .get("args")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        if name == "omnisharp" {
            for arg in &mut args {
                if arg == "$PID" {
                    *arg = std::process::id().to_string();
                }
            }
        }
        let env = raw
            .get("env")
            .and_then(Value::as_object)
            .map(|v| {
                v.iter()
                    .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        configs.push(ServerConfig {
            custom: matches!(name.as_str(), "biome" | "swiftlint"),
            name,
            command,
            resolved_command,
            args,
            env,
            file_types: string_array(raw.get("fileTypes")).unwrap_or_default(),
            root_markers: markers,
            language_id: raw.get("languageId").and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string),
            is_linter: raw.get("isLinter").and_then(Value::as_bool).unwrap_or(false),
            init_options: raw.get("initOptions").cloned().unwrap_or_else(|| Value::Object(Map::new())),
            settings: raw.get("settings").cloned().unwrap_or_else(|| Value::Object(Map::new())),
            warmup_timeout_ms: raw.get("warmupTimeoutMs").and_then(Value::as_u64),
            workspace_ready_timings: raw.get("workspaceReadyTimings").cloned().unwrap_or(Value::Null),
            raw,
        });
    }
    if let Some(native) = configs.iter().find(|v| v.name == "typescript-native") {
        let remove = if native_typescript(&native.resolved_command) {
            "typescript-language-server"
        } else {
            "typescript-native"
        };
        configs.retain(|v| v.name != remove);
    }
    Ok(LspConfig { servers: configs, idle_timeout_ms, source_snapshot: snapshot, unresolved_plugin_sources: true })
}
pub fn servers_for_file(config: &LspConfig, path: &Path) -> Vec<ServerConfig> {
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or_default().to_lowercase();
    let filename = path.file_name().and_then(|v| v.to_str()).unwrap_or_default().to_lowercase();
    let mut matches = config
        .servers
        .iter()
        .filter(|server| {
            server.file_types.iter().any(|kind| {
                let kind = kind.to_lowercase();
                let plain = kind.trim_start_matches('.');
                kind == filename || plain == filename || plain == extension
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by_key(|v| v.is_linter);
    matches
}

pub(super) fn project_aware(server: &ServerConfig) -> bool {
    !server.custom && !server.is_linter
}
