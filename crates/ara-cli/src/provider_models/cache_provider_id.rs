//! Fixed OMP provider-models/cache-provider-id.ts. Native Bun hash namespace.
use super::descriptor_types::ProviderEnvironment;
use crate::bun_hash::hash_string_base36;
use ara_rpc::WireString;
pub fn is_credential_scoped_model_cache_provider(provider: &WireString) -> bool {
    ["opencode-go", "opencode-zen", "github-copilot"].iter().any(|id| provider.equals_ascii(id))
}
pub fn get_default_model_discovery_base_url(
    provider: &WireString,
    env: &dyn ProviderEnvironment,
) -> Option<WireString> {
    match provider.to_utf8().ok()?.as_str() {
        "ollama" => Some("http://127.0.0.1:11434".into()),
        "litellm" => Some(env.get("LITELLM_BASE_URL").unwrap_or_else(|| "http://localhost:4000/v1".into())),
        "opencode-go" => Some("https://opencode.ai/zen/go/v1".into()),
        "opencode-zen" => Some("https://opencode.ai/zen/v1".into()),
        "vllm" => Some("http://127.0.0.1:8000/v1".into()),
        _ => None,
    }
}
pub fn resolve_ollama_model_cache_provider_id(provider: &WireString, base: Option<&WireString>) -> WireString {
    let mut endpoint = WireString::from("http://127.0.0.1:11434");
    if let Ok(url) = reqwest::Url::parse(&String::from_utf16_lossy(base.unwrap_or(&endpoint).units())) {
        let path = url.path().trim_end_matches('/');
        let path = path.strip_suffix("/v1").unwrap_or(path);
        let host = url.host_str().unwrap_or("");
        let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_owned() };
        let port = url.port().map_or_else(String::new, |p| format!(":{p}"));
        endpoint = format!("{}://{host}{port}{path}", url.scheme()).into();
    }
    let mut out = provider.clone();
    out.append_str(&format!(":ollama-models-v1:{}", hash_string_base36(&endpoint)));
    out
}
fn trim_one_slash(value: &WireString) -> WireString {
    if value.units().last() == Some(&47) { value.slice_prefix(value.len() - 1) } else { value.clone() }
}
pub fn resolve_model_cache_provider_id(
    provider: &WireString,
    api_key: Option<&WireString>,
    base: Option<&WireString>,
    env: &dyn ProviderEnvironment,
) -> WireString {
    let name = provider.to_utf8().unwrap_or_default();
    match name.as_str() {
        "ollama" => resolve_ollama_model_cache_provider_id(provider, base),
        "cursor" => "cursor:default-effort-v4".into(),
        "openrouter" => "openrouter:pseudo-api".into(),
        "litellm" | "vllm" => {
            let base =
                base.cloned().or_else(|| get_default_model_discovery_base_url(provider, env)).expect("known default");
            format!("{name}:{}:{}", if name == "litellm" { "rich-v8" } else { "models-v2" }, hash_string_base36(&base))
                .into()
        }
        "opencode-go" | "opencode-zen" => {
            let base =
                base.cloned().or_else(|| get_default_model_discovery_base_url(provider, env)).expect("known default");
            let mut base = trim_one_slash(&base);
            if !base.units().ends_with(&"/v1".encode_utf16().collect::<Vec<_>>()) {
                base.append_str("/v1");
            }
            let mut scope = api_key.cloned().unwrap_or_else(|| "".into());
            scope.append_str("\0");
            scope = join(&scope, &base);
            format!("{name}:models-v3:{}", hash_string_base36(&scope)).into()
        }
        "github-copilot" => {
            let base = base.cloned().unwrap_or_else(|| "https://api.githubcopilot.com".into());
            let mut scope = api_key.cloned().unwrap_or_else(|| "".into());
            scope.append_str("\0");
            scope = join(&scope, &base);
            format!("github-copilot:models-v1:{}", hash_string_base36(&scope)).into()
        }
        _ => provider.clone(),
    }
}
pub(crate) fn join(a: &WireString, b: &WireString) -> WireString {
    let mut units = a.units().to_vec();
    units.extend_from_slice(b.units());
    WireString::from_units(units)
}
