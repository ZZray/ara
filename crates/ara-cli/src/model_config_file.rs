//! Host-owned `models.yml` handle, from OMP `config-file.ts` at 596f2da.
//! MIT: Copyright (c) 2025 Mario Zechner, 2025-2026 Can Bölük,
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//!
//! Paths are supplied by the host. Loading never resolves credentials or
//! performs discovery. Results (including failures and absence) are cached
//! until explicit invalidation, as in the fixed upstream handle.

use crate::models_config::{ConfigValidationStage, ModelsConfig};
use serde_json::Value;
use std::{
    collections::HashSet,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::UNIX_EPOCH,
};

static MIGRATED_PATHS: LazyLock<Mutex<HashSet<(PathBuf, PathBuf)>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigErrorStage {
    Read,
    Schema,
    ValidateModels,
    Unexpected,
    CreateDefault,
}

impl fmt::Display for ConfigErrorStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "Read",
            Self::Schema => "Schema",
            Self::ValidateModels => "Validate(models)",
            Self::Unexpected => "Unexpected",
            Self::CreateDefault => "createDefault",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigFileError {
    pub stage: ConfigErrorStage,
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ConfigFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Failed to load config file models, {} error: {}", self.stage, self.message)
    }
}

impl Error for ConfigFileError {}

#[derive(Clone, Debug)]
pub enum ModelConfigLoad {
    Ok(ModelsConfig),
    Error(ConfigFileError),
    NotFound,
}

impl ModelConfigLoad {
    pub fn value(&self) -> Option<&ModelsConfig> {
        if let Self::Ok(value) = self { Some(value) } else { None }
    }
}

/// Rust specialization of the fixed generic `ConfigFile<ModelsConfig>`.
/// No process-global agent directory or credentials are consulted.
#[derive(Clone, Debug)]
pub struct ModelsConfigFile {
    base_path: PathBuf,
    yaml_fallback: Option<PathBuf>,
    json_migration: Option<PathBuf>,
    cache: Arc<Mutex<Option<ModelConfigLoad>>>,
    warnings: Arc<Mutex<Vec<String>>>,
}

impl ModelsConfigFile {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ConfigFileError> {
        let base_path = path.into();
        let (yaml_fallback, json_migration) = match base_path.extension().and_then(|x| x.to_str()) {
            Some("yml") => (Some(base_path.with_extension("yaml")), Some(base_path.with_extension("json"))),
            Some("yaml") => (None, Some(base_path.with_extension("json"))),
            Some("json" | "jsonc") => (None, None),
            _ => {
                return Err(ConfigFileError {
                    stage: ConfigErrorStage::Unexpected,
                    message: format!("Invalid config file path: {}", base_path.display()),
                    path: base_path,
                });
            }
        };
        Ok(Self {
            base_path,
            yaml_fallback,
            json_migration,
            cache: Arc::new(Mutex::new(None)),
            warnings: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn path(&self) -> &Path {
        if self.base_path.exists() {
            &self.base_path
        } else {
            self.yaml_fallback.as_deref().filter(|p| p.exists()).unwrap_or(&self.base_path)
        }
    }

    /// Relocation retains configuration semantics and drops a previous path's
    /// load cache. Upstream runs migration on a newly relocated handle.
    pub fn relocate(&self, path: impl Into<PathBuf>) -> Result<Self, ConfigFileError> {
        let path = path.into();
        if path == self.base_path {
            return Ok(self.clone());
        }
        let mut relocated = Self::new(path)?;
        relocated.ensure_migrated();
        Ok(relocated)
    }

    pub fn invalidate(&mut self) {
        *self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.warnings.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    pub fn get_mtime_ms(&self) -> Result<Option<f64>, ConfigFileError> {
        match std::fs::metadata(self.path()) {
            Ok(metadata) => metadata
                .modified()
                .map(|time| Some(epoch_ms(time)))
                .map_err(|error| self.error(ConfigErrorStage::Read, error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(self.error(ConfigErrorStage::Read, error.to_string())),
        }
    }

    pub async fn get_mtime_ms_async(&self) -> Result<Option<f64>, ConfigFileError> {
        match tokio::fs::metadata(self.path()).await {
            Ok(metadata) => metadata
                .modified()
                .map(|time| Some(epoch_ms(time)))
                .map_err(|error| self.error(ConfigErrorStage::Read, error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(self.error(ConfigErrorStage::Read, error.to_string())),
        }
    }

    pub fn create_default(&self) -> Result<ModelsConfig, ConfigFileError> {
        ModelsConfig::validate_schema(serde_json::json!({}))
            .map_err(|error| self.error(ConfigErrorStage::CreateDefault, error.to_string()))
    }

    pub fn try_load(&mut self) -> ModelConfigLoad {
        if let Some(cached) = self.cached() {
            return cached;
        }
        self.ensure_migrated();
        let result = self.read_result(std::fs::read(self.path()));
        self.store_cache(result.clone());
        result
    }

    pub async fn try_load_async(&mut self) -> ModelConfigLoad {
        if let Some(cached) = self.cached() {
            return cached;
        }
        // The upstream async method also performs its migration synchronously.
        self.ensure_migrated();
        let bytes = tokio::fs::read(self.path()).await;
        let result = self.read_result(bytes);
        self.store_cache(result.clone());
        result
    }

    pub fn load(&mut self) -> Option<ModelsConfig> {
        match self.try_load() {
            ModelConfigLoad::Ok(value) => Some(value),
            _ => None,
        }
    }

    pub async fn load_async(&mut self) -> Option<ModelsConfig> {
        match self.try_load_async().await {
            ModelConfigLoad::Ok(value) => Some(value),
            _ => None,
        }
    }

    pub fn load_or_default(&mut self) -> Result<ModelsConfig, ConfigFileError> {
        self.load().map(Ok).unwrap_or_else(|| self.create_default())
    }

    pub async fn load_or_default_async(&mut self) -> Result<ModelsConfig, ConfigFileError> {
        self.load_async().await.map(Ok).unwrap_or_else(|| self.create_default())
    }

    fn error(&self, stage: ConfigErrorStage, message: String) -> ConfigFileError {
        ConfigFileError { stage, path: self.path().to_owned(), message }
    }

    fn cached(&self) -> Option<ModelConfigLoad> {
        self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn store_cache(&self, value: ModelConfigLoad) {
        *self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
    }

    fn warn(&self, message: String) {
        self.warnings.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(message);
    }

    fn read_result(&mut self, result: std::io::Result<Vec<u8>>) -> ModelConfigLoad {
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ModelConfigLoad::NotFound,
            Err(error) => {
                let error = self.error(ConfigErrorStage::Read, error.to_string());
                self.warn(error.to_string());
                return ModelConfigLoad::Error(error);
            }
        };
        // Node/Bun's UTF-8 decoder replaces invalid sequences rather than
        // treating a whole file as an unreadable filesystem result.
        let content = String::from_utf8_lossy(&bytes);
        let content = ara_prompt::js::trim(&content);
        let parsed = match self.path().extension().and_then(|x| x.to_str()) {
            Some("json" | "jsonc") => parse_jsonc(content),
            _ => parse_config_yaml(content),
        };
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(message) => {
                let error = self.error(ConfigErrorStage::Unexpected, message);
                self.warn(error.to_string());
                return ModelConfigLoad::Error(error);
            }
        };
        match ModelsConfig::validate(parsed) {
            Ok(value) => ModelConfigLoad::Ok(value),
            Err(error) => {
                let stage = match error.stage {
                    ConfigValidationStage::Schema => ConfigErrorStage::Schema,
                    ConfigValidationStage::Provider => ConfigErrorStage::ValidateModels,
                };
                let error = self.error(stage, error.to_string());
                if error.stage == ConfigErrorStage::Schema {
                    self.warn(error.to_string());
                }
                ModelConfigLoad::Error(error)
            }
        }
    }

    fn ensure_migrated(&mut self) {
        let Some(json_path) = &self.json_migration else { return };
        if self.yaml_fallback.as_ref().is_some_and(|p| !self.base_path.exists() && p.exists()) {
            return;
        }
        let key = (json_path.clone(), self.base_path.clone());
        let mut migrated = MIGRATED_PATHS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if migrated.contains(&key) {
            return;
        }
        if self.base_path.exists() || !json_path.exists() {
            migrated.insert(key);
            return;
        }
        let migration = (|| -> Result<bool, String> {
            let bytes = std::fs::read(json_path).map_err(|error| error.to_string())?;
            let content = String::from_utf8_lossy(&bytes);
            let parsed = parse_jsonc(ara_prompt::js::trim(&content))?;
            if !ara_prompt::js::truthy(&parsed) {
                return Ok(false);
            }
            let yaml = stringify_yaml_config(&parsed)?;
            std::fs::write(&self.base_path, yaml).map_err(|error| error.to_string())?;
            Ok(true)
        })();
        match migration {
            Ok(written) => {
                if !written {
                    self.warn(format!("migrateJsonToYml: invalid json structure: {}", json_path.display()));
                }
                migrated.insert(key);
            }
            Err(error) => self.warn(format!("migrateJsonToYml: migration failed: {error}")),
        }
    }
}

fn epoch_ms(time: std::time::SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64() * 1000.0,
        Err(error) => -error.duration().as_secs_f64() * 1000.0,
    }
}

/// JSONC comments and trailing commas are removed outside quoted strings.
/// Byte positions/newlines remain intact for parser diagnostics.
pub fn parse_jsonc(source: &str) -> Result<Value, String> {
    let mut bytes = source.as_bytes().to_vec();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        if in_string {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => {
                    in_string = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        match bytes[i] {
            b'"' => {
                in_string = true;
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                    bytes[i] = b' ';
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                bytes[i] = b' ';
                bytes[i + 1] = b' ';
                i += 2;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        bytes[i] = b' ';
                        bytes[i + 1] = b' ';
                        i += 2;
                        closed = true;
                        break;
                    }
                    if !matches!(bytes[i], b'\r' | b'\n') {
                        bytes[i] = b' ';
                    }
                    i += 1;
                }
                if !closed {
                    return Err("unterminated JSONC block comment".into());
                }
            }
            _ => i += 1,
        }
    }
    i = 0;
    in_string = false;
    while i < bytes.len() {
        if in_string {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => {
                    in_string = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if bytes[i] == b'"' {
            in_string = true;
        } else if bytes[i] == b',' {
            let next = bytes[i + 1..].iter().find(|byte| !byte.is_ascii_whitespace());
            let previous = bytes[..i].iter().rev().find(|byte| !byte.is_ascii_whitespace());
            if matches!(next, Some(b'}' | b']'))
                && matches!(previous, Some(b'"' | b'}' | b']' | b'0'..=b'9' | b'e' | b'l'))
            {
                bytes[i] = b' ';
            }
        }
        i += 1;
    }
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(js_numbers(parsed))
}

fn js_numbers(value: Value) -> Value {
    match value {
        Value::Number(number) => ara_prompt::js::number(number.as_f64().expect("finite JSON number")),
        Value::Array(values) => Value::Array(values.into_iter().map(js_numbers).collect()),
        Value::Object(values) => Value::Object(values.into_iter().map(|(k, v)| (k, js_numbers(v))).collect()),
        value => value,
    }
}

fn parse_config_yaml(source: &str) -> Result<Value, String> {
    // The shared parser's non-finite sentinels preserve truthiness for
    // frontmatter. They cannot be allowed to pass numeric config validation.
    // Non-finite YAML values stay an explicit loader limitation until a native
    // extended number representation is available; never silently saturate.
    use yaml_rust2::{
        parser::{Event, Parser},
        scanner::TScalarStyle,
    };
    let mut parser = Parser::new_from_str(source);
    loop {
        let (event, _) = parser.next_token().map_err(|error| error.to_string())?;
        match event {
            Event::StreamEnd => break,
            Event::Scalar(value, TScalarStyle::Plain, _, tag) => {
                let string_tag = tag.as_ref().is_some_and(|tag| {
                    !matches!(tag.handle.as_str(), "tag:yaml.org,2002:" | "!!") || tag.suffix == "str"
                });
                if !string_tag
                    && (matches!(value.to_ascii_lowercase().as_str(), ".inf" | "+.inf" | "-.inf" | ".nan")
                        || decimal_overflows(&value)
                        || radix_overflows(&value))
                {
                    return Err("non-finite YAML numbers cannot be represented in native models config".into());
                }
            }
            _ => {}
        }
    }
    ara_discovery::frontmatter::parse_yaml(source)
}

fn radix_overflows(value: &str) -> bool {
    let (digits, radix) = if let Some(digits) = value.strip_prefix("0x") {
        (digits, 16)
    } else if let Some(digits) = value.strip_prefix("0o") {
        (digits, 8)
    } else {
        return false;
    };
    !digits.is_empty()
        && digits.chars().all(|digit| digit.is_ascii() && digit.is_digit(radix))
        && !digits
            .chars()
            .fold(0.0, |number, digit| number * f64::from(radix) + f64::from(digit.to_digit(radix).unwrap()))
            .is_finite()
}

fn decimal_overflows(value: &str) -> bool {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let mut parts = unsigned.split(['e', 'E']);
    let mantissa = parts.next().unwrap_or("");
    let exponent = parts.next();
    if parts.next().is_some() {
        return false;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if exponent.is_some_and(|e| !digits(e.strip_prefix(['+', '-']).unwrap_or(e))) {
        return false;
    }
    let numeric = if let Some((whole, fraction)) = mantissa.split_once('.') {
        (digits(whole) || whole.is_empty())
            && (digits(fraction) || fraction.is_empty())
            && (!whole.is_empty() || !fraction.is_empty())
    } else {
        digits(mantissa)
    };
    numeric && value.parse::<f64>().is_ok_and(|number| !number.is_finite())
}

pub fn stringify_yaml_config(value: &Value) -> Result<String, String> {
    fn block(value: &Value) -> bool {
        match value {
            Value::Object(map) => !map.is_empty(),
            Value::Array(array) => !array.is_empty(),
            _ => false,
        }
    }
    fn write(value: &Value, indent: usize, output: &mut String) -> Result<(), String> {
        let padding = " ".repeat(indent);
        match value {
            Value::Object(map) if !map.is_empty() => {
                for (key, value) in ara_prompt::js::entries(map) {
                    output.push_str(&padding);
                    output.push_str(&serde_json::to_string(key).map_err(|error| error.to_string())?);
                    output.push(':');
                    if block(value) {
                        output.push('\n');
                        write(value, indent + 2, output)?;
                    } else {
                        output.push(' ');
                        output.push_str(&serde_json::to_string(value).map_err(|error| error.to_string())?);
                        output.push('\n');
                    }
                }
            }
            Value::Array(values) if !values.is_empty() => {
                for value in values {
                    output.push_str(&padding);
                    output.push('-');
                    if block(value) {
                        output.push('\n');
                        write(value, indent + 2, output)?;
                    } else {
                        output.push(' ');
                        output.push_str(&serde_json::to_string(value).map_err(|error| error.to_string())?);
                        output.push('\n');
                    }
                }
            }
            value => {
                output.push_str(&padding);
                output.push_str(&serde_json::to_string(value).map_err(|error| error.to_string())?);
                output.push('\n');
            }
        }
        Ok(())
    }
    // Always quote strings/keys: the emitter's YAML 1.1 quote heuristic loses
    // YAML 1.2 octal-looking strings. Native spelling remains a documented
    // formatting adaptation; all JSON values round-trip through the parser.
    let mut output = String::new();
    write(value, 0, &mut output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonc_comments_trailing_commas_strings_and_js_numbers() {
        let parsed = parse_jsonc(
            r#"{/* /**/ "keep": "https://x/*literal*/\\\"//", "n":9007199254740993, // hi
            "array": [1,2,],}"#,
        )
        .unwrap();
        assert_eq!(parsed["array"], json!([1, 2]));
        assert_eq!(parsed["n"].as_f64(), Some(9007199254740992.0));
        assert_eq!(parsed["keep"], json!("https://x/*literal*/\\\"//"));
        for invalid in ["/*", "{,}", "[,]", "{\"providers\":{,}}", "{\"x\":,}", "[1,,]", "{\"x\":1,/*", "{\"x\":'no'}"]
        {
            assert!(parse_jsonc(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn real_file_precedence_missing_error_and_success_cache() {
        let dir = tempfile::tempdir().unwrap();
        let yml = dir.path().join("models.yml");
        let fallback = dir.path().join("models.yaml");
        let mut file = ModelsConfigFile::new(&yml).unwrap();
        assert!(matches!(file.try_load(), ModelConfigLoad::NotFound));
        std::fs::write(&fallback, "providers: {}\nextra: fallback").unwrap();
        assert_eq!(file.path(), fallback);
        assert!(matches!(file.try_load(), ModelConfigLoad::NotFound));
        file.invalidate();
        assert_eq!(file.load().unwrap().value()["extra"], "fallback");
        std::fs::write(&yml, "providers: {}\nextra: primary").unwrap();
        assert_eq!(file.path(), yml);
        assert_eq!(file.load().unwrap().value()["extra"], "fallback");
        file.invalidate();
        assert_eq!(file.load().unwrap().value()["extra"], "primary");
        std::fs::write(&yml, "providers: {p: {models: [{id: x}]}}").unwrap();
        file.invalidate();
        assert!(matches!(
            file.try_load(),
            ModelConfigLoad::Error(ConfigFileError { stage: ConfigErrorStage::ValidateModels, .. })
        ));
        std::fs::write(&yml, "{}").unwrap();
        assert!(matches!(file.try_load(), ModelConfigLoad::Error(_)));
        assert_eq!(file.load_or_default().unwrap().value(), &json!({}));
        file.invalidate();
        assert!(file.load().is_some());
        let mut alias = file.relocate(&yml).unwrap();
        std::fs::write(&yml, "extra: alias").unwrap();
        alias.invalidate();
        assert_eq!(alias.load().unwrap().value()["extra"], "alias");
        assert_eq!(file.load().unwrap().value()["extra"], "alias");
    }

    #[test]
    fn migration_keeps_json_and_native_data_including_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("models.json");
        let yml = dir.path().join("models.yml");
        let original =
            "{/* comment */\"providers\":{\"local\":{\"auth\":\"none\"}},\"unknown\":{\"a\":[1,true,null,\"x\"],},}";
        std::fs::write(&json, original).unwrap();
        let mut file = ModelsConfigFile::new(&yml).unwrap();
        let loaded = file.load().unwrap();
        assert_eq!(loaded.value(), &parse_jsonc(original).unwrap());
        assert_eq!(std::fs::read_to_string(&json).unwrap(), original);
        assert!(yml.is_file());
        assert!(file.get_mtime_ms().unwrap().is_some());
        assert!(file.take_warnings().is_empty());
        // Already-migrated pairs are not replayed after destination deletion.
        std::fs::remove_file(&yml).unwrap();
        file.invalidate();
        assert!(matches!(file.try_load(), ModelConfigLoad::NotFound));
        let tricky = json!({"providers":{"p":{"apiKey":"0o123"}},"0o17":[".inf", "Infinity", "0xFF", "True", "Null", "1e3", "", "汉字\n行", {"null": "~"}]});
        assert_eq!(parse_config_yaml(&stringify_yaml_config(&tricky).unwrap()).unwrap(), tricky);
    }

    #[test]
    fn yaml_fallback_prevents_json_migration_and_direct_jsonc_does_not_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let yml = dir.path().join("models.yml");
        std::fs::write(dir.path().join("models.yaml"), "extra: yaml").unwrap();
        std::fs::write(dir.path().join("models.json"), "{\"extra\":\"json\"}").unwrap();
        let mut file = ModelsConfigFile::new(&yml).unwrap();
        assert_eq!(file.load().unwrap().value()["extra"], "yaml");
        assert!(!yml.exists());
        let direct = dir.path().join("explicit.jsonc");
        std::fs::write(&direct, "{/* hi */}").unwrap();
        assert!(ModelsConfigFile::new(&direct).unwrap().load().is_some());
        assert!(!direct.with_extension("yml").exists());
    }

    #[test]
    fn migration_missing_and_falsy_cache_but_parse_and_write_failures_retry() {
        let dir = tempfile::tempdir().unwrap();
        for (name, first, should_retry) in
            [("missing", None, false), ("falsy", Some("null"), false), ("invalid", Some("{"), true)]
        {
            let yml = dir.path().join(format!("{name}.yml"));
            let json = yml.with_extension("json");
            if let Some(first) = first {
                std::fs::write(&json, first).unwrap();
            }
            let mut file = ModelsConfigFile::new(&yml).unwrap();
            assert!(matches!(file.try_load(), ModelConfigLoad::NotFound));
            std::fs::write(&json, "{}").unwrap();
            file.invalidate();
            assert_eq!(file.load().is_some(), should_retry);
            assert_eq!(yml.exists(), should_retry);
        }
        // A write failure does not latch the migration pair.
        let json = dir.path().join("retry.json");
        let target = dir.path().join("missing-parent").join("retry.yml");
        std::fs::write(&json, "{}").unwrap();
        let mut file = ModelsConfigFile::new(&target).unwrap();
        file.json_migration = Some(json);
        assert!(matches!(file.try_load(), ModelConfigLoad::NotFound));
        assert_eq!(file.take_warnings().len(), 1);
        std::fs::create_dir(target.parent().unwrap()).unwrap();
        file.invalidate();
        assert!(file.load().is_some());
    }

    #[tokio::test]
    async fn sync_async_cache_mtime_relocation_and_read_failures() {
        let dir = tempfile::tempdir().unwrap();
        let yml = dir.path().join("models.yml");
        let mut file = ModelsConfigFile::new(&yml).unwrap();
        assert_eq!(file.get_mtime_ms_async().await.unwrap(), None);
        assert_eq!(file.load_or_default_async().await.unwrap().value(), &json!({}));
        std::fs::write(&yml, "extra: changed").unwrap();
        assert!(file.load_async().await.is_none());
        file.invalidate();
        assert_eq!(file.load_async().await.unwrap().value()["extra"], "changed");
        assert_eq!(file.get_mtime_ms_async().await.unwrap(), file.get_mtime_ms().unwrap());
        let relocated = dir.path().join("elsewhere.yaml");
        std::fs::write(relocated.with_extension("json"), "{\"extra\":\"relocated\"}").unwrap();
        let mut moved = file.relocate(&relocated).unwrap();
        assert!(relocated.is_file());
        assert_eq!(moved.load_async().await.unwrap().value()["extra"], "relocated");
        let mut unreadable = ModelsConfigFile::new(dir.path().join("directory.yml")).unwrap();
        std::fs::create_dir(unreadable.path()).unwrap();
        assert!(matches!(
            unreadable.try_load_async().await,
            ModelConfigLoad::Error(ConfigFileError { stage: ConfigErrorStage::Read, .. })
        ));
        assert_eq!(unreadable.take_warnings().len(), 1);
    }

    #[test]
    fn malformed_schema_empty_and_nonfinite_are_explicit_without_saturation() {
        let dir = tempfile::tempdir().unwrap();
        let yml = dir.path().join("models.yml");
        let mut file = ModelsConfigFile::new(&yml).unwrap();
        for (source, stage) in [
            ("", ConfigErrorStage::Schema),
            ("providers: bad", ConfigErrorStage::Schema),
            ("a: [", ConfigErrorStage::Unexpected),
            ("providers: {p: {apiKey: .inf}}", ConfigErrorStage::Unexpected),
        ] {
            std::fs::write(&yml, source).unwrap();
            file.invalidate();
            assert!(matches!(file.try_load(), ModelConfigLoad::Error(error) if error.stage == stage));
        }
        std::fs::write(&yml, "unknown: '.inf'").unwrap();
        file.invalidate();
        assert_eq!(file.load().unwrap().value()["unknown"], ".inf");
        assert!(ModelsConfigFile::new(dir.path().join("models.YML")).is_err());
        for token in [format!("0x{}", "f".repeat(1000)), format!("0o{}", "7".repeat(2000))] {
            assert!(
                parse_config_yaml(&format!("providers: {{p: {{models: [{{id: x, contextWindow: {token}}}]}}}}"))
                    .is_err()
            );
        }
        assert_eq!(
            parse_config_yaml("a: Infinity\nb: NaN\nc: inf").unwrap(),
            json!({"a":"Infinity", "b":"NaN", "c":"inf"})
        );
    }
}
