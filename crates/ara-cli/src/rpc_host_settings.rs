//! Narrow native RPC policy, fixed OMP 596f2da settings.ts / utils/dirs.ts.
//! Host user configuration lives in ARA_HOME/agent, outside Core and Session.
//! Full YAML emission preserves loaded values but normalizes formatting and
//! does not preserve comments, anchors or custom tag syntax. The fixed writer
//! also stringifies its parsed configuration; this is not a round-trip editor.

use anyhow::{Context as _, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlEmitter, YamlLoader};

const MAIN_CONFIG_FILENAMES: [&str; 2] = ["config.yml", "config.yaml"];
const DEFAULT_METHOD_ORDER: [&str; 5] = ["remote", "snapcompact", "handoff", "shake", "soft"];

/// Fixed features.unexpectedStopDetection defaults to mechanical. Smart's
/// classifier route is a separate native tiny/smol or local-runtime dependency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum UnexpectedStopMode {
    None,
    #[default]
    Mechanical,
    Smart,
}

impl UnexpectedStopMode {
    pub(super) fn load(agent_dir: &Path) -> Result<Self> {
        for filename in MAIN_CONFIG_FILENAMES {
            if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
                let root = document.as_hash().context("native settings root must be a mapping")?;
                let group = match root.get(&Yaml::String("features".into())) {
                    None | Some(Yaml::Null) => return Ok(Self::default()),
                    Some(group) => group.as_hash().context("features settings must be a mapping")?,
                };
                return match group.get(&Yaml::String("unexpectedStopDetection".into())) {
                    None => Ok(Self::default()),
                    Some(Yaml::String(value)) if value == "none" => Ok(Self::None),
                    Some(Yaml::String(value)) if value == "mechanical" => Ok(Self::Mechanical),
                    Some(Yaml::String(value)) if value == "smart" => Ok(Self::Smart),
                    Some(_) => bail!("features.unexpectedStopDetection must be none, mechanical, or smart"),
                };
            }
        }
        Ok(Self::default())
    }
}

/// Fixed settings-schema contextPromotion.enabled defaults to false.
pub(super) fn context_promotion_enabled(agent_dir: &Path) -> Result<bool> {
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            let root = document.as_hash().context("native settings root must be a mapping")?;
            let Some(group) = root.get(&Yaml::String("contextPromotion".into())) else { return Ok(false) };
            if matches!(group, Yaml::Null) {
                return Ok(false);
            }
            let group = group.as_hash().context("contextPromotion must be a mapping")?;
            return match group.get(&Yaml::String("enabled".into())) {
                None => Ok(false),
                Some(Yaml::Boolean(enabled)) => Ok(*enabled),
                Some(_) => bail!("contextPromotion.enabled must be a boolean"),
            };
        }
    }
    Ok(false)
}

/// Fixed settings-schema.ts:1442-1473. Hosts bind these switches to their
/// provider guard and Gemini reminder; Core does not own native settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LoopGuardSettings {
    pub(super) enabled: bool,
    pub(super) check_assistant_content: bool,
    pub(super) tool_call_reminder: bool,
}

impl Default for LoopGuardSettings {
    fn default() -> Self {
        Self { enabled: true, check_assistant_content: true, tool_call_reminder: true }
    }
}

impl LoopGuardSettings {
    pub(super) fn load(agent_dir: &Path) -> Result<Self> {
        for filename in MAIN_CONFIG_FILENAMES {
            if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
                return Self::from_document(&document);
            }
        }
        Ok(Self::default())
    }

    fn from_document(document: &Yaml) -> Result<Self> {
        let root = document.as_hash().context("native settings root must be a mapping")?;
        let model = match root.get(&Yaml::String("model".into())) {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("model settings must be a mapping")?),
        };
        let guard = match model.and_then(|group| group.get(&Yaml::String("loopGuard".into()))) {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("model.loopGuard settings must be a mapping")?),
        };
        let boolean = |name: &str| -> Result<bool> {
            match guard.and_then(|group| group.get(&Yaml::String(name.into()))) {
                None => Ok(true),
                Some(Yaml::Boolean(value)) => Ok(*value),
                Some(_) => bail!("model.loopGuard.{name} must be a boolean"),
            }
        };
        Ok(Self {
            enabled: boolean("enabled")?,
            check_assistant_content: boolean("checkAssistantContent")?,
            tool_call_reminder: boolean("toolCallReminder")?,
        })
    }
}

/// Fixed settings-schema.ts:1476-1509 and stream-guards.ts:201-215.
/// The raw threshold remains a number; the detector owns truncation/clamping.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ToolLoopGuardSettings {
    pub(super) enabled: bool,
    pub(super) threshold: f64,
    pub(super) exempt_tools: Vec<String>,
}

impl Default for ToolLoopGuardSettings {
    fn default() -> Self {
        Self { enabled: true, threshold: 5.0, exempt_tools: vec!["hub".into()] }
    }
}

impl ToolLoopGuardSettings {
    pub(super) fn load(agent_dir: &Path) -> Result<Self> {
        for filename in MAIN_CONFIG_FILENAMES {
            if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
                return Self::from_document(&document);
            }
        }
        Ok(Self::default())
    }

    fn from_document(document: &Yaml) -> Result<Self> {
        let root = document.as_hash().context("native settings root must be a mapping")?;
        let model = match root.get(&Yaml::String("model".into())) {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("model settings must be a mapping")?),
        };
        let guard = match model.and_then(|group| group.get(&Yaml::String("toolCallLoopGuard".into()))) {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("model.toolCallLoopGuard settings must be a mapping")?),
        };
        let field = |key: &str| guard.and_then(|group| group.get(&Yaml::String(key.into())));
        let enabled = match field("enabled") {
            None => true,
            Some(Yaml::Boolean(value)) => *value,
            Some(_) => bail!("model.toolCallLoopGuard.enabled must be a boolean"),
        };
        let threshold = match field("threshold") {
            None => 5.0,
            Some(Yaml::Integer(value)) => *value as f64,
            Some(Yaml::Real(value)) => match value.as_str() {
                ".nan" | ".NaN" | ".NAN" => f64::NAN,
                ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => f64::INFINITY,
                "-.inf" | "-.Inf" | "-.INF" => f64::NEG_INFINITY,
                _ => value.parse::<f64>().context("model.toolCallLoopGuard.threshold must be a number")?,
            },
            Some(_) => bail!("model.toolCallLoopGuard.threshold must be a number"),
        };
        let exempt_tools = match field("exemptTools") {
            None => vec!["hub".into()],
            Some(Yaml::Array(values)) => {
                values.iter().filter_map(Yaml::as_str).filter(|name| !name.is_empty()).map(str::to_owned).collect()
            }
            Some(_) => bail!("model.toolCallLoopGuard.exemptTools must be an array"),
        };
        Ok(Self { enabled, threshold, exempt_tools })
    }

    pub(super) fn same_key(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.exempt_tools == other.exempt_tools
            && (self.threshold == other.threshold || self.threshold.is_nan() && other.threshold.is_nan())
    }
}

pub(super) struct AutoCompactionPolicy {
    path: Option<PathBuf>,
    enabled: bool,
}

pub(super) struct RecoveryCompactionSettings {
    pub reserve_tokens: Option<f64>,
    pub soft_available: bool,
    pub method_order: Vec<String>,
    pub supersede_reads: bool,
    pub drop_useless: bool,
    pub handoff_save_to_disk: bool,
    pub snapcompact_shape: String,
    pub remote: ara_agent::remote::RemoteSettings,
}

impl AutoCompactionPolicy {
    pub(super) fn load(agent_dir: &Path) -> Result<Self> {
        // Fixed first existing native filename wins; malformed or unreadable
        // primary configuration is an error, not permission to use a fallback.
        for filename in MAIN_CONFIG_FILENAMES {
            let path = agent_dir.join(filename);
            if let Some((document, _)) = read_document(&path)? {
                return Ok(Self { path: Some(path), enabled: read_enabled(&document)? });
            }
        }
        Ok(Self { path: Some(agent_dir.join(MAIN_CONFIG_FILENAMES[0])), enabled: true })
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn recovery_settings(&self) -> Result<RecoveryCompactionSettings> {
        let loaded = self.path.as_ref().map(|path| read_document(path)).transpose()?.flatten();
        let group = match loaded
            .as_ref()
            .and_then(|(document, _)| document.as_hash())
            .and_then(|root| root.get(&Yaml::String("compaction".into())))
        {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("compaction settings must be a mapping")?),
        };
        let field = |name: &str| group.and_then(|group| group.get(&Yaml::String(name.into())));
        let reserve_tokens = match field("reserveTokens") {
            None | Some(Yaml::Null) => None,
            Some(Yaml::Integer(value)) => Some(*value as f64),
            Some(Yaml::Real(value)) => Some(value.parse::<f64>().context("compaction.reserveTokens must be finite")?),
            Some(_) => bail!("compaction.reserveTokens must be a number"),
        };
        if reserve_tokens.is_some_and(|value| !value.is_finite()) {
            bail!("compaction.reserveTokens must be finite");
        }
        let method_order = match field("methodOrder") {
            None | Some(Yaml::Null) => DEFAULT_METHOD_ORDER.iter().map(|method| (*method).into()).collect(),
            Some(Yaml::Array(methods)) => methods.iter().filter_map(Yaml::as_str).map(str::to_owned).collect(),
            Some(_) => Vec::new(),
        };
        let boolean = |name: &str| -> Result<bool> {
            match field(name) {
                None | Some(Yaml::Null) => Ok(true),
                Some(Yaml::Boolean(value)) => Ok(*value),
                Some(_) => bail!("compaction.{name} must be a boolean"),
            }
        };
        let snapcompact = loaded
            .as_ref()
            .and_then(|(document, _)| document.as_hash())
            .and_then(|root| root.get(&Yaml::String("snapcompact".into())));
        let snapcompact_shape = match snapcompact {
            None | Some(Yaml::Null) => "auto".to_owned(),
            Some(value) => match value
                .as_hash()
                .context("snapcompact settings must be a mapping")?
                .get(&Yaml::String("shape".into()))
            {
                None | Some(Yaml::Null) => "auto".to_owned(),
                Some(Yaml::String(shape)) => shape.clone(),
                Some(_) => bail!("snapcompact.shape must be a string"),
            },
        };
        Ok(RecoveryCompactionSettings {
            reserve_tokens,
            soft_available: method_order.iter().any(|method| method == "soft"),
            method_order,
            supersede_reads: boolean("supersedeReads")?,
            drop_useless: boolean("dropUseless")?,
            snapcompact_shape,
            handoff_save_to_disk: match field("handoffSaveToDisk") {
                None | Some(Yaml::Null) => false,
                Some(Yaml::Boolean(enabled)) => *enabled,
                Some(_) => bail!("compaction.handoffSaveToDisk must be a boolean"),
            },
            remote: ara_agent::remote::RemoteSettings {
                enabled: boolean("remoteEnabled")?,
                streaming_v2_enabled: boolean("remoteStreamingV2Enabled")?,
                endpoint: match field("remoteEndpoint") {
                    None | Some(Yaml::Null) => None,
                    Some(Yaml::String(endpoint)) => Some(endpoint.clone()),
                    Some(_) => bail!("compaction.remoteEndpoint must be a string"),
                },
            },
        })
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        let Some(path) = &self.path else {
            self.enabled = enabled;
            return Ok(());
        };
        // Re-read immediately before the scoped mutation: unknown values and
        // disjoint external edits made since load are retained.
        let loaded = read_document(path)?;
        let mut document = loaded.as_ref().map(|(document, _)| document.clone()).unwrap_or_else(empty_mapping);
        read_enabled(&document)?;
        let root = document.as_mut_hash().expect("read_document validates mapping");
        let key = Yaml::String("compaction".into());
        let group = root.entry(key).or_insert_with(empty_mapping);
        if matches!(group, Yaml::Null) {
            *group = empty_mapping();
        }
        let group = group.as_mut_hash().context("compaction settings must be a mapping")?;
        group.insert(Yaml::String("enabled".into()), Yaml::Boolean(enabled));
        // Fixed setAutoCompactionEnabled repairs an explicitly empty order.
        let order_key = Yaml::String("methodOrder".into());
        if enabled && !has_compaction_methods(group.get(&order_key)) {
            group.insert(
                order_key,
                Yaml::Array(DEFAULT_METHOD_ORDER.iter().map(|method| Yaml::String((*method).into())).collect()),
            );
        }
        let mut encoded = String::new();
        YamlEmitter::new(&mut encoded).dump(&document).context("serializing native compaction settings")?;
        encoded.push('\n');
        write_atomically(path, &encoded, loaded.as_ref().map(|(_, source)| source.as_str()), "compaction")?;
        // A failed parse/read/write/rename never changes the live policy and
        // cannot be acknowledged by the Host as a successful setting change.
        self.enabled = read_enabled(&document)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn isolated(enabled: bool) -> Self {
        Self { path: None, enabled }
    }
}

/// Native retry policy for the Session-owned recovery saga. Fixed OMP
/// settings-schema.ts declares the limits as `number`, without integer or
/// nonnegative constraints. Keep those values as f64; the Host owns their
/// comparison/backoff semantics and checked conversion to a timer duration.
/// Credential/model fallback settings remain in the native document and are
/// not consumed by this same-route policy surface.
pub(super) struct RetryPolicy {
    path: Option<PathBuf>,
    enabled: bool,
    max_retries: f64,
    base_delay_ms: f64,
    max_delay_ms: f64,
}

impl RetryPolicy {
    pub(super) fn load(agent_dir: &Path) -> Result<Self> {
        for filename in MAIN_CONFIG_FILENAMES {
            let path = agent_dir.join(filename);
            if let Some((document, _)) = read_document(&path)? {
                return Self::from_document(Some(path), &document);
            }
        }
        Self::from_document(Some(agent_dir.join(MAIN_CONFIG_FILENAMES[0])), &empty_mapping())
    }

    fn from_document(path: Option<PathBuf>, document: &Yaml) -> Result<Self> {
        let root = document.as_hash().context("native settings root must be a mapping")?;
        let group = match root.get(&Yaml::String("retry".into())) {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("retry settings must be a mapping")?),
        };
        let enabled = match group.and_then(|group| group.get(&Yaml::String("enabled".into()))) {
            None => true,
            Some(Yaml::Boolean(enabled)) => *enabled,
            Some(_) => bail!("retry.enabled must be a boolean"),
        };
        Ok(Self {
            path,
            enabled,
            max_retries: read_retry_number(group, "maxRetries", 10.0)?,
            base_delay_ms: read_retry_number(group, "baseDelayMs", 500.0)?,
            max_delay_ms: read_retry_number(group, "maxDelayMs", 300_000.0)?,
        })
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn max_retries(&self) -> f64 {
        self.max_retries
    }

    pub(super) fn base_delay_ms(&self) -> f64 {
        self.base_delay_ms
    }

    pub(super) fn max_delay_ms(&self) -> f64 {
        self.max_delay_ms
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        let Some(path) = &self.path else {
            self.enabled = enabled;
            return Ok(());
        };
        // Reload the whole native document before changing the one key, so
        // deferred fixed retry settings and disjoint external edits survive.
        let loaded = read_document(path)?;
        let mut document = loaded.as_ref().map(|(document, _)| document.clone()).unwrap_or_else(empty_mapping);
        Self::from_document(Some(path.clone()), &document)?;
        let root = document.as_mut_hash().expect("read_document validates mapping");
        let group = root.entry(Yaml::String("retry".into())).or_insert_with(empty_mapping);
        if matches!(group, Yaml::Null) {
            *group = empty_mapping();
        }
        group
            .as_mut_hash()
            .context("retry settings must be a mapping")?
            .insert(Yaml::String("enabled".into()), Yaml::Boolean(enabled));
        let updated = Self::from_document(Some(path.clone()), &document)?;
        let mut encoded = String::new();
        YamlEmitter::new(&mut encoded).dump(&document).context("serializing native retry settings")?;
        encoded.push('\n');
        write_atomically(path, &encoded, loaded.as_ref().map(|(_, source)| source.as_str()), "retry")?;
        // Publish enabled and any reloaded numeric values together only after
        // the atomic write succeeds. Failure leaves the live snapshot intact.
        *self = updated;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn isolated(enabled: bool) -> Self {
        Self { path: None, enabled, max_retries: 10.0, base_delay_ms: 500.0, max_delay_ms: 300_000.0 }
    }
}

fn read_retry_number(group: Option<&Hash>, key: &str, default: f64) -> Result<f64> {
    let value = match group.and_then(|group| group.get(&Yaml::String(key.into()))) {
        None => return Ok(default),
        Some(Yaml::Integer(value)) => *value as f64,
        Some(Yaml::Real(value)) => {
            value.parse::<f64>().with_context(|| format!("retry.{key} must be a finite number"))?
        }
        Some(_) => bail!("retry.{key} must be a number"),
    };
    if !value.is_finite() {
        bail!("retry.{key} must be a finite number");
    }
    Ok(value)
}

fn empty_mapping() -> Yaml {
    Yaml::Hash(Hash::new())
}

fn read_document(path: &Path) -> Result<Option<(Yaml, String)>> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading native settings {}", path.display())),
    };
    let mut documents =
        YamlLoader::load_from_str(&source).with_context(|| format!("parsing native settings {}", path.display()))?;
    if documents.len() > 1 {
        bail!("native settings must contain one YAML document: {}", path.display());
    }
    let document = match documents.pop() {
        None | Some(Yaml::Null) => empty_mapping(),
        Some(document @ Yaml::Hash(_)) => document,
        Some(_) => bail!("native settings must contain a mapping at the document root: {}", path.display()),
    };
    Ok(Some((document, source)))
}

fn read_enabled(document: &Yaml) -> Result<bool> {
    let root = document.as_hash().context("native settings root must be a mapping")?;
    let Some(group) = root.get(&Yaml::String("compaction".into())).filter(|group| !matches!(group, Yaml::Null)) else {
        return Ok(true);
    };
    let group = group.as_hash().context("compaction settings must be a mapping")?;
    let configured = match group.get(&Yaml::String("enabled".into())) {
        None | Some(Yaml::Null) => true,
        Some(Yaml::Boolean(enabled)) => *enabled,
        Some(_) => bail!("compaction.enabled must be a boolean"),
    };
    Ok(configured && has_compaction_methods(group.get(&Yaml::String("methodOrder".into()))))
}

fn has_compaction_methods(order: Option<&Yaml>) -> bool {
    match order {
        None => true,
        Some(Yaml::Array(methods)) => {
            methods.iter().any(|method| method.as_str().is_some_and(|method| DEFAULT_METHOD_ORDER.contains(&method)))
        }
        Some(_) => false,
    }
}

fn write_atomically(path: &Path, source: &str, expected: Option<&str>, group: &str) -> Result<()> {
    let parent = path.parent().context("native settings path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating native settings directory {}", parent.display()))?;
    // Follow a selected config symlink instead of replacing its link. Full
    // upstream link-chain locking/quarantine is beyond this one-key writer.
    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error).context("resolving native settings write target"),
    };
    let target_parent = target.parent().context("native settings target has no parent")?;
    let temporary = target_parent.join(format!(".ara-{group}-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .context("creating temporary native settings file")?;
        if let Ok(metadata) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temporary, metadata.permissions())
                .context("preserving native settings permissions")?;
        }
        output.write_all(source.as_bytes()).with_context(|| format!("writing native {group} settings"))?;
        output.sync_all().with_context(|| format!("syncing native {group} settings"))?;
        drop(output);
        let current = read_document(path)?.map(|(_, source)| source);
        if current.as_deref() != expected {
            bail!("native settings changed during {group} setting update; retry the command");
        }
        std::fs::rename(&temporary, &target).with_context(|| format!("replacing native {group} settings"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_policy_defaults_reloads_and_scoped_writes_preserve_native_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        let mut policy = AutoCompactionPolicy::load(directory.path()).unwrap();
        let defaults = policy.recovery_settings().unwrap();
        assert!(defaults.remote.enabled && defaults.remote.streaming_v2_enabled);
        assert!(defaults.remote.endpoint.is_none());
        std::fs::write(&path, "compaction:\n  remoteEnabled: false\n  remoteStreamingV2Enabled: false\n  remoteEndpoint: http://fixture.invalid/compact\ncustom: preserved\n").unwrap();
        let configured = policy.recovery_settings().unwrap();
        assert!(!configured.remote.enabled && !configured.remote.streaming_v2_enabled);
        assert_eq!(configured.remote.endpoint.as_deref(), Some("http://fixture.invalid/compact"));
        policy.set_enabled(false).unwrap();
        let reloaded = policy.recovery_settings().unwrap();
        assert!(!reloaded.remote.enabled && !reloaded.remote.streaming_v2_enabled);
        assert_eq!(reloaded.remote.endpoint, configured.remote.endpoint);
        assert_eq!(read_document(&path).unwrap().unwrap().0["custom"].as_str(), Some("preserved"));
        for source in [
            "compaction:\n  remoteEnabled: 'false'\n",
            "compaction:\n  remoteStreamingV2Enabled: 1\n",
            "compaction:\n  remoteEndpoint: true\n",
            "compaction: scalar\n",
        ] {
            std::fs::write(&path, source).unwrap();
            assert!(policy.recovery_settings().is_err(), "{source}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        }
    }

    #[test]
    fn default_true_is_lazy_and_first_write_uses_yml() {
        let directory = tempfile::tempdir().unwrap();
        let agent = directory.path().join("agent");
        let mut policy = AutoCompactionPolicy::load(&agent).unwrap();
        assert!(policy.enabled());
        assert!(!agent.exists());
        policy.set_enabled(false).unwrap();
        assert!(!policy.enabled());
        assert!(agent.join("config.yml").is_file());
        assert!(!AutoCompactionPolicy::load(&agent).unwrap().enabled());
    }

    #[test]
    fn yml_precedes_yaml_and_yaml_is_the_write_target_when_primary_missing() {
        let directory = tempfile::tempdir().unwrap();
        let yaml = directory.path().join("config.yaml");
        let yml = directory.path().join("config.yml");
        std::fs::write(&yaml, "compaction:\n  enabled: false\n").unwrap();
        let mut policy = AutoCompactionPolicy::load(directory.path()).unwrap();
        assert!(!policy.enabled());
        policy.set_enabled(true).unwrap();
        assert!(!yml.exists());
        assert!(AutoCompactionPolicy::load(directory.path()).unwrap().enabled());
        std::fs::write(&yml, "compaction:\n  enabled: false\n").unwrap();
        assert!(!AutoCompactionPolicy::load(directory.path()).unwrap().enabled());
        assert!(read_enabled(&read_document(&yaml).unwrap().unwrap().0).unwrap());
    }

    #[test]
    fn scoped_write_reloads_and_preserves_unknown_values_and_repairs_empty_order() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(
            &path,
            "compaction:\n  enabled: false\n  keepRecentTokens: 123\n  methodOrder: []\ncustom:\n  old: one\n",
        )
        .unwrap();
        let mut policy = AutoCompactionPolicy::load(directory.path()).unwrap();
        assert!(!policy.enabled());
        std::fs::write(&path, "compaction:\n  enabled: false\n  keepRecentTokens: 321\n  methodOrder: []\ncustom:\n  external: [one, 2, true]\n").unwrap();
        policy.set_enabled(true).unwrap();
        let document = read_document(&path).unwrap().unwrap().0;
        assert_eq!(document["compaction"]["keepRecentTokens"].as_i64(), Some(321));
        assert_eq!(
            document["custom"]["external"],
            Yaml::Array(vec![Yaml::String("one".into()), Yaml::Integer(2), Yaml::Boolean(true)])
        );
        assert_eq!(
            document["compaction"]["methodOrder"],
            Yaml::Array(DEFAULT_METHOD_ORDER.iter().map(|method| Yaml::String((*method).into())).collect())
        );
    }

    #[test]
    fn enabled_with_empty_or_unknown_order_is_false_until_repaired() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        for order in ["[]", "[unsupported]", "[unsupported, soft]"] {
            std::fs::write(&path, format!("compaction:\n  enabled: true\n  methodOrder: {order}\n")).unwrap();
            let mut policy = AutoCompactionPolicy::load(directory.path()).unwrap();
            assert_eq!(policy.enabled(), order == "[unsupported, soft]");
            policy.set_enabled(true).unwrap();
            assert!(policy.enabled());
        }
    }

    #[test]
    fn malformed_wrong_shape_and_unreadable_primary_do_not_fallback_or_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(directory.path().join("config.yaml"), "compaction:\n  enabled: true\n").unwrap();
        for source in ["bad: [", "- sequence", "compaction: scalar", "compaction:\n  enabled: 'false'\n"] {
            std::fs::write(&path, source).unwrap();
            assert!(AutoCompactionPolicy::load(directory.path()).is_err(), "{source}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(AutoCompactionPolicy::load(directory.path()).is_err());
    }

    #[test]
    fn malformed_reload_and_io_failure_do_not_update_live_policy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        let mut policy = AutoCompactionPolicy::load(directory.path()).unwrap();
        std::fs::write(&path, "bad: [").unwrap();
        assert!(policy.set_enabled(false).is_err());
        assert!(policy.enabled());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bad: [");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(policy.set_enabled(false).is_err());
        assert!(policy.enabled());
    }

    #[test]
    fn retry_defaults_are_lazy_and_saved_toggle_survives_reload() {
        let directory = tempfile::tempdir().unwrap();
        let agent = directory.path().join("agent");
        let mut policy = RetryPolicy::load(&agent).unwrap();
        assert!(policy.enabled());
        assert_eq!(policy.max_retries(), 10.0);
        assert_eq!(policy.base_delay_ms(), 500.0);
        assert_eq!(policy.max_delay_ms(), 300_000.0);
        assert!(!agent.exists());
        policy.set_enabled(false).unwrap();
        assert!(!policy.enabled());
        assert!(agent.join("config.yml").is_file());
        let reopened = RetryPolicy::load(&agent).unwrap();
        assert!(!reopened.enabled());
        assert_eq!(reopened.max_retries(), 10.0);
        let mut isolated = RetryPolicy::isolated(false);
        isolated.set_enabled(true).unwrap();
        assert!(isolated.enabled());
        assert_eq!(isolated.base_delay_ms(), 500.0);
    }

    #[test]
    fn retry_first_existing_filename_wins_and_yaml_remains_write_target() {
        let directory = tempfile::tempdir().unwrap();
        let yaml = directory.path().join("config.yaml");
        let yml = directory.path().join("config.yml");
        std::fs::write(&yaml, "retry:\n  enabled: false\n  maxRetries: 2\n").unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        assert!(!policy.enabled());
        assert_eq!(policy.max_retries(), 2.0);
        policy.set_enabled(true).unwrap();
        assert!(!yml.exists());
        assert!(RetryPolicy::load(directory.path()).unwrap().enabled());
        std::fs::write(&yml, "retry:\n  enabled: false\n  maxRetries: 3\n").unwrap();
        let preferred = RetryPolicy::load(directory.path()).unwrap();
        assert!(!preferred.enabled());
        assert_eq!(preferred.max_retries(), 3.0);
        assert_eq!(read_document(&yaml).unwrap().unwrap().0["retry"]["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn retry_numbers_preserve_fixed_fraction_negative_and_zero_semantics() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "retry:\n  maxRetries: 1.5\n  baseDelayMs: -2.75\n  maxDelayMs: 0\n").unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        assert_eq!(policy.max_retries(), 1.5);
        assert_eq!(policy.base_delay_ms(), -2.75);
        assert_eq!(policy.max_delay_ms(), 0.0);
        policy.set_enabled(false).unwrap();
        let reopened = RetryPolicy::load(directory.path()).unwrap();
        assert_eq!(reopened.max_retries(), 1.5);
        assert_eq!(reopened.base_delay_ms(), -2.75);
        assert_eq!(reopened.max_delay_ms(), 0.0);
    }

    #[test]
    fn retry_scoped_write_retains_fallback_custom_and_compaction_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "retry:\n  enabled: true\n  maxRetries: 1\ncustom: old\n").unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        // All fixed fallback keys are retained as source values until their
        // mandatory host credential/model interfaces consume them.
        std::fs::write(&path, "retry:\n  enabled: true\n  maxRetries: 7\n  baseDelayMs: 12.5\n  maxDelayMs: 0\n  modelFallback: true\n  usageAwareFallback: false\n  usageReservePct: 10\n  usageReservePolicy: confirm\n  fallbackChains:\n    default: [openai/fallback]\n    provider/*: [other/*]\n  fallbackRevertPolicy: cooldown-expiry\n  custom: [one, 2, true]\ncompaction:\n  enabled: false\n  methodOrder: []\ncustom:\n  external: changed\n").unwrap();
        let before = read_document(&path).unwrap().unwrap().0;
        policy.set_enabled(false).unwrap();
        let after = read_document(&path).unwrap().unwrap().0;
        let mut expected = before;
        expected
            .as_mut_hash()
            .unwrap()
            .get_mut(&Yaml::String("retry".into()))
            .unwrap()
            .as_mut_hash()
            .unwrap()
            .insert(Yaml::String("enabled".into()), Yaml::Boolean(false));
        // The native stringifier can reposition the updated root group.
        // Compare every retained value, including unknown nested values.
        let actual = after.as_hash().unwrap();
        let expected = expected.as_hash().unwrap();
        assert_eq!(actual.len(), expected.len());
        for (key, value) in expected {
            assert_eq!(actual.get(key), Some(value), "retained setting {key:?}");
        }
        assert!(!policy.enabled());
        assert_eq!(policy.max_retries(), 7.0);
        assert_eq!(policy.base_delay_ms(), 12.5);
        assert_eq!(policy.max_delay_ms(), 0.0);
    }

    #[test]
    fn retry_invalid_primary_never_falls_back_or_overwrites_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(directory.path().join("config.yaml"), "retry:\n  enabled: true\n").unwrap();
        for source in
            ["bad: [", "- sequence", "retry: scalar", "retry:\n  enabled: 'false'\n", "retry:\n  enabled: null\n"]
        {
            std::fs::write(&path, source).unwrap();
            assert!(RetryPolicy::load(directory.path()).is_err(), "{source}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        }
        for key in ["maxRetries", "baseDelayMs", "maxDelayMs"] {
            for value in ["'2'", "true", "null", ".nan", ".inf", "-.inf"] {
                let source = format!("retry:\n  {key}: {value}\n");
                std::fs::write(&path, &source).unwrap();
                assert!(RetryPolicy::load(directory.path()).is_err(), "{source}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
            }
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(RetryPolicy::load(directory.path()).is_err());
    }

    #[test]
    fn retry_failed_reload_preserves_live_enabled_and_numeric_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "retry:\n  enabled: true\n  maxRetries: 2.5\n  baseDelayMs: 25\n  maxDelayMs: 100\n")
            .unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        for source in ["bad: [", "retry:\n  maxRetries: 'bad'\n"] {
            std::fs::write(&path, source).unwrap();
            assert!(policy.set_enabled(false).is_err());
            assert!(policy.enabled());
            assert_eq!(policy.max_retries(), 2.5);
            assert_eq!(policy.base_delay_ms(), 25.0);
            assert_eq!(policy.max_delay_ms(), 100.0);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(policy.set_enabled(false).is_err());
        assert!(policy.enabled());
        assert_eq!(policy.max_retries(), 2.5);
    }

    #[test]
    fn retry_atomic_writer_rejects_changed_generation_and_cleans_own_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        let old = "retry:\n  enabled: true\n";
        let external = "retry:\n  enabled: true\ncustom: external\n";
        std::fs::write(&path, external).unwrap();
        let error = write_atomically(&path, "retry:\n  enabled: false\n", Some(old), "retry").unwrap_err();
        assert!(error.to_string().contains("changed during retry setting update"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
