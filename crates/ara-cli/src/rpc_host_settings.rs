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

pub(super) struct AutoCompactionPolicy {
    path: Option<PathBuf>,
    enabled: bool,
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
        write_atomically(path, &encoded, loaded.as_ref().map(|(_, source)| source.as_str()))?;
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

fn write_atomically(path: &Path, source: &str, expected: Option<&str>) -> Result<()> {
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
    let temporary = target_parent.join(format!(".ara-compaction-{}.tmp", uuid::Uuid::new_v4()));
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
        output.write_all(source.as_bytes()).context("writing native compaction settings")?;
        output.sync_all().context("syncing native compaction settings")?;
        drop(output);
        let current = read_document(path)?.map(|(_, source)| source);
        if current.as_deref() != expected {
            bail!("native settings changed during compaction setting update; retry the command");
        }
        std::fs::rename(&temporary, &target).context("replacing native compaction settings")?;
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
}
