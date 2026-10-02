//! The fixed catalog's lenient classifyModel("", id) path, used only for shape choice.
//! Source: packages/catalog/src/compat/{taxonomy,revision,cascade}.ts (MIT).
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Default)]
pub(crate) struct Identity {
    pub class: String,
    pub family: Option<String>,
    pub revision: Option<[u8; 3]>,
}

fn rules() -> &'static Value {
    static RULES: OnceLock<Value> = OnceLock::new();
    RULES.get_or_init(|| serde_json::from_str(include_str!("catalog-rules.json")).expect("fixed catalog rules"))
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn bare(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}
fn bounded(value: &str, token: &str) -> bool {
    value == token
        || value
            .strip_prefix(token)
            .and_then(|tail| tail.chars().next())
            .is_some_and(|ch| matches!(ch, '-' | '_' | '.' | ':' | '0'..='9'))
}

fn glob(pattern: &str, subject: &str) -> bool {
    // Anchored '*' matching, with the same greedy/backtracking semantics as cascade.ts.
    let p = pattern.as_bytes();
    let s = subject.as_bytes();
    let (mut i, mut j, mut star, mut matched) = (0, 0, None, 0);
    while j < s.len() {
        if i < p.len() && p[i] == s[j] {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            matched = j;
        } else if let Some(at) = star {
            i = at + 1;
            matched += 1;
            j = matched;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

pub(crate) fn parse_revision(value: &str) -> Option<[u8; 3]> {
    let mut out = [0; 3];
    let mut count = 0;
    for part in value.split(['.', '-']) {
        if count == 3 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[count] = part.parse().ok()?;
        count += 1;
    }
    (count > 0).then_some(out)
}
fn parse_prefix(value: &str) -> Option<[u8; 3]> {
    let mut out = [0; 3];
    let mut count = 0;
    let mut at = 0;
    let bytes = value.as_bytes();
    while count < 3 {
        let start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        let component = if at < bytes.len() && bytes[at].is_ascii_alphabetic() {
            None
        } else {
            value[start..at].parse::<u8>().ok()
        };
        let Some(component) = component else {
            return (count > 0).then_some(out);
        };
        out[count] = component;
        count += 1;
        if at + 1 >= bytes.len() || !matches!(bytes[at], b'.' | b'-') || !bytes[at + 1].is_ascii_digit() {
            break;
        }
        at += 1;
    }
    Some(out)
}
fn family_rank(cls: &Value, subject: &str) -> (Option<String>, bool) {
    let mut winner: Option<((i64, usize), String)> = None;
    let mut tied = false;
    for family in array(&cls["families"]) {
        let pattern = string(&family["glob"]);
        if !glob(pattern, subject) {
            continue;
        }
        let rank = (family["priority"].as_i64().unwrap_or(0), pattern.encode_utf16().filter(|&c| c != 42).count());
        let id = string(&family["id"]);
        match &winner {
            Some((prior, previous)) if *prior == rank && previous != id => tied = true,
            None => {
                winner = Some((rank, id.into()));
                tied = false;
            }
            Some((prior, _)) if *prior < rank => {
                winner = Some((rank, id.into()));
                tied = false;
            }
            _ => {}
        }
    }
    (winner.map(|(_, id)| id), tied)
}
fn ranks_in_class(cls: &Value, model: &str) -> Identity {
    let lower = ara_prompt::js::trim(model).to_lowercase();
    let subject = bare(&lower);
    let (mut family, tied) = family_rank(cls, subject);
    if tied {
        family = None;
        if let Some(at) = subject.find(['.', ':']).filter(|&i| i > 0 && i + 1 < subject.len())
            && array(&cls["matchers"]).iter().any(|m| string(&m["token"]) == &subject[..at])
        {
            let (rescored, tied) = family_rank(cls, &subject[at + 1..]);
            if !tied {
                family = rescored;
            }
        }
    }
    let mut revision = None;
    if !array(&cls["skipBare"]).iter().any(|v| v.as_str() == Some(subject)) {
        for rule in array(&cls["revisionPrefixes"]) {
            let prefix = string(&rule["prefix"]);
            let tail = if rule["anywhere"] == true {
                subject.find(prefix).map(|at| &subject[at + prefix.len()..])
            } else {
                subject.strip_prefix(prefix)
            };
            if let Some(tail) = tail {
                revision = tail.find(|c: char| c.is_ascii_digit()).and_then(|at| parse_prefix(&tail[at..]));
                break;
            }
        }
    }
    Identity { class: string(&cls["id"]).into(), family, revision }
}
fn classify_ranks(model: &str) -> Identity {
    let lower = ara_prompt::js::trim(model).to_lowercase();
    let subject = bare(&lower);
    let mut winner: Option<((u8, usize), &Value)> = None;
    let mut tied = false;
    for cls in array(&rules()["taxonomy"]["classes"]) {
        for matcher in array(&cls["matchers"]) {
            let token = string(&matcher["token"]);
            let kind = string(&matcher["kind"]);
            let rank = match kind {
                "exact" => 4,
                "bounded" => 3,
                "namespace" => 2,
                "prefix" => 1,
                _ => 0,
            };
            let matches = match kind {
                "exact" => subject == token,
                "bounded" => bounded(subject, token),
                "prefix" => subject.starts_with(token),
                "namespace" if matcher["bounded"] == true => {
                    lower.split(['/', '.', ':']).any(|p| !p.is_empty() && bounded(p, token))
                }
                "namespace" => lower.split('/').any(|p| !p.is_empty() && p == token),
                "glob" => glob(token, subject),
                _ => false,
            };
            if !matches {
                continue;
            }
            let rank = (rank, token.encode_utf16().count());
            match winner {
                Some((prior, previous)) if prior == rank && previous["id"] != cls["id"] => tied = true,
                None => {
                    winner = Some((rank, cls));
                    tied = false;
                }
                Some((prior, _)) if prior < rank => {
                    winner = Some((rank, cls));
                    tied = false;
                }
                _ => {}
            }
        }
    }
    if tied {
        return Identity { class: "unknown".into(), ..Default::default() };
    }
    winner
        .map(|(_, cls)| ranks_in_class(cls, model))
        .unwrap_or_else(|| Identity { class: "unknown".into(), ..Default::default() })
}
pub(crate) fn classify(model: &str) -> Identity {
    let trimmed = ara_prompt::js::trim(model);
    let lower_bare = bare(trimmed).to_lowercase();
    let classes = array(&rules()["taxonomy"]["classes"]);
    let mut agnostic = None;
    for cls in classes {
        for item in array(&cls["overrides"]) {
            if string(&item["model"]).to_lowercase() != lower_bare {
                continue;
            }
            if let Some(provider) = item["provider"].as_str() {
                if provider.is_empty() {
                    return override_identity(item, trimmed);
                }
            } else if agnostic.is_none() {
                agnostic = Some(item);
            }
        }
    }
    if let Some(item) = agnostic {
        return override_identity(item, trimmed);
    }
    let lower = trimmed.to_lowercase();
    let subject = bare(&lower);
    let mut suffix = "";
    if trimmed.len() == model.len() {
        for item in array(&rules()["taxonomy"]["collapse"]["suffixes"]) {
            let token = string(&item["suffix"]);
            if lower.ends_with(token)
                && !item["exceptBarePrefix"].as_str().is_some_and(|p| subject.starts_with(p))
                && token.len() > suffix.len()
            {
                suffix = token;
            }
        }
    }
    classify_ranks(&trimmed[..trimmed.len() - suffix.len()])
}
fn override_identity(item: &Value, model: &str) -> Identity {
    let logical = item["logical"].as_str().unwrap_or(model);
    let class = item["class"].as_str().map(str::to_owned).unwrap_or_else(|| classify_ranks(logical).class);
    let mut identity = array(&rules()["taxonomy"]["classes"])
        .iter()
        .find(|c| c["id"].as_str() == Some(&class))
        .map(|cls| ranks_in_class(cls, logical))
        .unwrap_or_default();
    identity.class = class;
    if let Some(family) = item["family"].as_str() {
        identity.family = Some(family.into());
    }
    if let Some(revision) = item["revision"].as_str() {
        identity.revision = parse_revision(revision);
    }
    identity
}
