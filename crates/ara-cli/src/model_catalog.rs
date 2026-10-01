//! Read-only bundled Host catalog from fixed OMP 596f2da.
//!
//! `packages/catalog/src/models.ts` consumes the generated rows verbatim;
//! `build.ts::buildModel` is for discovered/custom specs, not these rows.
//! Metadata remains here in the Host. This module does not resolve auth,
//! discovery, models.yml overrides, selectors, roles, or runtime routes.
//! Upstream MIT notices and exact data provenance are in `../data/`.

use ara_ai::{Model, model_tokenizer::ModelTokenizer};
use serde_json::Value;
use std::{collections::HashMap, error::Error, fmt};

pub const UPSTREAM_COMMIT: &str = "596f2da7101178214aa27a753529d15e6b7ad91d";
pub const BUNDLED_SHA256: &str = "4f609bf5d4f786c3164c77333ac9962dd2c8b0382291a82815667e558ebc04d0";
pub const BUNDLED_JSON: &[u8] = include_bytes!("../data/omp-models.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError(String);

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for CatalogError {}

/// One literal `(provider, id)` identity and its entire materialized row.
/// Callers cannot mutate a row independently of its index keys.
#[derive(Debug, Clone)]
pub struct CatalogModel {
    provider: String,
    id: String,
    metadata: Value,
}

impl CatalogModel {
    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Includes unknown fields, explicit nulls and nested compatibility data.
    pub fn metadata(&self) -> &Value {
        &self.metadata
    }

    /// Produce an execution model only when every execution-affecting field
    /// can be represented. This is NOT model availability or authorization.
    ///
    /// Current `ara_ai::Model` lacks input and context-limit enforcement and
    /// the materialized OMP compat contract. Consequently the fixed bundled
    /// rows cannot yet be used as complete routes by this projection. Keep
    /// their metadata for the future native routing/compat implementation;
    /// never turn missing support into implicit default provider behavior.
    pub fn try_execution_model(&self) -> Result<Model, ProjectionError> {
        let row = self.metadata.as_object().expect("catalog rows validated on load");
        let mut unsupported = Vec::new();
        let api = row["api"].as_str().expect("validated API");
        if !matches!(api, "openai-completions" | "openai-responses" | "anthropic-messages") {
            unsupported.push(UnsupportedContract::new("api", "no native Rust provider binding for this API"));
        }
        let mut tokenizer = None;
        for (field, value) in row {
            match field.as_str() {
                // These fields are represented directly by ara_ai::Model.
                "id" | "provider" | "api" | "baseUrl" | "reasoning" => {}
                "maxTokens" => {
                    if !value.is_null() && value.as_u64().is_none() {
                        unsupported.push(UnsupportedContract::new(field, "limit is not an unsigned integer or null"));
                    }
                }
                "tokenizer" => match value.as_str().and_then(ModelTokenizer::from_name) {
                    Some(known) => tokenizer = Some(known),
                    None => {
                        unsupported.push(UnsupportedContract::new(field, "local tokenizer family is not supported"))
                    }
                },
                // Descriptive/selection/accounting fields remain available in
                // Host metadata; projection does not perform those operations.
                "name"
                | "identity"
                | "cost"
                | "int"
                | "tps"
                | "priority"
                | "premiumMultiplier"
                | "serviceTierCost"
                | "isRecommended"
                | "compatConfig"
                | "supportsComputerUseConfig" => {}
                "requestModelId" if value.as_str() == Some(self.id()) => {}
                "requestModelId" => unsupported.push(UnsupportedContract::new(
                    field,
                    "Rust Model has one id and cannot distinguish local identity from wire model id",
                )),
                "contextWindow" if value.is_null() => {}
                "contextWindow" => unsupported
                    .push(UnsupportedContract::new(field, "catalog context window is not yet projected to execution")),
                "input" => unsupported.push(UnsupportedContract::new(
                    field,
                    "Rust Model does not carry or enforce catalog input modalities",
                )),
                "compat" => unsupported.push(UnsupportedContract::new(
                    field,
                    "materialized OMP compatibility policy is not mapped to provider options",
                )),
                "thinking" => unsupported.push(UnsupportedContract::new(
                    field,
                    "catalog thinking modes, effort levels and ceilings are not mapped",
                )),
                _ => unsupported
                    .push(UnsupportedContract::new(field, "execution contract has no reviewed Rust projection")),
            }
        }
        if !unsupported.is_empty() {
            return Err(ProjectionError { provider: self.provider.clone(), id: self.id.clone(), unsupported });
        }
        Ok(Model {
            id: self.id.clone(),
            api: api.to_owned(),
            provider: self.provider.clone(),
            base_url: row["baseUrl"].as_str().expect("validated base URL").to_owned(),
            reasoning: row["reasoning"].as_bool().expect("validated reasoning flag"),
            max_tokens: row["maxTokens"].as_u64(),
            context_window: None,
            tokenizer,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedContract {
    pub field: String,
    pub reason: &'static str,
}

impl UnsupportedContract {
    fn new(field: &str, reason: &'static str) -> Self {
        Self { field: field.to_owned(), reason }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionError {
    pub provider: String,
    pub id: String,
    pub unsupported: Vec<UnsupportedContract>,
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "catalog model {}/{} cannot be projected for execution", self.provider, self.id)?;
        for contract in &self.unsupported {
            write!(f, "; {}: {}", contract.field, contract.reason)?;
        }
        Ok(())
    }
}

impl Error for ProjectionError {}

/// Provider and row iteration retains JSON insertion order, as the bundled
/// OMP Object/Map traversal does. Hash maps are only used for exact lookup.
#[derive(Debug)]
pub struct ModelCatalog {
    models: Vec<CatalogModel>,
    providers: Vec<String>,
    index: HashMap<String, HashMap<String, usize>>,
}

impl ModelCatalog {
    pub fn bundled() -> Result<Self, CatalogError> {
        Self::from_json(BUNDLED_JSON)
    }

    /// Read already-materialized provider -> id -> row data, not ModelSpec.
    /// As with upstream JSON imports, duplicate object keys use the last
    /// value at the original insertion position. Same ids across providers
    /// are distinct. A row that disagrees with its enclosing keys is rejected.
    pub fn from_json(bytes: &[u8]) -> Result<Self, CatalogError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| CatalogError(error.to_string()))?;
        let Value::Object(providers) = value else {
            return Err(CatalogError("catalog must be a provider object".into()));
        };
        let mut catalog = Self { models: Vec::new(), providers: Vec::new(), index: HashMap::new() };
        for (provider, rows) in providers {
            let Value::Object(rows) = rows else {
                return Err(CatalogError(format!(
                    "catalog provider {provider:?} must contain an object of model rows"
                )));
            };
            let mut index = HashMap::new();
            for (id, metadata) in rows {
                validate_row(&provider, &id, &metadata)?;
                index.insert(id.clone(), catalog.models.len());
                catalog.models.push(CatalogModel { provider: provider.clone(), id, metadata });
            }
            catalog.providers.push(provider.clone());
            catalog.index.insert(provider, index);
        }
        Ok(catalog)
    }

    pub fn find(&self, provider: &str, id: &str) -> Option<&CatalogModel> {
        self.index.get(provider)?.get(id).map(|index| &self.models[*index])
    }

    /// Static bundled provider membership, not configured auth or discovery.
    pub fn has_provider(&self, provider: &str) -> bool {
        self.index.contains_key(provider)
    }

    pub fn providers(&self) -> &[String] {
        &self.providers
    }

    pub fn all(&self) -> &[CatalogModel] {
        &self.models
    }
}

fn validate_row(provider: &str, id: &str, metadata: &Value) -> Result<(), CatalogError> {
    let invalid = |field: &str| CatalogError(format!("catalog {provider}/{id}: invalid or missing {field}"));
    let row = metadata.as_object().ok_or_else(|| invalid("model object"))?;
    if row.get("provider").and_then(Value::as_str) != Some(provider)
        || row.get("id").and_then(Value::as_str) != Some(id)
    {
        return Err(invalid("provider/id identity (must match enclosing keys)"));
    }
    for field in ["name", "api", "baseUrl"] {
        if row.get(field).and_then(Value::as_str).is_none() {
            return Err(invalid(field));
        }
    }
    if row.get("reasoning").and_then(Value::as_bool).is_none() {
        return Err(invalid("reasoning"));
    }
    for field in ["contextWindow", "maxTokens"] {
        if !row.get(field).is_some_and(|value| value.is_null() || value.is_number()) {
            return Err(invalid(field));
        }
    }
    for field in ["identity", "cost"] {
        if !row.get(field).is_some_and(Value::is_object) {
            return Err(invalid(field));
        }
    }
    if !row.get("input").and_then(Value::as_array).is_some_and(|values| values.iter().all(Value::is_string)) {
        return Err(invalid("input"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::LazyLock;

    static CATALOG: LazyLock<ModelCatalog> = LazyLock::new(|| ModelCatalog::bundled().unwrap());

    #[test]
    fn every_fixed_row_decodes_without_metadata_loss_or_identity_aliasing() {
        let original: Value = serde_json::from_slice(BUNDLED_JSON).unwrap();
        assert_eq!(BUNDLED_JSON.len(), 12_244_267);
        assert_eq!(CATALOG.providers().len(), 67);
        assert_eq!(CATALOG.all().len(), 4_776);
        let mut sequence = CATALOG.all().iter();
        for (provider, rows) in original.as_object().unwrap() {
            assert!(CATALOG.has_provider(provider));
            for (id, row) in rows.as_object().unwrap() {
                let model = sequence.next().unwrap();
                assert_eq!((model.provider(), model.id()), (provider.as_str(), id.as_str()));
                assert_eq!(model.metadata(), row);
                assert!(std::ptr::eq(CATALOG.find(provider, id).unwrap(), model));
            }
        }
        assert!(sequence.next().is_none());
        assert!(!CATALOG.has_provider("unconfigured-provider"));
        assert!(CATALOG.find("anthropic", "not-a-model").is_none());
    }

    #[test]
    fn fixed_nulls_extended_policy_and_literal_max_ids_are_preserved() {
        let unknown = CATALOG.find("aimlapi", "act_two").unwrap().metadata();
        assert!(unknown["contextWindow"].is_null());
        assert!(unknown["maxTokens"].is_null());
        let claude = CATALOG.find("anthropic", "claude-opus-4-6").unwrap().metadata();
        assert_eq!(claude["identity"], json!({"class":"anthropic","family":"opus","revision":"4.6.0"}));
        assert_eq!(claude["thinking"]["mode"], "anthropic-adaptive");
        assert_eq!(claude["compat"]["supportsEagerToolInputStreaming"], true);
        let gpt = CATALOG.find("openai", "gpt-5.4").unwrap().metadata();
        assert_eq!(gpt["applyPatchToolType"], "freeform");
        assert_eq!(gpt["supportsComputerUse"], true);
        assert!(CATALOG.find("nanogpt", "anthropic/claude-opus-4.6:thinking:max").is_some());
        assert!(CATALOG.find("nanogpt", "nanogpt/coding-router:max").is_some());
    }

    fn row(provider: &str, id: &str) -> Value {
        json!({"id":id,"provider":provider,"name":"Fixture","api":"openai-completions",
            "baseUrl":"http://localhost","reasoning":false,"input":["text"],
            "cost":{},"identity":{"class":"unknown"},"contextWindow":null,"maxTokens":null})
    }

    #[test]
    fn future_fields_and_literal_auto_ids_do_not_become_selector_syntax() {
        // Fixed bundled data has literal :max entries but no :auto entry.
        // models.ts accepts arbitrary dictionary ids without parsing suffixes.
        let mut entry = row("fixture", "family/model:auto");
        entry["futurePolicy"] = json!({"nested":[null, {"flag":true}],"large":9_007_199_254_740_993u64});
        let catalog =
            ModelCatalog::from_json(&serde_json::to_vec(&json!({"fixture":{"family/model:auto":entry}})).unwrap())
                .unwrap();
        let model = catalog.find("fixture", "family/model:auto").unwrap();
        assert_eq!(model.metadata(), &entry);
        assert!(catalog.find("fixture", "family/model").is_none());
        assert!(model.try_execution_model().unwrap_err().unsupported.iter().any(|item| item.field == "futurePolicy"));
    }

    #[test]
    fn duplicate_json_keys_match_json_import_and_same_ids_remain_provider_scoped() {
        let mut first = row("one", "same");
        first["name"] = json!("First");
        let last = row("one", "same");
        let second = row("two", "same");
        let input = [
            r#"{"one":{"same":"#,
            &first.to_string(),
            r#", "same":"#,
            &last.to_string(),
            r#"},"two":{"same":"#,
            &second.to_string(),
            "}}",
        ]
        .concat();
        let catalog = ModelCatalog::from_json(input.as_bytes()).unwrap();
        assert_eq!(catalog.all().len(), 2);
        assert_eq!(catalog.find("one", "same").unwrap().metadata(), &last);
        assert_eq!(catalog.find("two", "same").unwrap().metadata(), &second);
        let wrong = json!({"one":{"different":last}});
        assert!(
            ModelCatalog::from_json(&serde_json::to_vec(&wrong).unwrap()).unwrap_err().to_string().contains("identity")
        );
    }

    #[test]
    fn malformed_materialized_rows_fail_without_inventing_identity_or_limits() {
        for input in [b"[]".as_slice(), br#"{"provider":[]}"#, b"{"] {
            assert!(ModelCatalog::from_json(input).is_err());
        }
        for field in ["identity", "reasoning", "contextWindow", "input"] {
            let mut entry = row("fixture", "model");
            entry.as_object_mut().unwrap().remove(field);
            let error =
                ModelCatalog::from_json(&serde_json::to_vec(&json!({"fixture":{"model":entry}})).unwrap()).unwrap_err();
            assert!(error.to_string().contains(field), "{error}");
        }
    }

    #[test]
    fn execution_projection_reports_contracts_instead_of_silently_dropping_them() {
        // No bundled row is a complete route expressible by today's Model.
        for model in CATALOG.all() {
            let error = model.try_execution_model().unwrap_err();
            assert!(error.unsupported.iter().any(|item| item.field == "input"));
        }
        for (field, value) in [
            ("requestModelId", json!("upstream-id")),
            ("tokenizer", json!("qwen3")),
            ("contextWindow", json!(200_000)),
            ("compat", json!({"wireModelIdMode":"openrouter"})),
            ("thinking", json!({"mode":"effort","efforts":["high"]})),
            ("limits", json!({"futureBudget":20})),
            ("api", json!("openai-codex-responses")),
        ] {
            let mut entry = row("fixture", "model");
            entry[field] = value;
            let catalog =
                ModelCatalog::from_json(&serde_json::to_vec(&json!({"fixture":{"model":entry}})).unwrap()).unwrap();
            let error = catalog.find("fixture", "model").unwrap().try_execution_model().unwrap_err();
            assert!(error.unsupported.iter().any(|item| item.field == field), "{field}: {error}");
        }
    }
}
