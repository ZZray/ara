//! Fixed OMP 596f2da: catalog/src/discovery/gitlab-duo-workflow.ts.
//! MIT License; Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE for the full license.

use super::*;
use crate::js_regex::JsRegExp;
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

pub const GITLAB_DEFAULT_BASE_URL: &str = "https://gitlab.com";
pub const FALLBACK_MODEL_ID: &str = "claude_sonnet_4_6_vertex";
pub const FALLBACK_MODEL_NAME: &str = "Claude Sonnet 4.6 - Vertex";
const AVAILABLE_QUERY: &str = "query lsp_aiChatAvailableModels($rootNamespaceId: GroupID!) {\n  aiChatAvailableModels(rootNamespaceId: $rootNamespaceId) {\n    defaultModel { name ref }\n    selectableModels { name ref }\n    pinnedModel { name ref }\n  }\n}";
const PROJECT_QUERY: &str = "query omp_gitlabDuoWorkflowProjectRootNamespace($fullPath: ID!) {\n  project(fullPath: $fullPath) {\n    namespace {\n      id\n      rootAncestor { id }\n    }\n  }\n}";

/// Host filesystem seam. Failures have the same null result as fs.readFile's catch.
#[async_trait]
pub trait GitLabFs: Send + Sync {
    async fn read_text(&self, path: &Path) -> Option<WireString>;
}
pub struct NativeGitLabFs;
#[async_trait]
impl GitLabFs for NativeGitLabFs {
    async fn read_text(&self, path: &Path) -> Option<WireString> {
        tokio::fs::read(path).await.ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned().into())
    }
}
#[derive(Clone)]
pub struct GitLabHost {
    pub env: HashMap<String, WireString>,
    pub cwd: PathBuf,
    pub fs: Arc<dyn GitLabFs>,
}
impl GitLabHost {
    pub fn new(cwd: PathBuf, env: HashMap<String, WireString>, fs: Arc<dyn GitLabFs>) -> Self {
        Self { cwd, env, fs }
    }
}
#[derive(Clone)]
pub struct GitLabDiscoveryConfig {
    pub api_key: WireString,
    pub base_url: Option<WireString>,
    pub namespace_id: Option<WireString>,
    pub project_id: Option<WireString>,
    pub project_path: Option<WireString>,
    pub cwd: Option<PathBuf>,
}
impl Default for GitLabDiscoveryConfig {
    fn default() -> Self {
        Self { api_key: "".into(), base_url: None, namespace_id: None, project_id: None, project_path: None, cwd: None }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabModelRef {
    pub name: WireString,
    pub model_ref: WireString,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabNamespaceSelection {
    pub root_namespace_id: WireString,
    pub namespace_path: Option<WireString>,
    pub project_path: Option<WireString>,
    pub source: &'static str,
}

fn concat(left: &WireString, right: &WireString) -> WireString {
    let mut units = left.units().to_vec();
    units.extend(right.units());
    WireString::from_units(units)
}
fn regex(pattern: &str, flags: &str, value: &WireString) -> Option<crate::js_regex::JsRegexMatch> {
    JsRegExp::new(pattern.into(), flags).expect("fixed GitLab pattern").exec(value)
}
fn nonempty(value: &WireString) -> Option<WireString> {
    let value = js_trim(value);
    (!value.is_empty()).then_some(value)
}
fn identifier(value: Option<&WireValue>) -> Option<WireString> {
    match value? {
        WireValue::String(value) => nonempty(value),
        WireValue::Number(value) => Some(if value.is_nan() {
            "NaN".into()
        } else if *value == f64::INFINITY {
            "Infinity".into()
        } else if *value == f64::NEG_INFINITY {
            "-Infinity".into()
        } else {
            WireValue::Number(*value).stringify().into()
        }),
        _ => None,
    }
}
fn normalized_base(value: Option<&WireString>) -> WireString {
    let value = value.and_then(nonempty).unwrap_or_else(|| GITLAB_DEFAULT_BASE_URL.into());
    let value = trim_trailing_slashes(&value);
    if value.is_empty() { GITLAB_DEFAULT_BASE_URL.into() } else { value }
}
fn object(value: Option<&WireValue>) -> Option<&WireValue> {
    value.filter(|value| value.is_object())
}
fn field(value: Option<&WireValue>, key: &str) -> Option<WireString> {
    identifier(value.and_then(|value| value.get(key)))
}
fn nested_root(value: &WireValue) -> Option<&WireValue> {
    ["root_namespace", "rootNamespace", "root_ancestor", "rootAncestor"].iter().find_map(|key| object(value.get(key)))
}
fn root_id(value: &WireValue, explicit: bool) -> Option<WireString> {
    if !value.is_object() {
        return None;
    }
    if let Some(id) = identifier(value.get("root_namespace_id")).or_else(|| identifier(value.get("rootNamespaceId"))) {
        return Some(id);
    }
    if let Some(root) = nested_root(value) {
        let id = field(Some(root), "id")
            .or_else(|| field(Some(root), "full_path"))
            .or_else(|| field(Some(root), "fullPath"));
        if explicit || id.is_some() {
            return id;
        }
    }
    if let Some(namespace) = object(value.get("namespace")) {
        return root_id(namespace, explicit).or_else(|| {
            if explicit {
                None
            } else {
                field(Some(namespace), "id")
                    .or_else(|| field(Some(namespace), "full_path"))
                    .or_else(|| field(Some(namespace), "fullPath"))
            }
        });
    }
    if explicit {
        None
    } else {
        field(Some(value), "id").or_else(|| field(Some(value), "full_path")).or_else(|| field(Some(value), "fullPath"))
    }
}
fn namespace_path(value: &WireValue) -> Option<WireString> {
    if !value.is_object() {
        return None;
    }
    field(Some(value), "full_path").or_else(|| field(Some(value), "fullPath")).or_else(|| field(Some(value), "path"))
}
fn parse_model(value: Option<&WireValue>) -> Option<GitLabModelRef> {
    let value = object(value)?;
    // resilientString rejects numeric refs/names before normalizeIdentifier.
    let model_ref = value.get("ref").and_then(WireValue::as_string).and_then(nonempty)?;
    let name = value.get("name").and_then(WireValue::as_string).and_then(nonempty).unwrap_or_else(|| model_ref.clone());
    Some(GitLabModelRef { name, model_ref })
}
fn availability(value: &WireValue) -> Option<Vec<GitLabModelRef>> {
    if !value.is_object() {
        return None;
    }
    if let Some(model) = parse_model(value.get("pinnedModel")) {
        return Some(vec![model]);
    }
    let selected: Vec<_> = value
        .get("selectableModels")
        .and_then(WireValue::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|value| parse_model(Some(value)))
        .collect();
    if !selected.is_empty() {
        return Some(selected);
    }
    Some(parse_model(value.get("defaultModel")).into_iter().collect())
}
fn json_headers(key: &WireString) -> Result<Vec<(WireString, WireString)>, DiscoveryError> {
    let mut headers = DiscoveryHeaders::from_pairs(&[])?;
    for (name, value) in [("Accept", "application/json"), ("Content-Type", "application/json")] {
        headers.set(&name.into(), &value.into())?;
    }
    headers.set(&"Authorization".into(), &concat(&"Bearer ".into(), key))?;
    Ok(headers.entries())
}
fn encode_component(value: &WireString) -> Result<String, DiscoveryError> {
    let text = value.to_utf8().map_err(|_| DiscoveryError::named("URIError", "URI malformed"))?;
    let mut output = String::new();
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(byte) {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(output)
}
async fn get(context: &CatalogContext, config: &GitLabDiscoveryConfig, url: WireString) -> Option<DiscoveryReply> {
    let headers = json_headers(&config.api_key).ok()?;
    let reply = context.transport.fetch(DiscoveryRequest { url, headers, ..Default::default() }).await.ok()?;
    reply.ok().then_some(reply)
}
async fn graphql(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    base: &WireString,
    query: &str,
    key: &str,
    value: WireString,
) -> Option<VariantSpec> {
    let headers = json_headers(&config.api_key).ok()?;
    let body = wire_object(&[("query", wire_string(query)), ("variables", wire_object(&[(key, wire_string(value))]))])
        .stringify()
        .into_bytes();
    let reply = context
        .transport
        .fetch(DiscoveryRequest {
            url: concat(base, &"/api/graphql".into()),
            method: HttpMethod::Post,
            headers,
            body: Some(body),
            ..Default::default()
        })
        .await
        .ok()?;
    if !reply.ok() {
        return None;
    }
    reply.json().ok()
}
async fn available(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    base: &WireString,
    id: &WireString,
) -> Option<Vec<GitLabModelRef>> {
    let id = if regex(r"^\d+$", "", id).is_some() { concat(&"gid://gitlab/Group/".into(), id) } else { id.clone() };
    let payload = graphql(context, config, base, AVAILABLE_QUERY, "rootNamespaceId", id).await?;
    availability(object(payload.get("data"))?.get("aiChatAvailableModels")?)
}
async fn override_candidate(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    base: &WireString,
    id: &WireString,
) -> Option<GitLabNamespaceSelection> {
    let rest = regex(r"^gid://gitlab/(?:Group|Namespace)/(\d+)$", "", id)
        .and_then(|found| found.captures.get(1).cloned().flatten())
        .or_else(|| regex(r"^\d+$", "", id).map(|_| id.clone()))?;
    let url = concat(base, &format!("/api/v4/groups/{}", encode_component(&rest).ok()?).into());
    let payload = get(context, config, url).await?.json().ok()?;
    Some(GitLabNamespaceSelection {
        root_namespace_id: root_id(&payload.value, false).unwrap_or_else(|| id.clone()),
        namespace_path: namespace_path(&payload.value),
        project_path: None,
        source: "override",
    })
}
async fn project_root(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    base: &WireString,
    project: &WireString,
) -> Option<WireString> {
    // encodeURIComponent is evaluated inside the upstream fetch try.
    let rest = if let Ok(encoded) = encode_component(project) {
        get(context, config, concat(base, &format!("/api/v4/projects/{encoded}").into()))
            .await
            .and_then(|response| response.json().ok())
    } else {
        None
    };
    if let Some(root) = rest.as_ref().and_then(|payload| root_id(&payload.value, true)) {
        return Some(root);
    }
    let full_path = rest
        .as_ref()
        .filter(|payload| payload.value.is_object())
        .and_then(|payload| {
            identifier(payload.get("path_with_namespace")).or_else(|| identifier(payload.get("fullPath")))
        })
        .or_else(|| project.units().contains(&47).then(|| project.clone()))?;
    let payload = graphql(context, config, base, PROJECT_QUERY, "fullPath", full_path).await?;
    root_id(object(object(payload.get("data"))?.get("project"))?, true)
}
async fn groups(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    base: &WireString,
) -> Result<Vec<GitLabNamespaceSelection>, DiscoveryError> {
    let mut candidates = Vec::new();
    let mut next = Some(WireString::from("1"));
    for _ in 0..50 {
        let Some(page) = next.take() else {
            break;
        };
        // URL construction is deliberately outside the fetch catch.
        let mut url = reqwest::Url::parse(&String::from_utf16_lossy(concat(base, &"/api/v4/groups".into()).units()))
            .map_err(|_| DiscoveryError::named("TypeError", "Invalid URL"))?;
        url.query_pairs_mut()
            .append_pair("top_level_only", "true")
            .append_pair("per_page", "100")
            .append_pair("order_by", "name")
            .append_pair("sort", "asc")
            .append_pair("page", &String::from_utf16_lossy(page.units()));
        let Some(response) = get(context, config, url.as_str().into()).await else {
            break;
        };
        let Ok(payload) = response.json() else {
            break;
        };
        let Some(items) = payload.value.as_array() else {
            break;
        };
        for item in items {
            if let Some(id) = root_id(item, false) {
                let preferred = matches!(item.get("duo_features_enabled"), Some(WireValue::Bool(true)))
                    || matches!(item.get("duo_core_features_enabled"), Some(WireValue::Bool(true)));
                candidates.push((
                    preferred,
                    GitLabNamespaceSelection {
                        root_namespace_id: id,
                        namespace_path: namespace_path(item),
                        project_path: None,
                        source: "group",
                    },
                ));
            }
        }
        next = response.header("x-next-page").and_then(nonempty);
    }
    candidates.sort_by_key(|(preferred, _)| !preferred);
    Ok(candidates.into_iter().map(|(_, candidate)| candidate).collect())
}
async fn validate(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    base: &WireString,
    mut candidate: GitLabNamespaceSelection,
    runtime: bool,
) -> Option<GitLabNamespaceSelection> {
    candidate.root_namespace_id = nonempty(&candidate.root_namespace_id)?;
    candidate.namespace_path = candidate.namespace_path.as_ref().and_then(nonempty);
    if runtime {
        candidate.project_path = candidate.project_path.as_ref().and_then(nonempty);
        return Some(candidate);
    }
    if available(context, config, base, &candidate.root_namespace_id).await?.is_empty() {
        return None;
    }
    // Upstream validateNamespaceCandidate intentionally does not carry projectPath.
    candidate.project_path = None;
    Some(candidate)
}
async fn select(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    host: &GitLabHost,
    runtime: bool,
) -> Result<GitLabNamespaceSelection, DiscoveryError> {
    let base = normalized_base(config.base_url.as_ref());
    let namespace = config
        .namespace_id
        .as_ref()
        .and_then(nonempty)
        .or_else(|| host.env.get("GITLAB_DUO_NAMESPACE_ID").and_then(nonempty));
    if let Some(id) = namespace {
        let candidate = if runtime { override_candidate(context, config, &base, &id).await } else { None }.unwrap_or(
            GitLabNamespaceSelection {
                root_namespace_id: id,
                namespace_path: None,
                project_path: None,
                source: "override",
            },
        );
        if let Some(candidate) = validate(context, config, &base, candidate, runtime).await {
            return Ok(candidate);
        }
    }
    let project = config
        .project_id
        .as_ref()
        .and_then(nonempty)
        .or_else(|| config.project_path.as_ref().and_then(nonempty))
        .or_else(|| host.env.get("GITLAB_DUO_PROJECT_ID").and_then(nonempty))
        .or_else(|| host.env.get("GITLAB_DUO_PROJECT_PATH").and_then(nonempty));
    if let Some(project) = project
        && let Some(id) = project_root(context, config, &base, &project).await
    {
        let candidate = GitLabNamespaceSelection {
            root_namespace_id: id,
            namespace_path: None,
            project_path: project.units().contains(&47).then_some(project),
            source: "project",
        };
        if let Some(candidate) = validate(context, config, &base, candidate, runtime).await {
            return Ok(candidate);
        }
    }
    if let Some(project) = discover_remote_project(config, host, &base).await
        && let Some(id) = project_root(context, config, &base, &project).await
    {
        let candidate = GitLabNamespaceSelection {
            root_namespace_id: id,
            namespace_path: None,
            project_path: Some(project),
            source: "remote",
        };
        if let Some(candidate) = validate(context, config, &base, candidate, runtime).await {
            return Ok(candidate);
        }
    }
    for candidate in groups(context, config, &base).await? {
        if let Some(candidate) = validate(context, config, &base, candidate, runtime).await {
            return Ok(candidate);
        }
    }
    Err(DiscoveryError::new(if runtime {
        "Unable to find a GitLab Duo Workflow namespace. Set GITLAB_DUO_NAMESPACE_ID to a root namespace or GITLAB_DUO_PROJECT_ID to a GitLab project."
    } else {
        "Unable to find a GitLab Duo Workflow namespace with available models. Set GITLAB_DUO_NAMESPACE_ID to a root namespace with Duo model access."
    }))
}
pub async fn discover_gitlab_duo_workflow_namespace(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    host: &GitLabHost,
) -> Result<GitLabNamespaceSelection, DiscoveryError> {
    select(context, config, host, false).await
}
pub async fn discover_gitlab_duo_workflow_runtime_namespace(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    host: &GitLabHost,
) -> Result<GitLabNamespaceSelection, DiscoveryError> {
    select(context, config, host, true).await
}
pub async fn fetch_gitlab_duo_workflow_models(
    context: &CatalogContext,
    config: &GitLabDiscoveryConfig,
    host: &GitLabHost,
) -> DiscoveryResult {
    let selection = discover_gitlab_duo_workflow_namespace(context, config, host).await?;
    let base = normalized_base(config.base_url.as_ref());
    let Some(models) = available(context, config, &base, &selection.root_namespace_id).await else {
        return Ok(None);
    };
    if models.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        models
            .iter()
            .map(|model| build_gitlab_duo_workflow_model_spec(model, Some(&base), Some(&selection.root_namespace_id)))
            .collect(),
    ))
}
pub fn build_gitlab_duo_workflow_model_spec(
    model: &GitLabModelRef,
    base_url: Option<&WireString>,
    root_namespace_id: Option<&WireString>,
) -> SpecRef {
    let window = [
        (r"claude[_-]?opus", 1_000_000.0),
        (r"claude[_-]?sonnet", 1_000_000.0),
        (r"claude[_-]?haiku", 200_000.0),
        ("gemini", 1_000_000.0),
        (r"gpt[_-]?5", 400_000.0),
    ]
    .iter()
    .find_map(|(pattern, window)| regex(pattern, "i", &model.model_ref).map(|_| *window))
    .unwrap_or(200_000.0);
    let mut value = VariantSpec::from_wire(wire_object(&[
        ("id", wire_string(model.model_ref.clone())),
        ("name", wire_string(model.name.clone())),
        ("api", wire_string("gitlab-duo-agent")),
        ("provider", wire_string("gitlab-duo-agent")),
        ("baseUrl", wire_string(normalized_base(base_url))),
        ("reasoning", WireValue::Bool(false)),
        ("input", WireValue::Array(vec![wire_string("text")])),
        ("cost", zero_cost()),
        ("contextWindow", WireValue::Number(window)),
        ("maxTokens", WireValue::Null),
        ("supportsTools", WireValue::Bool(true)),
    ]));
    if let Some(id) = root_namespace_id.filter(|id| !id.is_empty()) {
        value.set("gitlabDuoWorkflowRootNamespaceId", wire_string(id.clone()));
    }
    Arc::new(value)
}
pub fn build_gitlab_duo_workflow_fallback_model(
    id: Option<&WireString>,
    name: Option<&WireString>,
    base_url: Option<&WireString>,
) -> SpecRef {
    build_gitlab_duo_workflow_model_spec(
        &GitLabModelRef {
            name: name.cloned().unwrap_or_else(|| FALLBACK_MODEL_NAME.into()),
            model_ref: id.cloned().unwrap_or_else(|| FALLBACK_MODEL_ID.into()),
        },
        base_url,
        None,
    )
}

fn resolve(base: &Path, value: &Path) -> PathBuf {
    let joined = if value.is_absolute() { value.to_owned() } else { base.join(value) };
    let mut output = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Node path.resolve clamps parent traversal at the volume root.
                if matches!(output.components().next_back(), Some(Component::Normal(_))) {
                    output.pop();
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}
fn wire_path(value: &WireString) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(value.units()))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(String::from_utf16_lossy(value.units()))
    }
}
async fn dot_git_config(host: &GitLabHost, git: &Path) -> Option<WireString> {
    if let Some(text) = host.fs.read_text(&git.join("config")).await {
        return Some(text);
    }
    let text = host.fs.read_text(git).await?;
    let directory = regex(r"^gitdir:\s*(.+)$", "im", &text)?.captures.get(1)?.as_ref().and_then(nonempty)?;
    let path = resolve(git.parent().unwrap_or(git), &wire_path(&directory));
    if let Some(common) = host.fs.read_text(&path.join("commondir")).await.filter(|text| !text.is_empty()) {
        let common = resolve(&path, &wire_path(&js_trim(&common)));
        if let Some(text) = host.fs.read_text(&common.join("config")).await {
            return Some(text);
        }
    }
    host.fs.read_text(&path.join("config")).await
}
pub async fn read_git_config_text(host: &GitLabHost, start: &Path) -> Option<WireString> {
    let mut current = resolve(&host.cwd, start);
    loop {
        if let Some(text) = dot_git_config(host, &current.join(".git")).await.filter(|text| !text.is_empty()) {
            return Some(text);
        }
        let parent = current.parent()?;
        if parent == current {
            return None;
        }
        current = parent.to_owned();
    }
}
pub fn parse_git_remote_urls(text: &WireString) -> Vec<WireString> {
    let mut urls = Vec::new();
    let mut remote = false;
    for units in text.units().split(|unit| *unit == 10) {
        let units = units.strip_suffix(&[13]).unwrap_or(units);
        let line = WireString::from_units(units.to_vec());
        if let Some(section) = regex(r"^\s*\[([^\]]+)\]", "", &line) {
            remote = section
                .captures
                .get(1)
                .and_then(Option::as_ref)
                .is_some_and(|text| regex(r#"^remote\s+"[^"]+"$"#, "", &js_trim(text)).is_some());
            continue;
        }
        if remote
            && let Some(value) =
                regex(r"^\s*url\s*=\s*(.+?)\s*$", "", &line).and_then(|found| found.captures.get(1).cloned().flatten())
        {
            urls.push(value);
        }
    }
    urls
}
fn host(url: &reqwest::Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or("")),
        None => url.host_str().unwrap_or("").to_owned(),
    }
}
pub fn parse_gitlab_remote_project_path(
    remote: &WireString,
    expected_host: Option<&str>,
    base_path: &str,
) -> Option<WireString> {
    let (remote_host, mut project, insensitive) = match reqwest::Url::parse(&String::from_utf16_lossy(remote.units())) {
        Ok(url) => (host(&url), WireString::from(url.path()), url.scheme() == "ssh"),
        Err(_) => {
            let captures = regex(r"^(?:[^@]+@)?([^:]+):(.+)$", "", remote)?.captures;
            (String::from_utf16_lossy(captures.get(1)?.as_ref()?.units()), captures.get(2)?.clone()?, true)
        }
    };
    if let Some(expected) = expected_host {
        let matches = if insensitive {
            remote_host
                .split(':')
                .next()
                .unwrap_or(&remote_host)
                .eq_ignore_ascii_case(expected.split(':').next().unwrap_or(expected))
        } else {
            remote_host.eq_ignore_ascii_case(expected)
        };
        if !matches {
            return None;
        }
    }
    while project.units().first() == Some(&47) {
        project = WireString::from_units(project.units()[1..].to_vec());
    }
    let base: Vec<u16> = base_path.encode_utf16().collect();
    if !base.is_empty()
        && (project.units() == base
            || (project.units().starts_with(&base) && project.units().get(base.len()) == Some(&47)))
    {
        project = WireString::from_units(project.units()[base.len()..].to_vec());
    }
    let units = project.units();
    let start = units.iter().position(|unit| *unit != 47).unwrap_or(units.len());
    let end = units.iter().rposition(|unit| *unit != 47).map_or(start, |index| index + 1);
    project = WireString::from_units(units[start..end].to_vec());
    if regex(r"\.git$", "i", &project).is_some() {
        project = WireString::from_units(project.units()[..project.units().len() - 4].to_vec());
    }
    project.units().contains(&47).then_some(project)
}
async fn discover_remote_project(
    config: &GitLabDiscoveryConfig,
    host_context: &GitLabHost,
    base: &WireString,
) -> Option<WireString> {
    let config_text = read_git_config_text(host_context, config.cwd.as_deref().unwrap_or(&host_context.cwd)).await?;
    let base = reqwest::Url::parse(&String::from_utf16_lossy(base.units())).ok();
    let expected = base.as_ref().map(host);
    let base_path = base.as_ref().map(|url| url.path().trim_matches('/')).unwrap_or("");
    parse_git_remote_urls(&config_text)
        .iter()
        .find_map(|url| parse_gitlab_remote_project_path(url, expected.as_deref(), base_path))
}
