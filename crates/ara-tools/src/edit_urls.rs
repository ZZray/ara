//! Host adaptation of OMP b91db66e edit/index.ts URL handshake (MIT).
//! Router declarations and backing grants remain outside the shared edit Core.
use crate::edit_host::EditHostPathPolicy;
use crate::{ContentUriEditPolicy, ContentUriEditSpec, ContentUriRoute, ToolContext};
use ara_agent::ToolError;
use ara_edit::path_policy::UrlResolution;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub fn path_policy(ctx: &ToolContext) -> ara_edit::PathPolicy {
    let state = ctx.edit_host().map(|host| host.path_policy()).unwrap_or_default();
    let mut policy = state.file_policy(&ctx.cwd);
    policy.home_dir = state.home_dir.unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default());
    let mut specs = ctx.content_uri_port().map(|p| p.edit_specs()).unwrap_or_default();
    // Native local/skill declarations are Host fallbacks, after explicit ports.
    // Skill resources are immutable in the new upstream SchemeSpec.
    if !specs.iter().any(|s| s.scheme.eq_ignore_ascii_case("skill")) {
        specs.push(ContentUriEditSpec {
            scheme: "skill".into(),
            single_slash_alias: false,
            line_selectors: true,
            port_authority: false,
            immutable: true,
            policy: ContentUriEditPolicy::ReadOnly,
        });
    }
    for spec in &specs {
        if !policy.url_schemes.iter().any(|s| s.eq_ignore_ascii_case(&spec.scheme)) {
            policy.url_schemes.push(spec.scheme.clone());
        }
        policy.url_alias_schemes.retain(|s| !s.eq_ignore_ascii_case(&spec.scheme));
        if spec.single_slash_alias {
            policy.url_alias_schemes.push(spec.scheme.clone());
        }
    }
    if specs.iter().any(|s| s.scheme.eq_ignore_ascii_case("local")) {
        // An override/tombstone cannot inherit the native sandbox's plain-path grant.
        policy.plan_writable_roots.clear();
    }
    if let Some(port) = ctx.content_uri_port() {
        for (scheme, root) in port.edit_sandbox_roots() {
            if specs.iter().find(|spec| spec.scheme.eq_ignore_ascii_case(&scheme)).is_some_and(|spec| {
                !spec.immutable && spec.policy == ContentUriEditPolicy::File { plan_writable: true }
            }) {
                policy.plan_writable_roots.push(root);
            }
        }
    }
    policy
}

#[derive(Clone)]
pub struct EditUrlResolver {
    ctx: ToolContext,
}
impl EditUrlResolver {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
    pub async fn resolve(&self, authored: &str, cancel: CancellationToken) -> UrlResolution {
        match self.resolve_inner(authored, cancel).await {
            Ok(answer) => answer,
            Err(error) => UrlResolution { absolute: None, error: Some(error.0), plan_writable: false },
        }
    }
    async fn resolve_inner(&self, authored: &str, cancel: CancellationToken) -> Result<UrlResolution, ToolError> {
        check_cancel(&cancel)?;
        let (url, selector) = split_edit_target(&self.ctx, authored);
        if let Some(sel) = selector
            && !sel.eq_ignore_ascii_case("raw")
            && !sel.eq_ignore_ascii_case("conflicts")
        {
            return Err(ToolError(format!(
                "edit does not accept the trailing selector \":{sel}\" — it addresses a whole file. Remove \":{sel}\", or if the filename truly ends with it, percent-encode the \":\" as %3A."
            )));
        }
        if let Some(port) = self.ctx.content_uri_port() {
            // Re-read live route/spec on every resolution, including apply.
            match port.route(&url) {
                ContentUriRoute::Removed => return Err(ToolError(format!("Unknown internal URL: {authored}"))),
                ContentUriRoute::Registered { .. } => {
                    let scheme = crate::internal_urls::hierarchical_scheme(&url).unwrap_or_default();
                    let spec = port.edit_specs().into_iter().find(|s| s.scheme.eq_ignore_ascii_case(scheme));
                    let Some(spec) = spec else {
                        return Err(ToolError(format!("{authored} has no Host edit file grant")));
                    };
                    let plan_writable = match spec.policy {
                        ContentUriEditPolicy::Handler => {
                            return Err(ToolError(format!("{authored} is written through `write`, not edited")));
                        }
                        ContentUriEditPolicy::File { plan_writable } if !spec.immutable => plan_writable,
                        _ => return Err(ToolError(format!("{authored} is read-only"))),
                    };
                    let backing = port.resolve_edit_file(&url, cancel.clone()).await?;
                    check_cancel(&cancel)?;
                    if backing.as_ref().is_some_and(|file| file.immutable) {
                        return Err(ToolError(format!("{authored} is read-only")));
                    }
                    return Ok(UrlResolution { absolute: backing.map(|file| file.path), error: None, plan_writable });
                }
                ContentUriRoute::Unregistered => {}
            }
        }
        if crate::internal_urls::hierarchical_scheme(&url).is_some_and(|s| s.eq_ignore_ascii_case("skill")) {
            return Err(ToolError(format!("{authored} is read-only")));
        }
        let host = self.ctx.edit_host();
        let url = url.clone();
        let answer = tokio::task::spawn_blocking(move || match host {
            Some(host) => host.resolve_edit_native_url(&url),
            None => Ok(None),
        })
        .await
        .map_err(|error| ToolError(format!("Edit URL resolver failed: {error}")))??;
        check_cancel(&cancel)?;
        answer.ok_or_else(|| ToolError(format!("Unknown internal URL: {authored}")))
    }
}
fn check_cancel(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() { Err(ToolError("Aborted: Cancelled".into())) } else { Ok(()) }
}

/// Native compatibility inputs belong to the Host. Used by the existing
/// WRITE/AST filesystem branches and the default Host edit resolver.
pub fn native_file_answers(state: &EditHostPathPolicy, authored: &str) -> HashMap<String, UrlResolution> {
    let policy = state.file_policy(Path::new("."));
    let Some(url) = policy.url_target(authored).map(|s| s.into_owned()) else { return HashMap::new() };
    let result = resolve_native_file(state, &url);
    let answer = match result {
        Ok(Some(answer)) => answer,
        Ok(None) => UrlResolution { absolute: None, error: None, plan_writable: false },
        Err(error) => UrlResolution { absolute: None, error: Some(error.0), plan_writable: false },
    };
    HashMap::from([(url, answer)])
}
pub fn resolve_native_file(state: &EditHostPathPolicy, url: &str) -> Result<Option<UrlResolution>, ToolError> {
    let Some((scheme, rest)) = url.split_once("://") else { return Ok(None) };
    let (root, relative, plan_writable) = if scheme.eq_ignore_ascii_case("local") {
        let root = state
            .local_sandbox_root
            .clone()
            .ok_or_else(|| ToolError("local:// is unavailable in this session".into()))?;
        let (host, path) = split_authority(rest)?;
        let relative = if host.is_empty() {
            path
        } else if path.is_empty() {
            host
        } else {
            format!("{host}/{path}")
        };
        (root, relative, true)
    } else if scheme.eq_ignore_ascii_case("vault") {
        let roots=state.vault_roots.as_ref().ok_or_else(||ToolError("vault:// is disabled. Enable it by setting `vault.enabled = true` (Settings → Tools → Obsidian Vault).".into()))?;
        let (host, path) = split_authority(rest)?;
        let key = if host.is_empty() { "_" } else { &host };
        let root = roots.iter().find(|(name, _)| name == key).map(|(_, root)| root.clone()).ok_or_else(|| {
            ToolError(
                "vault:// path resolution requires a cached vault root; read vault:// first or use the write tool"
                    .into(),
            )
        })?;
        (root, path, false)
    } else {
        return Ok(None);
    };
    crate::internal_urls::validate_relative_path(&relative)
        .map_err(|error| ToolError(error.replace("skill://", &format!("{scheme}://"))))?;
    let root = crate::normalize(&std::path::absolute(root).map_err(|error| ToolError(error.to_string()))?);
    let target = crate::normalize(&root.join(&relative));
    if !target.starts_with(&root) {
        return Err(ToolError(format!("{scheme}:// URL escapes its root")));
    }
    validate_containment(&target, &root)?;
    Ok(Some(UrlResolution { absolute: Some(target), error: None, plan_writable }))
}
/// Existing deepest ancestor is checked for create targets; no directory or
/// file is created by URL authorization itself.
pub fn validate_containment(target: &Path, root: &Path) -> Result<(), ToolError> {
    if !target.starts_with(root) {
        return Err(ToolError("URL backing path escapes its root".into()));
    }
    let physical_root = match std::fs::canonicalize(root) {
        Ok(root) => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => root.to_owned(),
        Err(error) => return Err(ToolError(error.to_string())),
    };
    let mut ancestor = target;
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(real) => {
                return if real.starts_with(&physical_root) {
                    Ok(())
                } else {
                    Err(ToolError("URL backing path escapes its root".into()))
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(ancestor) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(ToolError("URL backing path goes through a dangling symlink".into()));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(ToolError(error.to_string())),
                }
                if ancestor == root {
                    return Ok(());
                }
                ancestor =
                    ancestor.parent().ok_or_else(|| ToolError("URL backing path has no existing ancestor".into()))?;
            }
            Err(error) => return Err(ToolError(error.to_string())),
        }
    }
}
fn split_authority(rest: &str) -> Result<(String, String), ToolError> {
    let rest = rest.split(['?', '#']).next().unwrap_or(rest).replace('\\', "/");
    let (host, path) = rest.split_once('/').unwrap_or((&rest, ""));
    Ok((decode(host)?, decode(path)?))
}
fn decode(input: &str) -> Result<String, ToolError> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let value = bytes
                .get(i + 1..i + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| ToolError("Invalid URL encoding".into()))?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ToolError("Invalid URL encoding".into()))
}

/// OMP router.split / tui splitInternalUrlSel with the actual scheme's spec.
fn split_edit_target(ctx: &ToolContext, input: &str) -> (String, Option<String>) {
    let Some(scheme) = crate::internal_urls::hierarchical_scheme(input) else { return (input.into(), None) };
    let spec = ctx
        .content_uri_port()
        .and_then(|port| port.edit_specs().into_iter().find(|spec| spec.scheme.eq_ignore_ascii_case(scheme)));
    let lines = spec.as_ref().map_or_else(
        || ["local", "vault", "skill"].iter().any(|native| scheme.eq_ignore_ascii_case(native)),
        |spec| spec.line_selectors,
    );
    if !lines {
        return (input.into(), None);
    }
    let scheme_end = scheme.len() + 3;
    if spec.is_some_and(|spec| spec.port_authority) && !input[scheme_end..].contains('/') {
        return (input.into(), None);
    }
    static PART: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:raw|conflicts|img|L?[0-9]+(?:(?:[-+]|\.\.)L?[0-9]+|-|\.\.)?(?:,L?[0-9]+(?:(?:[-+]|\.\.)L?[0-9]+|-|\.\.)?)*|-[0-9]+(?:[-+][0-9]+)?)$").unwrap()
    });
    let mut path = input;
    let mut chunks = Vec::new();
    while let Some(colon) = path.rfind(':').filter(|index| *index >= scheme_end) {
        let tail = &path[colon + 1..];
        if !PART.is_match(tail) {
            break;
        }
        chunks.push(tail);
        path = &path[..colon];
    }
    if chunks.is_empty() {
        (input.into(), None)
    } else {
        chunks.reverse();
        (path.into(), Some(chunks.join(":")))
    }
}
