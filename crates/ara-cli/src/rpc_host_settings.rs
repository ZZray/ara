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

/// Fixed Task policy reads the existing global -> project YAML chain.
/// Identity/spawn policy, plan mode and keepAlive are explicit Host inputs;
/// they are not invented persisted settings.
pub(super) struct TaskSettings;
impl TaskSettings {
    pub(super) fn max_effort(cwd:&Path,agent_dir:&Path)->Result<ara_ai::ReasoningEffort>{
        let mut effective=Yaml::BadValue;
        for name in MAIN_CONFIG_FILENAMES{if let Some((document,_))=read_document(&agent_dir.join(name))?{merge_read_group(&mut effective,&document);break;}}
        if let Some((document,_))=read_document(&cwd.join(".ara/config.yml"))?{merge_read_group(&mut effective,&document);}
        match &effective["task"]["maxEffort"]{
            Yaml::BadValue=>Ok(ara_ai::ReasoningEffort::Max),
            Yaml::String(value)=>ara_ai::ReasoningEffort::parse(value).context("task.maxEffort must be a canonical effort"),
            _=>bail!("task.maxEffort must be a canonical effort"),
        }
    }
    pub(super) fn load(cwd:&Path,agent_dir:&Path)->Result<ara_cli::task_contract::TaskPolicy>{
        let mut effective=Yaml::BadValue;
        for name in MAIN_CONFIG_FILENAMES {
            if let Some((document,_))=read_document(&agent_dir.join(name))?{
                document.as_hash().context("Task settings root must be a mapping")?;
                merge_read_group(&mut effective,&document);break;
            }
        }
        if let Some((document,_))=read_document(&cwd.join(".ara/config.yml"))?{
            document.as_hash().context("Task settings root must be a mapping")?;
            merge_read_group(&mut effective,&document);
        }
        let task=&effective["task"];let isolation=&task["isolation"];
        let mut policy=ara_cli::task_contract::TaskPolicy::default();
        policy.isolation_enabled=read_setting_truth(&isolation["enabled"],false);
        policy.batch_enabled=read_setting_truth(&task["batch"],true);
        policy.effort_enabled=read_setting_truth(&task["enableEffort"],false);
        policy.structured=read_setting_truth(&task["structured"],false);
        policy.eval_tools_enabled=read_setting_truth(&effective["eval"]["tools"]["enabled"],true);
        policy.async_enabled=read_setting_truth(&effective["async"]["enabled"],true);
        policy.apply_changes=read_setting_truth(&isolation["apply"],true);
        policy.merge_mode=match &isolation["merge"] {Yaml::BadValue=>"patch".into(),Yaml::String(value) if matches!(value.as_str(),"patch"|"branch")=>value.clone(),_=>bail!("task.isolation.merge must be patch or branch")};
        policy.backend=match &effective["isolation"]["backend"] {
            Yaml::BadValue=>"auto".into(),
            Yaml::String(value) if matches!(value.as_str(),"auto"|"apfs"|"btrfs"|"zfs"|"reflink"|"overlayfs"|"projfs"|"block-clone"|"rcopy")=>value.clone(),
            _=>bail!("isolation.backend is not a fixed supported backend"),
        };
        let number=|key:&str,default:f64|->Result<f64>{match &task[key]{Yaml::BadValue=>Ok(default),value @ (Yaml::Integer(_)|Yaml::Real(_))=>read_setting_number(value,default,0.0),_=>bail!("task.{key} must be a number")}};
        policy.maximum_depth=number("maxRecursionDepth",2.0)?;
        policy.max_concurrency=number("maxConcurrency",32.0)?;
        policy.max_runtime_ms=number("maxRuntimeMs",0.0)?;
        Ok(policy)
    }
}

/// Fixed b91 irc/messaging.ts: a child retains peers even at the spawn cap.
/// Explicit enableIrc is a Host input, never a new YAML setting.
pub(super) fn peer_messaging_enabled(cwd:&Path,agent_dir:&Path,depth:usize,enable_irc:bool)->Result<bool>{
    if !enable_irc{return Ok(false);}
    if depth>0{return Ok(true);}
    let mut task=Yaml::BadValue;
    let mut documents=Vec::new();
    for name in MAIN_CONFIG_FILENAMES{if let Some((document,_))=read_document(&agent_dir.join(name))?{documents.push(document);break;}}
    if let Some((document,_))=read_document(&cwd.join(".ara/config.yml"))?{documents.push(document);}
    for document in documents{
        document.as_hash().context("Peer messaging settings root must be a mapping")?;
        merge_read_group(&mut task,&document["task"]);
    }
    let value=&task["maxRecursionDepth"];
    let maximum=match value{
        Yaml::BadValue=>2.0,
        Yaml::Integer(_)|Yaml::Real(_)=>read_setting_number(value,2.0,0.0)?,
        _=>bail!("task.maxRecursionDepth must be a number"),
    };
    Ok(enable_irc&&(depth>0||maximum<0.0||(depth as f64)<maximum))
}
pub(super) fn peer_task_depth(journal:Option<&ara_session::SessionJournal>)->Result<usize>{
    Ok(journal.map(|journal|journal.latest_child_init()).transpose()?.flatten()
        .and_then(|init|init.extra.get("araChildDepth").and_then(serde_json::Value::as_u64)).unwrap_or(0)as usize)
}

#[cfg(test)]
mod task_settings_observations {
    use super::*;
    #[test]
    fn task_nested_global_project_settings_and_host_only_fields() {
        let directory=tempfile::tempdir().unwrap();let global=directory.path().join("agent");let project=directory.path().join("project");
        std::fs::create_dir_all(&global).unwrap();std::fs::create_dir_all(project.join(".ara")).unwrap();
        std::fs::write(global.join("config.yml"),"task:\n  batch: false\n  enableEffort: true\n  maxRecursionDepth: 3\n  isolation:\n    enabled: true\n    merge: branch\n    apply: false\nisolation:\n  backend: rcopy\nasync:\n  enabled: false\n").unwrap();
        std::fs::write(project.join(".ara/config.yml"),"task:\n  structured: true\n  isolation:\n    merge: patch\n  keepAlive: false\neval:\n  tools:\n    enabled: false\n").unwrap();
        let policy=TaskSettings::load(&project,&global).unwrap();
        assert!(!policy.batch_enabled);assert!(policy.effort_enabled);assert!(policy.structured);assert!(policy.isolation_enabled);
        assert_eq!(policy.merge_mode,"patch");assert_eq!(policy.backend,"rcopy");assert!(!policy.apply_changes);assert!(!policy.async_enabled);assert!(!policy.eval_tools_enabled);
        assert_eq!(policy.maximum_depth,3.0);assert_eq!(policy.max_concurrency,32.0);
        assert!(policy.keep_alive,"keepAlive belongs to the Host execution contract, not an invented setting");
        std::fs::write(project.join(".ara/config.yml"),"task:\n  isolation:\n    merge: invalid\n").unwrap();
        assert!(TaskSettings::load(&project,&global).is_err());
    }
}

/// Match the already selected branch which SessionJournal::fork_at will retain.
/// Root-user branching retains no session_init; it must not inherit child depth.
pub(super) fn peer_branch_task_depth(journal:&ara_session::SessionJournal,leaf:Option<&str>)->usize{
    let by_id: std::collections::HashMap<_, _> = journal.entries().iter()
        .map(|entry| (entry.id.as_str(), entry)).collect();
    let mut current = leaf;
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) { break; }
        let Some(entry) = by_id.get(id) else { break; };
        if entry.kind == "session_init" {
            return entry.raw.get("araChildDepth")
                .and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
        }
        current = entry.parent_id.as_deref();
    }
    0
}

/// Captured READ policy at the existing Host global -> project YAML owner.
/// Fixed settings.get resolves nested paths only. Mappings merge recursively;
/// authored null/scalars replace a group, and missing leaves use schema defaults.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ReadSettings {
    pub output_max_columns: f64,
    pub default_limit: f64,
    pub render_markdown: bool,
    pub auto_resize_images: bool,
    pub screenshot_dir: Option<String>,
    pub summary: ara_tools::read_summary::SummaryPolicy,
}
impl ReadSettings {
    pub fn apply(&self, context: ara_tools::ToolContext) -> ara_tools::ToolContext {
        context
            .with_read_output_max_columns(self.output_max_columns)
            .with_read_default_limit(self.default_limit)
            .with_read_render_markdown(self.render_markdown)
            .with_read_auto_resize_images(self.auto_resize_images)
            .with_read_summary_policy(self.summary)
    }
    pub fn matches(&self, context: &ara_tools::ToolContext) -> bool {
        // Numeric NaN is an authored snapshot value, not perpetual dirtiness.
        let same = |a: f64, b: f64| a == b || a.is_nan() && b.is_nan();
        let summary = context.read_summary_policy();
        let same_option = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => same(a, b),
            _ => false,
        };
        same(self.output_max_columns, context.read_output_max_columns())
            && same(self.clamped_limit(), context.read_default_limit())
            && self.render_markdown == context.read_render_markdown()
            && self.auto_resize_images == context.read_auto_resize_images()
            && self.summary.native_options_valid == summary.native_options_valid
            && self.summary.enabled == summary.enabled
            && self.summary.prose == summary.prose
            && same(self.summary.min_total_lines, summary.min_total_lines)
            && same_option(self.summary.min_body_lines, summary.min_body_lines)
            && same_option(self.summary.min_comment_lines, summary.min_comment_lines)
            && same_option(self.summary.unfold_until, summary.unfold_until)
            && same_option(self.summary.unfold_limit, summary.unfold_limit)
    }
    fn clamped_limit(&self) -> f64 {
        if self.default_limit.is_nan() {
            f64::NAN
        } else {
            self.default_limit
                .clamp(1.0, ara_tools::DEFAULT_MAX_LINES as f64)
        }
    }
}
fn merge_read_group(base: &mut Yaml, incoming: &Yaml) {
    if matches!(incoming, Yaml::BadValue) {
        return;
    }
    if let (Yaml::Hash(previous), Yaml::Hash(next)) = (&mut *base, incoming) {
        for (key, value) in next {
            if let Some(current) = previous.get_mut(key) {
                merge_read_group(current, value)
            } else {
                previous.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = incoming.clone();
    }
}
fn read_setting_truth(value: &Yaml, default: bool) -> bool {
    match value {
        Yaml::BadValue => default,
        Yaml::Null => false,
        Yaml::Boolean(v) => *v,
        Yaml::Integer(v) => *v != 0,
        Yaml::Real(v) => v.parse::<f64>().is_ok_and(|v| v != 0.0 && !v.is_nan()),
        Yaml::String(v) => !v.is_empty(),
        _ => true,
    }
}
fn read_setting_number(value: &Yaml, missing: f64, null: f64) -> Result<f64> {
    Ok(match value {
        Yaml::BadValue => missing,
        Yaml::Null => null,
        Yaml::Integer(v) => *v as f64,
        Yaml::Real(v) => match v.as_str() {
            ".nan" | ".NaN" | ".NAN" => f64::NAN,
            ".inf" | ".Inf" | ".INF" => f64::INFINITY,
            "-.inf" | "-.Inf" | "-.INF" => f64::NEG_INFINITY,
            _ => v.parse().context("READ setting must be numeric")?,
        },
        // Constructor/comparison coerce authored boolean/string using JS Number;
        // NAPI folds below require actual numbers instead.
        Yaml::Boolean(v) => {
            if *v {
                1.0
            } else {
                0.0
            }
        }
        Yaml::String(v) => {
            let v = v.trim_matches(ara_prompt::js::is_space);
            if v.is_empty() {
                0.0
            } else {
                v.parse().unwrap_or(f64::NAN)
            }
        }
        _ => bail!("READ setting must be numeric"),
    })
}
fn read_native_fold(value: &Yaml, default: f64) -> Result<Option<f64>> {
    match value {
        Yaml::Null => Ok(None),
        Yaml::BadValue => Ok(Some(default)),
        Yaml::Integer(_) | Yaml::Real(_) => read_setting_number(value, default, 0.0).map(Some),
        // Fixed napi object conversion throws here, but trySummarize catches
        // it AFTER source acquisition and falls back to a second ACP read.
        _ => Ok(None),
    }
}
pub(super) fn read_settings(cwd:&Path,agent_dir:&Path)->Result<ReadSettings>{
    let mut read =Yaml::BadValue;
    let mut tools = Yaml::BadValue;
    let mut images = Yaml::BadValue;
    let mut browser = Yaml::BadValue;
    let mut dotted_columns = Yaml::BadValue;let mut documents=Vec::new();
    for name in MAIN_CONFIG_FILENAMES {
        if let Some((document,_))=read_document(&agent_dir.join(name))? {documents.push(document);break;}
    }
    if let Some((document,_))=read_document(&cwd.join(".ara/config.yml"))?{documents.push(document);}
    for document in documents {
        document.as_hash().context("READ settings root must be a mapping")?;
        merge_read_group(&mut read, &document["read"]);
        merge_read_group(&mut tools, &document["tools"]);
        merge_read_group(&mut images, &document["images"]);
        merge_read_group(&mut browser, &document["browser"]);
        let incoming = &document["tools.outputMaxColumns"];
        if !matches!(incoming, Yaml::BadValue) {
            dotted_columns = incoming.clone();
        } else if !matches!(&document["tools"]["outputMaxColumns"], Yaml::BadValue) {
            // Preserve the old Host per-document dotted/nested precedence.
            dotted_columns = document["tools"]["outputMaxColumns"].clone();
        }
    }
    let defaults = ara_tools::read_summary::SummaryPolicy::default();
    let summarize = &read["summarize"];
    Ok(ReadSettings {
        // Existing output-columns Host contract retains its per-document
        // dotted/nested precedence and validation; new READ leaves use fixed
        // nested resolution. No new owner changes this existing column policy.
        output_max_columns: if matches!(dotted_columns,Yaml::BadValue) {768.0} else {
            dotted_columns.as_f64().or_else(||dotted_columns.as_i64().map(|v|v as f64))
                .context("tools.outputMaxColumns must be a number")?
        },
        default_limit: read_setting_number(
            &read["defaultLimit"],
            300.0,
            ara_tools::DEFAULT_MAX_LINES as f64,
        )?,
        render_markdown: read_setting_truth(&read["renderMarkdown"], false),
        auto_resize_images: read_setting_truth(&images["autoResize"], true),
        screenshot_dir: match &browser["screenshotDir"] {
            Yaml::BadValue | Yaml::Null => None,
            Yaml::String(value) if value.is_empty() => None,
            Yaml::String(value) => Some(value.clone()),
            _ => bail!("browser.screenshotDir must be a string"),
        },
        summary: ara_tools::read_summary::SummaryPolicy {
            native_options_valid: ["minBodyLines","minCommentLines","unfoldUntil","unfoldLimit"].iter()
                .all(|name|matches!(summarize[*name],Yaml::BadValue|Yaml::Null|Yaml::Integer(_)|Yaml::Real(_))),
            enabled: read_setting_truth(&summarize["enabled"], true),
            prose: read_setting_truth(&summarize["prose"], false),
            min_total_lines: read_setting_number(&summarize["minTotalLines"], 100.0, 0.0)?,
            min_body_lines: read_native_fold(
                &summarize["minBodyLines"],
                defaults.min_body_lines.unwrap(),
            )?,
            min_comment_lines: read_native_fold(
                &summarize["minCommentLines"],
                defaults.min_comment_lines.unwrap(),
            )?,
            unfold_until: read_native_fold(
                &summarize["unfoldUntil"],
                defaults.unfold_until.unwrap(),
            )?,
            unfold_limit: read_native_fold(
                &summarize["unfoldLimit"],
                defaults.unfold_limit.unwrap(),
            )?,
        },
    })
}
// Retain the existing callable owner for focused output-columns checks.
pub(super) fn read_output_max_columns(cwd: &Path, agent_dir: &Path) -> Result<f64> {
    Ok(read_settings(cwd, agent_dir)?.output_max_columns)
}

#[cfg(test)]
#[test]
fn read_settings_source_snapshot_grouped() {
    use ara_tools::read_summary::SummaryPolicy;
    let directory = tempfile::tempdir().unwrap();
    let global = directory.path().join("agent");
    let cwd = directory.path().join("project");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(cwd.join(".ara")).unwrap();
    let missing = read_settings(&cwd, &global).unwrap();
    assert_eq!(missing.default_limit, 300.0);
    assert_eq!(missing.summary, SummaryPolicy::default());
    let global_path = global.join("config.yml");
    let project = cwd.join(".ara/config.yml");
    std::fs::write(&global_path,"read: {defaultLimit: 2.5, renderMarkdown: true, summarize: {prose: true, minBodyLines: 4.9, unfoldUntil: 0}}\ntools: {outputMaxColumns: 120}\n").unwrap();
    for (text, limit, markdown, prose, body) in [
        (
            "read: {summarize: {minBodyLines: null}}\n",
            2.5,
            true,
            true,
            None,
        ),
        (
            "read: {defaultLimit: null}\n",
            3000.0,
            true,
            true,
            Some(4.9),
        ),
        ("read: null\n", 300.0, false, false, Some(4.0)),
        ("read: replaced\n", 300.0, false, false, Some(4.0)),
        ("read: {summarize: null}\n", 2.5, true, false, Some(4.0)),
        (
            "read: {summarize: {enabled: null, minTotalLines: null}}\n",
            2.5,
            true,
            true,
            Some(4.9),
        ),
    ] {
        std::fs::write(&project, text).unwrap();
        let candidate = read_settings(&cwd, &global).unwrap();
        assert_eq!(candidate.default_limit, limit, "{text}");
        assert_eq!(candidate.render_markdown, markdown);
        assert_eq!(candidate.summary.prose, prose);
        assert_eq!(candidate.summary.min_body_lines, body);
        let admitted = missing.apply(ara_tools::ToolContext::new(&cwd));
        let newer = candidate.apply(admitted.clone());
        assert!(missing.matches(&admitted));
        assert!(candidate.matches(&newer));
        assert_eq!(admitted.read_default_limit(), 300.0);
        assert!(!admitted.read_render_markdown());
    }
    std::fs::write(
        &project,
        "read: {defaultLimit: .nan, summarize: {minBodyLines: .nan}}\n",
    )
    .unwrap();
    let nan = read_settings(&cwd, &global).unwrap();
    let context = nan.apply(ara_tools::ToolContext::new(&cwd));
        assert!(context.read_default_limit().is_nan());
    assert!(nan.matches(&context));
    // The fixed resolver ignores unmigrated quoted dotted READ leaves.
    std::fs::write(
        &project,
        "read.defaultLimit: 17\nread.summarize.enabled: false\n",
    )
    .unwrap();
    assert_eq!(read_settings(&cwd, &global).unwrap().default_limit, 2.5);}

/// Fixed AutoQA settings snapshot with the existing global atomic writer.
/// Configured-vs-default and caller persisted consent stay observable; a
/// cached process grant does not rewrite a stale child snapshot.
pub(super) struct AutoQaPolicy {
    path:PathBuf,
    config:std::sync::Mutex<ara_cli::report_tool_issue::AutoQaConfig>,
}
impl AutoQaPolicy {
    pub(super) fn load(cwd:&Path,agent_dir:&Path)->Result<std::sync::Arc<Self>>{
        use ara_cli::report_tool_issue::{AutoQaConfig,Consent};
        let mut config=AutoQaConfig::default();let mut path=agent_dir.join("config.yml");let mut documents=Vec::new();
        for name in MAIN_CONFIG_FILENAMES{let candidate=agent_dir.join(name);if let Some((document,_))=read_document(&candidate)?{path=candidate;documents.push(document);break}}
        if let Some((document,_))=read_document(&cwd.join(".ara/config.yml"))?{documents.push(document)}
        for document in documents{
            let root=document.as_hash().context("autoqa settings root must be a mapping")?;
            let value=|key:&str|->Option<&Yaml>{
                root.get(&Yaml::String(format!("dev.{key}"))).or_else(||{
                    let dev=root.get(&Yaml::String("dev".into()))?.as_hash()?;
                    if let Some(value)=dev.get(&Yaml::String(key.into())){return Some(value)}
                    let (group,child)=key.split_once('.')?;
                    dev.get(&Yaml::String(group.into()))?.as_hash()?.get(&Yaml::String(child.into()))
                })
            };
            if let Some(v)=value("autoqa"){
                config.explicitly_configured=true;
                config.enabled=match v{Yaml::Null|Yaml::BadValue=>false,Yaml::Boolean(v)=>*v,Yaml::Integer(v)=>*v!=0,Yaml::Real(_)=>v.as_f64().is_some_and(|v|v!=0.0&&!v.is_nan()),Yaml::String(v)=>!v.is_empty(),_=>true};
            }
            if let Some(v)=value("autoqaConsent"){config.consent=match v.as_str(){Some("granted")=>Consent::Granted,Some("denied")=>Consent::Denied,_=>Consent::Unset};}
            if let Some(v)=value("autoqaPush.endpoint"){config.endpoint=v.as_str().map(str::to_owned);}
            if let Some(v)=value("autoqaPush.token"){config.token=v.as_str().map(str::to_owned);}
        }
        Ok(std::sync::Arc::new(Self{path,config:std::sync::Mutex::new(config)}))
    }
}
impl ara_cli::report_tool_issue::AutoQaSettings for AutoQaPolicy{
    fn snapshot(&self)->ara_cli::report_tool_issue::AutoQaConfig{self.config.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    fn set_consent(&self,consent:ara_cli::report_tool_issue::Consent)->std::result::Result<(),String>{
        use ara_cli::report_tool_issue::Consent;
        let value=match consent{Consent::Granted=>"granted",Consent::Denied=>"denied",Consent::Unset=>"unset"};
        let save=||->Result<()>{
            let loaded=read_document(&self.path)?;
            let mut document=loaded.as_ref().map(|(doc,_)|doc.clone()).unwrap_or_else(empty_mapping);
            let root=document.as_mut_hash().context("autoqa settings root must be a mapping")?;
            // Existing dotted keys remain authoritative to the shared reader.
            if root.contains_key(&Yaml::String("dev.autoqaConsent".into())){root.insert(Yaml::String("dev.autoqaConsent".into()),Yaml::String(value.into()));}
            else{
                let dev=root.entry(Yaml::String("dev".into())).or_insert_with(empty_mapping);
                if matches!(dev,Yaml::Null){*dev=empty_mapping();}
                dev.as_mut_hash().context("dev settings must be a mapping")?.insert(Yaml::String("autoqaConsent".into()),Yaml::String(value.into()));
            }
            let mut encoded=String::new();YamlEmitter::new(&mut encoded).dump(&document)?;encoded.push('\n');
            write_atomically(&self.path,&encoded,loaded.as_ref().map(|(_,s)|s.as_str()),"autoqa")
        };
        // Settings.set source mutates the local snapshot even if its best-effort
        // persistence throws; process cache is also retained by the service.
        self.config.lock().unwrap_or_else(|e|e.into_inner()).consent=consent;
        save().map_err(|e|format!("{e:#}"))
    }
}

pub(super) fn autoqa_environment()->ara_cli::report_tool_issue::AutoQaEnvironment{
    fn flag(name:&str)->Option<bool>{std::env::var(name).ok().filter(|s|!s.is_empty()).map(|s|matches!(s.as_str(),"1"|"Y"|"y"|"TRUE"|"true"|"YES"|"yes"|"ON"|"on"))}
    fn value(name:&str)->Option<String>{std::env::var(name).ok().map(|s|s.trim_matches(ara_prompt::js::is_space).to_owned()).filter(|s|!s.is_empty())}
    ara_cli::report_tool_issue::AutoQaEnvironment{enabled:flag("ARA_AUTO_QA"),push:flag("ARA_AUTO_QA_PUSH").unwrap_or(false),endpoint:value("ARA_AUTO_QA_PUSH_URL"),token:value("ARA_AUTO_QA_PUSH_TOKEN")}
}

#[cfg(test)]
#[test]
fn report_issue_adapter_grouped(){
    use ara_cli::report_tool_issue::{AutoQaSettings,Consent,DEFAULT_ENDPOINT};
    let dir=tempfile::tempdir().unwrap();let global=dir.path().join("agent");let project=dir.path().join("project");std::fs::create_dir_all(&global).unwrap();std::fs::create_dir_all(project.join(".ara")).unwrap();
    let policy=AutoQaPolicy::load(&project,&global).unwrap();assert!(policy.snapshot().enabled);assert!(!policy.snapshot().explicitly_configured);assert_eq!(policy.snapshot().consent,Consent::Unset);assert_eq!(policy.snapshot().endpoint.as_deref(),Some(DEFAULT_ENDPOINT));assert!(!global.join("config.yml").exists());
    let file=global.join("config.yaml");std::fs::write(&file,"custom: keep\ndev:\n  autoqaConsent: denied\n  autoqaPush:\n    endpoint: http://127.0.0.1/fixture\n    token: fixture-token\n").unwrap();std::fs::write(project.join(".ara/config.yml"),"dev.autoqa: true\n").unwrap();let policy=AutoQaPolicy::load(&project,&global).unwrap();let config=policy.snapshot();assert!(config.enabled&&config.explicitly_configured);assert_eq!(config.consent,Consent::Denied);assert_eq!(config.endpoint.as_deref(),Some("http://127.0.0.1/fixture"));assert_eq!(config.token.as_deref(),Some("fixture-token"));
    std::fs::write(&file,"custom: newer\ndev:\n  autoqaConsent: denied\n  autoqaPush:\n    endpoint: http://127.0.0.1/fixture\n").unwrap();policy.set_consent(Consent::Granted).unwrap();let doc=read_document(&file).unwrap().unwrap().0;assert_eq!(doc["custom"].as_str(),Some("newer"));assert_eq!(doc["dev"]["autoqaConsent"].as_str(),Some("granted"));assert_eq!(policy.snapshot().consent,Consent::Granted);assert!(!global.join("config.yml").exists());
    println!("report-issue-host-settings:pass:defaults-explicit-global-project-consent-atomic-preserve");
}

/// Fixed tools/index.ts factory gates and settings-schema.ts defaults.
/// Native Host global -> project snapshot; unrestricted CLI explicit lists
/// auto-include AST counterparts only while their source setting is enabled.
pub(super) struct DefaultToolSupplierSettings {
    pub ast_grep: bool,
    pub ast_edit: bool,
    pub debug: bool,
    pub web_search: bool,
}
impl DefaultToolSupplierSettings {
    pub fn select<'a>(&self, names: &mut Vec<&'a str>, explicit: bool) {
        if explicit {
            if self.ast_grep && names.contains(&"grep") && !names.contains(&"ast_grep") { names.push("ast_grep"); }
            if self.ast_edit && names.contains(&"edit") && !names.contains(&"ast_edit") { names.push("ast_edit"); }
        }
        names.retain(|name| match *name {
            "ast_grep" => self.ast_grep, "ast_edit" => self.ast_edit,
            "debug" => self.debug, "web_search" => self.web_search, _ => true,
        });
    }
}
pub(super) fn default_tool_supplier_settings(cwd: &Path, agent_dir: &Path) -> Result<DefaultToolSupplierSettings> {
    let mut settings = DefaultToolSupplierSettings { ast_grep:false, ast_edit:true, debug:true, web_search:true };
    let mut documents = Vec::new();
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? { documents.push(document); break; }
    }
    if let Some((document, _)) = read_document(&cwd.join(".ara").join("config.yml"))? { documents.push(document); }
    for document in documents {
        let root = document.as_hash().context("native default tool settings root must be a mapping")?;
        for (group, enabled) in [("astGrep", &mut settings.ast_grep), ("astEdit", &mut settings.ast_edit), ("debug", &mut settings.debug), ("web_search", &mut settings.web_search)] {
            if let Some(value) = root.get(&Yaml::String(format!("{group}.enabled"))).or_else(|| root.get(&Yaml::String(group.into()))?.as_hash()?.get(&Yaml::String("enabled".into()))) {
                *enabled = match value {
                    Yaml::BadValue | Yaml::Null => false, Yaml::Boolean(value) => *value,
                    Yaml::Integer(value) => *value != 0, Yaml::Real(_) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
                    Yaml::String(value) => !value.is_empty(), _ => true,
                };
            }
        }
    }
    Ok(settings)
}

/// Fixed tools.xdev, xdevDocs, xdevInlineDevices and approval settings.
/// Host reads the same global -> project document chain as LSP and vault.
pub(super) struct XdevSettings {
    pub enabled:bool,
    pub docs_mode:String,
    pub inline_patterns:Vec<String>,
    pub approval_mode:String,
    pub approval_policies:ara_rpc::WireValue,
    pub bash_patterns:ara_rpc::WireValue,
}
pub(super) fn xdev_settings(cwd:&Path,agent_dir:&Path)->Result<XdevSettings>{
    let mut documents=Vec::new();
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document,_))=read_document(&agent_dir.join(filename))?{documents.push(document);break}
    }
    if let Some((document,_))=read_document(&cwd.join(".ara").join("config.yml"))?{documents.push(document)}
    let mut settings=XdevSettings{enabled:true,docs_mode:"builtins".into(),inline_patterns:Vec::new(),approval_mode:"yolo".into(),approval_policies:ara_rpc::WireValue::object(Vec::new()),bash_patterns:ara_rpc::WireValue::Array(Vec::new())};
    for document in documents {
        let root=document.as_hash().context("native xdev settings root must be a mapping")?;
        let value=|name:&str|->Option<&Yaml>{root.get(&Yaml::String(format!("tools.{name}"))).or_else(||root.get(&Yaml::String("tools".into()))?.as_hash()?.get(&Yaml::String(name.into())))};
        if let Some(value)=value("xdev") {settings.enabled=match value {
            Yaml::BadValue|Yaml::Null=>false,Yaml::Boolean(value)=>*value,Yaml::Integer(value)=>*value!=0,
            Yaml::Real(_)=>value.as_f64().is_some_and(|value|value!=0.0&&!value.is_nan()),Yaml::String(value)=>!value.is_empty(),_=>true,
        }}
        if let Some(value)=value("xdevDocs"){
            let raw=match value {Yaml::String(value)=>ara_rpc::WireValue::String(value.as_str().into()),_=>ara_rpc::WireValue::Null};
            settings.docs_mode=ara_cli::actual_tools::xdev_docs::normalize_docs_mode(&raw).to_utf8().unwrap_or_default();
        }
        if let Some(value)=value("xdevInlineDevices"){
            // Original compileInlineGlobs ignores nonstrings without inspecting
            // their descendants. Never recursively convert ignored YAML data.
            let raw=match value {
                Yaml::Array(values)=>ara_rpc::WireValue::Array(values.iter().map(|value|match value{Yaml::String(value)=>ara_rpc::WireValue::String(value.as_str().into()),_=>ara_rpc::WireValue::Null}).collect()),
                _=>ara_rpc::WireValue::Null,
            };
            settings.inline_patterns=ara_cli::actual_tools::xdev_docs::normalize_inline_patterns(&raw).into_iter().filter_map(|value|value.to_utf8().ok()).collect();
        }
        if let Some(value)=value("approvalMode"){settings.approval_mode=value.as_str().unwrap_or("").into()}
        if let Some(value)=root.get(&Yaml::String("bash.patterns".into())).or_else(||root.get(&Yaml::String("bash".into()))?.as_hash()?.get(&Yaml::String("patterns".into()))){
            settings.bash_patterns=ara_rpc::WireValue::parse(&serde_json::to_string(&retry_fallback_json(value)?)?)?;
        }
        if let Some(value)=value("approval"){
            // Retain malformed policy values for the fixed resolver to normalize.
            settings.approval_policies=ara_rpc::WireValue::parse(&serde_json::to_string(&retry_fallback_json(value)?)?)?;
        }
    }
    Ok(settings)
}

/// Fixed vault.enabled defaults false. Use the existing native configuration
/// locations and the same global → project snapshot boundary as LSP/plan.
pub(super) fn vault_enabled(cwd: &Path, agent_dir: &Path) -> Result<bool> {
    let mut configured = None;
    let mut documents = Vec::new();
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            documents.push(document);
            break;
        }
    }
    if let Some((document, _)) = read_document(&cwd.join(".ara").join("config.yml"))? {
        documents.push(document);
    }
    for document in documents {
        let root = document.as_hash().context("native vault settings root must be a mapping")?;
        if let Some(value) = root.get(&Yaml::String("vault.enabled".into())) {
            configured = Some(value.clone());
            continue;
        }
        match &document["vault"] {
            Yaml::BadValue => {}
            group @ Yaml::Hash(_) => {
                if !matches!(&group["enabled"], Yaml::BadValue) {
                    configured = Some(group["enabled"].clone());
                }
            }
            _ => configured = None,
        }
    }
    Ok(match configured {
        None | Some(Yaml::BadValue | Yaml::Null) => false,
        Some(Yaml::Boolean(value)) => value,
        Some(Yaml::String(value)) => !value.is_empty(),
        Some(Yaml::Integer(value)) => value != 0,
        Some(value @ Yaml::Real(_)) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
        Some(_) => true,
    })
}

/// Narrow production LSP settings snapshot. Fixed Settings.get reads nested
/// leaves, global then native project. Full foreign Settings-capability and
/// runtime overlay owners remain separate from the LSP server config loader.
pub(super) fn lsp_settings(cwd: &Path, agent_dir: &Path) -> Result<ara_cli::host_lsp::LspSettings> {
    let mut values = ara_cli::host_lsp::LspSettings::default();
    let mut documents = Vec::new();
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            documents.push(document);
            break;
        }
    }
    if let Some((document, _)) = read_document(&cwd.join(".ara").join("config.yml"))? {
        documents.push(document);
    }
    for document in documents {
        document.as_hash().context("native LSP settings root must be a mapping")?;
        let setting = |name: &str, default: bool| match &document["lsp"][name] {
            Yaml::BadValue => default,
            Yaml::Null => false,
            Yaml::Boolean(value) => *value,
            Yaml::String(value) => !value.is_empty(),
            Yaml::Integer(value) => *value != 0,
            value @ Yaml::Real(_) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
            _ => true,
        };
        values.enabled = setting("enabled", values.enabled);
        values.shared = setting("shared", values.shared);
        values.lazy = setting("lazy", values.lazy);
        values.format_on_write = setting("formatOnWrite", values.format_on_write);
        values.diagnostics_on_edit = setting("diagnosticsOnEdit", values.diagnostics_on_edit);
        values.diagnostics_on_write = setting("diagnosticsOnWrite", values.diagnostics_on_write);
        values.diagnostics_deduplicate = setting("diagnosticsDeduplicate", values.diagnostics_deduplicate);
    }
    Ok(values)
}

#[derive(Clone, Debug, Default)]
pub(super) struct EditPostApplySettings {
    pub blackbox_enabled: bool,
    pub auto_repair_enabled: bool,
    pub roles: serde_json::Map<String, serde_json::Value>,
    pub provider_order: Vec<String>,
}
impl EditPostApplySettings {
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
        // Same Settings.getByPath nested leaves as the fixed tool adapter.
        let switch = |group: &str| -> Result<bool> {
            match document["edit"][group]["enabled"] {
                Yaml::BadValue | Yaml::Null => Ok(false),
                Yaml::Boolean(value) => Ok(value),
                _ => bail!("edit.{group}.enabled must be a boolean"),
            }
        };
        Ok(Self {
            blackbox_enabled: switch("blackbox")?,
            auto_repair_enabled: switch("autoRepair")?,
            roles: root
                .get(&Yaml::String("modelRoles".into()))
                .map(retry_fallback_json)
                .transpose()?
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default(),
            provider_order: root
                .get(&Yaml::String("modelProviderOrder".into()))
                .and_then(Yaml::as_vec)
                .map(|v| v.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
        })
    }
}

pub(super) fn enabled_model_patterns(agent_dir: &Path) -> Result<Vec<String>> {
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            let Some(values) = document["enabledModels"].as_vec() else { return Ok(Vec::new()) };
            return values
                .iter()
                .map(|value| value.as_str().map(str::to_owned).context("enabledModels must contain strings"))
                .collect();
        }
    }
    Ok(Vec::new())
}

/// Fixed sdk.ts/settings-schema.ts: immediate by default; Host owns persistence.
pub(super) fn load_interrupt_mode(agent_dir: &Path) -> Result<ara_agent::InterruptMode> {
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            return interrupt_mode_from_document(&document);
        }
    }
    Ok(ara_agent::InterruptMode::Immediate)
}

fn interrupt_mode_from_document(document: &Yaml) -> Result<ara_agent::InterruptMode> {
    match &document["interruptMode"] {
        Yaml::BadValue | Yaml::Null => Ok(ara_agent::InterruptMode::Immediate),
        Yaml::String(value) if value == "immediate" => Ok(ara_agent::InterruptMode::Immediate),
        Yaml::String(value) if value == "wait" => Ok(ara_agent::InterruptMode::Wait),
        _ => bail!("interruptMode must be immediate or wait"),
    }
}

pub(super) fn store_interrupt_mode(agent_dir: &Path, mode: ara_agent::InterruptMode) -> Result<()> {
    let mut selected = None;
    for filename in MAIN_CONFIG_FILENAMES {
        let path = agent_dir.join(filename);
        if let Some(loaded) = read_document(&path)? {
            selected = Some((path, Some(loaded)));
            break;
        }
    }
    let (path, loaded) = selected.unwrap_or_else(|| (agent_dir.join(MAIN_CONFIG_FILENAMES[0]), None));
    let mut document = loaded.as_ref().map(|(document, _)| document.clone()).unwrap_or_else(empty_mapping);
    let value = match mode {
        ara_agent::InterruptMode::Immediate => "immediate",
        ara_agent::InterruptMode::Wait => "wait",
    };
    document
        .as_mut_hash()
        .context("native settings root must be a mapping")?
        .insert(Yaml::String("interruptMode".into()), Yaml::String(value.into()));
    let mut encoded = String::new();
    YamlEmitter::new(&mut encoded).dump(&document).context("serializing interrupt settings")?;
    encoded.push('\n');
    write_atomically(&path, &encoded, loaded.as_ref().map(|(_, source)| source.as_str()), "interrupt")
}

/// Fixed edit settings adapter. Native own-key order puts canonical array-index
/// strings first, ascending; other keys retain YAML/JS insertion order.
pub(super) fn edit_mode_settings(agent_dir: &Path) -> Result<super::edit_mode_policy::EditModeSettings> {
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            return edit_mode_settings_from_document(&document);
        }
    }
    Ok(Default::default())
}

fn edit_mode_settings_from_document(document: &Yaml) -> Result<super::edit_mode_policy::EditModeSettings> {
    let root = document.as_hash().context("native settings root must be a mapping")?;
    let setting = |name: &str| -> Option<&Yaml> {
        root.get(&Yaml::String(format!("edit.{name}")))
            .or_else(|| root.get(&Yaml::String("edit".into()))?.as_hash()?.get(&Yaml::String(name.into())))
    };
    let mode = setting("mode").map(retry_fallback_json).transpose()?;
    let mut model_variants = setting("modelVariants")
        .filter(|value| value.as_hash().is_some())
        .map(retry_fallback_json)
        .transpose()?
        .and_then(|value| value.as_object().cloned())
        .map(|values| values.into_iter().collect::<Vec<_>>())
        .unwrap_or_default();
    model_variants.sort_by_key(|(key, _)| {
        key.parse::<u32>().ok().filter(|index| *index != u32::MAX && index.to_string() == *key).unwrap_or(u32::MAX)
    });
    Ok(super::edit_mode_policy::EditModeSettings { mode, model_variants })
}

/// Fixed Settings.get/getByPath reads the nested edit group. Quoted dotted
/// disk keys are not runtime overrides and are ignored for these four leaves.
/// Preserve raw leaves: env overrides can bypass wrong-type/null values. The
/// Host calls this loader only while constructing an enabled Edit tool.
pub(super) fn edit_tool_settings(agent_dir: &Path) -> Result<super::edit_mode_policy::EditSettingsSource> {
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            return edit_tool_settings_from_document(&document);
        }
    }
    Ok(Default::default())
}

fn edit_tool_settings_from_document(document: &Yaml) -> Result<super::edit_mode_policy::EditSettingsSource> {
    let root = document.as_hash().context("native settings root must be a mapping")?;
    let group = root.get(&Yaml::String("edit".into())).and_then(Yaml::as_hash);
    let setting = |name: &str| -> Option<&Yaml> { group.and_then(|group| group.get(&Yaml::String(name.into()))) };
    Ok(super::edit_mode_policy::EditSettingsSource {
        allow_fuzzy: setting("fuzzyMatch").cloned(),
        fuzzy_threshold: setting("fuzzyThreshold").cloned(),
        enforce_seen_lines: setting("enforceSeenLines").cloned(),
        block_auto_generated: setting("blockAutoGenerated").cloned(),
    })
}

/// Fixed schema defaults to false; stream-guards.ts consumes native truthiness.
/// Read only the nested disk leaf, without touching fuzzy/threshold inputs.
pub(super) fn edit_streaming_abort(agent_dir: &Path) -> Result<bool> {
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            return Ok(match &document["edit"]["streamingAbort"] {
                Yaml::BadValue | Yaml::Null => false,
                Yaml::Boolean(value) => *value,
                Yaml::String(value) => !value.is_empty(),
                Yaml::Integer(value) => *value != 0,
                value @ Yaml::Real(_) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
                _ => true,
            });
        }
    }
    Ok(false)
}

/// Fixed native Auto defaults. Loading observes settings only; route/auth work
/// belongs to the bounded classification job.
#[derive(Clone, Debug)]
pub(super) struct AutoThinkingSettings {
    pub backend: String,
    pub allow_max: bool,
    pub ultrathink: bool,
    pub default_thinking: Option<ara_cli::retry_fallback::ConfiguredThinkingLevel>,
    pub roles: serde_json::Map<String, serde_json::Value>,
    pub provider_order: Vec<String>,
}

impl Default for AutoThinkingSettings {
    fn default() -> Self {
        Self {
            backend: "online".into(),
            allow_max: false,
            ultrathink: true,
            default_thinking: None,
            roles: serde_json::Map::new(),
            provider_order: Vec::new(),
        }
    }
}

impl AutoThinkingSettings {
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
        let setting = |group: &str, name: &str| -> Option<&Yaml> {
            root.get(&Yaml::String(format!("{group}.{name}")))
                .or_else(|| root.get(&Yaml::String(group.into()))?.as_hash()?.get(&Yaml::String(name.into())))
        };
        let switch = |value: Option<&Yaml>, key: &str, default: bool| -> Result<bool> {
            match value {
                None => Ok(default),
                Some(Yaml::Boolean(value)) => Ok(*value),
                Some(_) => bail!("{key} must be a boolean"),
            }
        };
        let backend = setting("providers", "autoThinkingModel")
            .map(|value| value.as_str().context("providers.autoThinkingModel must be a string").map(str::to_owned))
            .transpose()?
            .unwrap_or_else(|| "online".into());
        let ceiling = setting("providers", "autoThinkingMaxEffort")
            .map(|value| value.as_str().context("providers.autoThinkingMaxEffort must be xhigh or max"))
            .transpose()?
            .unwrap_or("xhigh");
        if !matches!(ceiling, "xhigh" | "max") {
            bail!("providers.autoThinkingMaxEffort must be xhigh or max");
        }
        Ok(Self {
            backend,
            allow_max: ceiling == "max",
            ultrathink: switch(setting("magicKeywords", "enabled"), "magicKeywords.enabled", true)?
                && switch(setting("magicKeywords", "ultrathink"), "magicKeywords.ultrathink", true)?,
            default_thinking: root
                .get(&Yaml::String("defaultThinkingLevel".into()))
                .and_then(Yaml::as_str)
                .and_then(ara_cli::session_thinking::parse_configured),
            roles: root
                .get(&Yaml::String("modelRoles".into()))
                .map(retry_fallback_json)
                .transpose()?
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default(),
            provider_order: root
                .get(&Yaml::String("modelProviderOrder".into()))
                .and_then(Yaml::as_vec)
                .map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
        })
    }
}

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

/// Resolve a child override against the CURRENT model window on every admission.
/// Fixed agent/compaction.ts resolveThresholdTokens; inherited reserve supplies prompt budget fallback.
pub(super) fn child_compaction_threshold(pair:&ara_session::ChildCompactionThreshold,window:usize,prompt_budget:f64)->usize {
    let window=window.max(1) as f64;
    let budget=if prompt_budget.is_finite(){prompt_budget.max(0.0)}else{window};
    if pair.threshold_tokens.is_finite()&&pair.threshold_tokens>0.0 {
        return if pair.threshold_tokens>=window {budget.floor()as usize}else{pair.threshold_tokens.clamp(1.0,(window-1.0).max(1.0)).floor()as usize};
    }
    if pair.threshold_percent.is_finite()&&pair.threshold_percent>0.0 {
        return (window*(pair.threshold_percent.clamp(1.0,99.0)/100.0)).floor()as usize;
    }
    budget.floor()as usize
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

/// Native reserve-policy equality checks in turn-recovery.ts:1637-1742.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UsageReservePolicy {
    Confirm,
    Auto,
    FailClosed,
    /// Native Settings keeps unrecognized values. They are neither exact
    /// `confirm` (which reuses approval), `auto`, nor `fail-closed`.
    Other,
}

/// Native retry policy for the Session-owned recovery saga. Fixed OMP
/// settings-schema.ts declares the limits as `number`, without integer or
/// nonnegative constraints. Keep those values as f64; the Host owns their
/// comparison/backoff semantics and checked conversion to a timer duration.
/// Fallback settings follow settings.ts:637-646 and the equality/truthiness
/// checks in turn-recovery.ts:1592-1742. Reading them does not select a route.
pub(super) struct RetryPolicy {
    path: Option<PathBuf>,
    enabled: bool,
    max_retries: f64,
    base_delay_ms: f64,
    max_delay_ms: f64,
    model_fallback: bool,
    usage_aware_fallback: bool,
    usage_reserve_pct: Yaml,
    reserve_policy: Yaml,
    fallback_chains: serde_json::Value,
    model_roles: serde_json::Map<String, serde_json::Value>,
    fallback_revert_policy: ara_cli::retry_fallback::RetryFallbackRevertPolicy,
}

impl RetryPolicy {
    /// Child role overlay is process memory only. Other Sessions and disk settings are unchanged.
    pub(super) fn with_child_fallback(mut self, id:&str, role:Option<&ara_session::ChildRetryFallback>)->Self {
        if let Some(role)=role {
            let name=format!("subagent:{id}");
            self.model_roles.insert(name.clone(),serde_json::Value::String(role.primary.clone()));
            let mut chains=serde_json::Map::new();
            chains.insert(name.clone(),serde_json::json!(role.chain));
            if let Some(inherited)=self.fallback_chains.as_object(){for (key,value) in inherited{if key!=&name{chains.insert(key.clone(),value.clone());}}}
            self.fallback_chains=serde_json::Value::Object(chains);
        }
        self.path=None;
        self
    }

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
            model_fallback: read_retry_switch(group, "modelFallback", true)?,
            usage_aware_fallback: read_retry_switch(group, "usageAwareFallback", false)?,
            usage_reserve_pct: group
                .and_then(|group| group.get(&Yaml::String("usageReservePct".into())))
                .cloned()
                .unwrap_or(Yaml::Integer(10)),
            reserve_policy: group
                .and_then(|group| group.get(&Yaml::String("usageReservePolicy".into())))
                .cloned()
                .unwrap_or_else(|| Yaml::String("confirm".into())),
            fallback_chains: match group.and_then(|group| group.get(&Yaml::String("fallbackChains".into()))) {
                None => serde_json::Value::Object(serde_json::Map::new()),
                Some(value) => retry_fallback_json(value)?,
            },
            model_roles: root
                .get(&Yaml::String("modelRoles".into()))
                .map(retry_fallback_json)
                .transpose()?
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default(),
            fallback_revert_policy: ara_cli::retry_fallback::get_retry_fallback_revert_policy(
                group
                    .and_then(|group| group.get(&Yaml::String("fallbackRevertPolicy".into())))
                    .and_then(Yaml::as_str)
                    .map(|value| serde_json::Value::String(value.into()))
                    .as_ref(),
            ),
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

    pub(super) fn model_fallback(&self) -> bool {
        self.model_fallback
    }

    pub(super) fn usage_aware_fallback(&self) -> bool {
        self.usage_aware_fallback
    }

    /// Call only when usage preflight is enabled: Native division by 100
    /// performs Number coercion then, not while loading or writing settings.
    /// Preserve fractional/negative/nonfinite values and conversion errors.
    pub(super) fn usage_reserve_pct(&self) -> Result<f64> {
        codex_reset_number(&self.usage_reserve_pct).context("coercing retry.usageReservePct")
    }

    /// Exact string comparisons preserve Native's unknown-policy behavior;
    /// a non-string is not equal to any of the three recognized values.
    pub(super) fn reserve_policy(&self) -> &str {
        match self.usage_reserve_policy() {
            UsageReservePolicy::Confirm => "confirm",
            UsageReservePolicy::Auto => "auto",
            UsageReservePolicy::FailClosed => "fail-closed",
            UsageReservePolicy::Other => self.reserve_policy.as_str().unwrap_or(""),
        }
    }

    pub(super) fn usage_reserve_policy(&self) -> UsageReservePolicy {
        match self.reserve_policy.as_str() {
            Some("confirm") => UsageReservePolicy::Confirm,
            Some("auto") => UsageReservePolicy::Auto,
            Some("fail-closed") => UsageReservePolicy::FailClosed,
            _ => UsageReservePolicy::Other,
        }
    }

    /// Keep malformed source values for native warning validation. Candidate
    /// resolution separately spreads arrays and treats scalars as no chains.
    #[cfg(test)]
    pub(super) fn fallback_chains_configured(&self) -> &serde_json::Value {
        &self.fallback_chains
    }

    pub(super) fn fallback_chains(&self, role_names: &[String]) -> ara_cli::retry_fallback::RetryFallbackChains {
        ara_cli::retry_fallback::get_retry_fallback_chains(Some(&self.fallback_chains), role_names)
    }

    /// Fixed settings.ts:381-386,1245-1248: strings and all-string arrays.
    pub(super) fn model_role(&self, role: &str) -> Option<String> {
        let value = self.model_roles.get(role)?;
        if let Some(value) = value.as_str() {
            return Some(value.to_owned());
        }
        value
            .as_array()?
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map(|values| values.join(","))
    }

    pub(super) fn model_role_names(&self) -> Vec<String> {
        self.model_roles.keys().filter(|role| self.model_role(role).is_some()).cloned().collect()
    }

    pub(super) fn fallback_revert_policy(&self) -> ara_cli::retry_fallback::RetryFallbackRevertPolicy {
        self.fallback_revert_policy
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        let Some(path) = &self.path else {
            self.enabled = enabled;
            return Ok(());
        };
        // Reload the whole native document before changing the one key, so
        // fallback settings and disjoint external edits survive.
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
        // Publish enabled and all reloaded settings together only after
        // the atomic write succeeds. Failure leaves the live snapshot intact.
        *self = updated;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn isolated(enabled: bool) -> Self {
        let mut policy = Self::from_document(None, &empty_mapping()).expect("native retry defaults are valid");
        policy.enabled = enabled;
        policy
    }
}

fn read_retry_switch(group: Option<&Hash>, key: &str, default: bool) -> Result<bool> {
    let Some(value) = group.and_then(|group| group.get(&Yaml::String(key.into()))) else { return Ok(default) };
    // settings.ts returns raw values; the recovery consumer applies JS Boolean.
    Ok(match value {
        Yaml::Null | Yaml::Alias(_) | Yaml::BadValue => false,
        Yaml::Boolean(value) => *value,
        Yaml::Integer(value) => *value != 0,
        Yaml::Real(_) => {
            let value = codex_reset_number(value)?;
            value != 0.0 && !value.is_nan()
        }
        Yaml::String(value) => !value.is_empty(),
        Yaml::Array(_) | Yaml::Hash(_) => true,
    })
}

/// Project raw chain values into the existing resolver/validator. Nonfinite
/// YAML numbers remain non-string malformed entries, represented by JSON null;
/// the authored YAML stays intact in scoped writes.
fn retry_fallback_json(value: &Yaml) -> Result<serde_json::Value> {
    use serde_json::Value;
    Ok(match value {
        Yaml::Null | Yaml::Alias(_) | Yaml::BadValue => Value::Null,
        Yaml::Boolean(value) => Value::Bool(*value),
        Yaml::Integer(value) => Value::from(*value as f64),
        Yaml::Real(_) => Value::from(codex_reset_number(value)?),
        Yaml::String(value) => Value::String(value.clone()),
        Yaml::Array(values) => Value::Array(values.iter().map(retry_fallback_json).collect::<Result<_>>()?),
        Yaml::Hash(values) => {
            let mut object = serde_json::Map::new();
            for (key, value) in values {
                let key = match key {
                    Yaml::Null => "null".into(),
                    Yaml::Integer(value) => ara_prompt::js::f64_to_string(*value as f64),
                    Yaml::Real(_) => ara_prompt::js::f64_to_string(codex_reset_number(key)?),
                    other => codex_reset_array_string(other)?,
                };
                object.insert(key, retry_fallback_json(value)?);
            }
            Value::Object(object)
        }
    })
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

/// Saved-reset consent and knobs from fixed settings-schema.ts and
/// settings.ts:2231-2241. Only the nested disk group is effective: native
/// getByPath does not resolve quoted dotted disk keys. Its flat boolean
/// migration changes the raw legacy value but does not authorize auto-spend.
pub(super) struct CodexResetPolicy {
    path: Option<PathBuf>,
    settings: ara_cli::codex_auto_reset::CodexResetSettings,
}

impl CodexResetPolicy {
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
        use ara_cli::codex_auto_reset::{CodexAutoRedeemMode, CodexResetSettings};
        let root = document.as_hash().context("native settings root must be a mapping")?;
        let group = match root.get(&Yaml::String("codexResets".into())) {
            None | Some(Yaml::Null) => None,
            Some(group) => Some(group.as_hash().context("codexResets settings must be a mapping")?),
        };
        let auto_redeem = match group.and_then(|group| group.get(&Yaml::String("autoRedeem".into()))) {
            None => CodexAutoRedeemMode::Unset,
            Some(Yaml::Boolean(true)) => CodexAutoRedeemMode::Yes,
            Some(Yaml::Boolean(false)) => CodexAutoRedeemMode::No,
            Some(Yaml::String(value)) if value == "unset" => CodexAutoRedeemMode::Unset,
            Some(Yaml::String(value)) if value == "yes" => CodexAutoRedeemMode::Yes,
            Some(Yaml::String(value)) if value == "no" => CodexAutoRedeemMode::No,
            Some(_) => bail!("codexResets.autoRedeem must be unset, yes, or no"),
        };
        Ok(Self {
            path,
            settings: CodexResetSettings {
                auto_redeem,
                min_blocked_minutes: read_codex_reset_number(group, "minBlockedMinutes", 60.0)?,
                keep_credits: read_codex_reset_number(group, "keepCredits", 0.0)?,
                salvage_horizon_hours: read_codex_reset_number(group, "salvageHorizonHours", 12.0)?,
            },
        })
    }

    pub(super) fn settings(&self) -> ara_cli::codex_auto_reset::CodexResetSettings {
        self.settings
    }

    pub(super) fn set_mode(&mut self, mode: ara_cli::codex_auto_reset::CodexAutoRedeemMode) -> Result<()> {
        let Some(path) = &self.path else {
            self.settings.auto_redeem = mode;
            return Ok(());
        };
        let loaded = read_document(path)?;
        let mut document = loaded.as_ref().map(|(document, _)| document.clone()).unwrap_or_else(empty_mapping);
        Self::from_document(Some(path.clone()), &document)?;
        let root = document.as_mut_hash().expect("read_document validates mapping");
        let group = root.entry(Yaml::String("codexResets".into())).or_insert_with(empty_mapping);
        if matches!(group, Yaml::Null) {
            *group = empty_mapping();
        }
        group
            .as_mut_hash()
            .context("codexResets settings must be a mapping")?
            .insert(Yaml::String("autoRedeem".into()), Yaml::String(mode.as_str().into()));
        let updated = Self::from_document(Some(path.clone()), &document)?;
        let mut encoded = String::new();
        YamlEmitter::new(&mut encoded).dump(&document).context("serializing native codexResets settings")?;
        encoded.push('\n');
        write_atomically(path, &encoded, loaded.as_ref().map(|(_, source)| source.as_str()), "codex-resets")?;
        *self = updated;
        Ok(())
    }
}

fn read_codex_reset_number(group: Option<&Hash>, key: &str, default: f64) -> Result<f64> {
    match group.and_then(|group| group.get(&Yaml::String(key.into()))) {
        None => Ok(default),
        Some(value) => codex_reset_number(value).with_context(|| format!("coercing codexResets.{key}")),
    }
}

// Native Settings preserves authored values; AgentSession's Math.max/trunc
// and retry reserve division perform Number coercion.
// The prompt helper uses Null for undefined, so authored YAML null needs zero.
fn codex_reset_number(value: &Yaml) -> Result<f64> {
    match value {
        Yaml::Integer(value) => Ok(*value as f64),
        Yaml::Real(value) => match value.as_str() {
            ".nan" | ".NaN" | ".NAN" => Ok(f64::NAN),
            ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => Ok(f64::INFINITY),
            "-.inf" | "-.Inf" | "-.INF" => Ok(f64::NEG_INFINITY),
            _ => value.parse::<f64>().context("native YAML real must be numeric"),
        },
        Yaml::Null => Ok(0.0),
        Yaml::Boolean(value) => Ok(if *value { 1.0 } else { 0.0 }),
        Yaml::String(value) => Ok(codex_reset_string_number(value)),
        Yaml::Array(_) => Ok(codex_reset_string_number(&codex_reset_array_string(value)?)),
        Yaml::Hash(values) if values.contains_key(&Yaml::String("toString".into())) => {
            // A YAML property cannot be callable. It shadows Object.toString,
            // leaving both ordinary primitive-conversion attempts nonprimitive.
            bail!("native numeric coercion cannot convert an object with an authored toString property")
        }
        Yaml::Hash(_) | Yaml::Alias(_) | Yaml::BadValue => Ok(f64::NAN),
    }
}

fn codex_reset_string_number(value: &str) -> f64 {
    let json = serde_json::Value::String(value.into());
    // Reuse the catalog's precisely rounded finite radix conversion and the
    // prompt's full decimal grammar, empty-string and nonfinite semantics.
    ara_cli::model_policy::to_number(&json).unwrap_or_else(|| ara_prompt::js::to_number(&json))
}

fn codex_reset_array_string(value: &Yaml) -> Result<String> {
    match value {
        Yaml::Array(values) => {
            values.iter().map(codex_reset_array_string).collect::<Result<Vec<_>>>().map(|values| values.join(","))
        }
        Yaml::Null => Ok(String::new()),
        Yaml::String(value) => Ok(value.clone()),
        Yaml::Boolean(value) => Ok(value.to_string()),
        Yaml::Integer(_) | Yaml::Real(_) => Ok(ara_prompt::js::f64_to_string(codex_reset_number(value)?)),
        Yaml::Hash(values) if values.contains_key(&Yaml::String("toString".into())) => {
            bail!("native array conversion cannot stringify an object with an authored toString property")
        }
        Yaml::Hash(_) => Ok("[object Object]".into()),
        Yaml::Alias(_) | Yaml::BadValue => Ok("undefined".into()),
    }
}

#[cfg(test)]
mod codex_reset_policy_tests {
    use super::*;
    use ara_cli::codex_auto_reset::{CodexAutoRedeemMode, CodexResetSettings};

    #[test]
    fn saved_reset_defaults_nested_boolean_migration_and_native_filename_precedence() {
        let directory = tempfile::tempdir().unwrap();
        let primary = directory.path().join("config.yml");
        let alternate = directory.path().join("config.yaml");
        assert_eq!(CodexResetPolicy::load(directory.path()).unwrap().settings(), CodexResetSettings::default());
        assert!(!primary.exists(), "loading defaults must not create a config file");
        for (mode, expected) in [
            ("true", CodexAutoRedeemMode::Yes),
            ("false", CodexAutoRedeemMode::No),
            ("unset", CodexAutoRedeemMode::Unset),
            ("'yes'", CodexAutoRedeemMode::Yes),
            ("'no'", CodexAutoRedeemMode::No),
        ] {
            std::fs::write(&alternate, format!("codexResets:\n  autoRedeem: {mode}\n  minBlockedMinutes: -1.5\n  keepCredits: 2.9\n  salvageHorizonHours: 0\n")).unwrap();
            let settings = CodexResetPolicy::load(directory.path()).unwrap().settings();
            assert_eq!(settings.auto_redeem, expected, "{mode}");
            assert_eq!(
                (settings.min_blocked_minutes, settings.keep_credits, settings.salvage_horizon_hours),
                (-1.5, 2.9, 0.0)
            );
        }
        // Fixed native migration does not make a quoted dotted disk key a
        // effective nested value. In particular, legacy true cannot opt in.
        std::fs::write(&primary, "'codexResets.autoRedeem': true\n").unwrap();
        assert_eq!(
            CodexResetPolicy::load(directory.path()).unwrap().settings().auto_redeem,
            CodexAutoRedeemMode::Unset
        );
        std::fs::write(&primary, "codexResets:\n  autoRedeem: false\n'codexResets.autoRedeem': true\n").unwrap();
        assert_eq!(CodexResetPolicy::load(directory.path()).unwrap().settings().auto_redeem, CodexAutoRedeemMode::No);
        std::fs::write(
            &primary,
            "codexResets:\n  minBlockedMinutes: .nan\n  keepCredits: .inf\n  salvageHorizonHours: -.inf\n",
        )
        .unwrap();
        let settings = CodexResetPolicy::load(directory.path()).unwrap().settings();
        assert!(settings.min_blocked_minutes.is_nan());
        assert_eq!((settings.keep_credits, settings.salvage_horizon_hours), (f64::INFINITY, f64::NEG_INFINITY));
        for (raw, expected) in [
            ("null", 0.0),
            ("true", 1.0),
            ("false", 0.0),
            ("''", 0.0),
            ("'  1.5  '", 1.5),
            ("'0x10'", 16.0),
            ("'Infinity'", f64::INFINITY),
            ("'-Infinity'", f64::NEG_INFINITY),
            ("[]", 0.0),
            ("[null]", 0.0),
            ("[6]", 6.0),
            ("[['7']]", 7.0),
            ("[.inf]", f64::INFINITY),
            ("[true]", f64::NAN),
            ("[1, 2]", f64::NAN),
            ("{}", f64::NAN),
            ("'inf'", f64::NAN),
            ("'not a number'", f64::NAN),
        ] {
            std::fs::write(&primary, format!("codexResets:\n  keepCredits: {raw}\n")).unwrap();
            let actual = CodexResetPolicy::load(directory.path()).unwrap().settings().keep_credits;
            assert!(actual == expected || actual.is_nan() && expected.is_nan(), "{raw}: {actual}");
        }
        for invalid in [
            "codexResets: []\n",
            "codexResets:\n  autoRedeem: always\n",
            "codexResets:\n  keepCredits: {toString: 1}\n",
            "codexResets:\n  autoRedeem: null\n",
            "[invalid\n",
        ] {
            std::fs::write(&primary, invalid).unwrap();
            assert!(CodexResetPolicy::load(directory.path()).is_err(), "{invalid}");
            assert_eq!(
                std::fs::read_to_string(&primary).unwrap(),
                invalid,
                "invalid primary is never silently overwritten or bypassed"
            );
        }
    }

    #[test]
    fn saved_reset_consent_writes_reload_disjoint_edits_and_publish_only_after_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        let mut policy = CodexResetPolicy::load(directory.path()).unwrap();
        std::fs::write(&path, "codexResets:\n  keepCredits: 3.5\n  minBlockedMinutes: 120\n  salvageHorizonHours: 6\n  futureKnob: retained\n'codexResets.autoRedeem': true\ncustom:\n  keep: old\n").unwrap();
        policy.set_mode(CodexAutoRedeemMode::Yes).unwrap();
        assert_eq!(
            policy.settings(),
            CodexResetSettings {
                auto_redeem: CodexAutoRedeemMode::Yes,
                min_blocked_minutes: 120.0,
                keep_credits: 3.5,
                salvage_horizon_hours: 6.0
            }
        );
        let document = read_document(&path).unwrap().unwrap().0;
        assert_eq!(document["codexResets"]["autoRedeem"].as_str(), Some("yes"));
        assert_eq!(document["codexResets"]["futureKnob"].as_str(), Some("retained"));
        assert_eq!(document["codexResets.autoRedeem"].as_bool(), Some(true));
        assert_eq!(document["custom"]["keep"].as_str(), Some("old"));
        assert_eq!(CodexResetPolicy::load(directory.path()).unwrap().settings(), policy.settings());
        std::fs::write(&path, "codexResets:\n  autoRedeem: 'yes'\n  keepCredits: 8\ncustom: newer\n").unwrap();
        policy.set_mode(CodexAutoRedeemMode::No).unwrap();
        assert_eq!(policy.settings().keep_credits, 8.0);
        assert_eq!(read_document(&path).unwrap().unwrap().0["custom"].as_str(), Some("newer"));
        let before = policy.settings();
        std::fs::write(&path, "codexResets: [malformed]\ncustom: survive\n").unwrap();
        assert!(policy.set_mode(CodexAutoRedeemMode::Yes).is_err());
        assert_eq!(policy.settings(), before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "codexResets: [malformed]\ncustom: survive\n");
        let blocked = tempfile::tempdir().unwrap();
        let blocked_parent = blocked.path().join("file-as-parent");
        std::fs::write(&blocked_parent, "occupied").unwrap();
        let mut unwritable = CodexResetPolicy { path: Some(blocked_parent.join("config.yml")), settings: before };
        assert!(unwritable.set_mode(CodexAutoRedeemMode::Yes).is_err());
        assert_eq!(unwritable.settings(), before);
        assert_eq!(std::fs::read_to_string(blocked_parent).unwrap(), "occupied");
        assert!(
            directory.path().read_dir().unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ara-codex-resets-"))
        );
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
    #[test]
    fn edit_settings_adapter_uses_flat_precedence_and_native_integer_key_order() {
        let source = r#"edit:
  fuzzyMatch: false
  fuzzyThreshold: 0.8
  enforceSeenLines: true
  blockAutoGenerated: false
  mode: replace
  modelVariants:
    alpha: replace
    '10': patch
    '2': apply_patch
    '01': sloppy
    '4294967295': hashline
    beta: false
edit.mode: patch
edit.fuzzyMatch: true
"#;
        let documents = YamlLoader::load_from_str(source).unwrap();
        let settings = edit_mode_settings_from_document(&documents[0]).unwrap();
        assert_eq!(settings.mode, Some(serde_json::json!("patch")));
        let raw = edit_tool_settings_from_document(&documents[0]).unwrap();
        let engine = super::super::edit_mode_policy::resolve_edit_settings(&raw, Default::default()).unwrap();
        assert!(!engine.allow_fuzzy); // Quoted edit.fuzzyMatch=true is ignored.
        assert_eq!(engine.fuzzy_threshold, 0.8);
        assert!(engine.enforce_seen_lines);
        assert!(!engine.block_auto_generated);
        let defaults = YamlLoader::load_from_str("{}").unwrap();
        assert_eq!(edit_tool_settings_from_document(&defaults[0]).unwrap(), Default::default());
        let flat_only = YamlLoader::load_from_str("edit.fuzzyMatch: false\nedit.fuzzyThreshold: 0.1\nedit.enforceSeenLines: true\nedit.blockAutoGenerated: false\n").unwrap();
        assert_eq!(edit_tool_settings_from_document(&flat_only[0]).unwrap(), Default::default());
        for source in [
            "edit: {fuzzyMatch: null}",
            "edit: {enforceSeenLines: 'false'}",
            "edit: {blockAutoGenerated: 0}",
            "edit: {fuzzyThreshold: '0.9'}",
        ] {
            let malformed = YamlLoader::load_from_str(source).unwrap();
            let raw = edit_tool_settings_from_document(&malformed[0]).unwrap();
            assert!(super::super::edit_mode_policy::resolve_edit_settings(&raw, Default::default()).is_err());
        }
        // Fixed constructor does not consume fuzzyMatch/threshold under an
        // explicit environment override; preserve wrong raw leaves until then.
        let malformed = YamlLoader::load_from_str("edit: {fuzzyMatch: null, fuzzyThreshold: bad}").unwrap();
        let raw = edit_tool_settings_from_document(&malformed[0]).unwrap();
        let overrides = super::super::edit_mode_policy::EditSettingsEnvironment {
            fuzzy: Some("true"),
            fuzzy_threshold: Some("0.9"),
        };
        let engine = super::super::edit_mode_policy::resolve_edit_settings(&raw, overrides).unwrap();
        assert!(engine.allow_fuzzy);
        assert_eq!(engine.fuzzy_threshold, 0.9);
        assert!(super::super::edit_mode_policy::resolve_edit_settings(&raw, Default::default()).is_err());
        assert_eq!(
            settings.model_variants.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            ["2", "10", "alpha", "01", "4294967295", "beta"]
        );
        assert_eq!(settings.model_variants.last().unwrap().1, serde_json::json!(false));
        let flat = format!("{source}edit.modelVariants:\n  flat: apply_patch\n");
        let documents = YamlLoader::load_from_str(&flat).unwrap();
        let settings = edit_mode_settings_from_document(&documents[0]).unwrap();
        assert_eq!(settings.model_variants, [("flat".to_owned(), serde_json::json!("apply_patch"))]);
    }

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
        assert!(policy.model_fallback());
        assert!(!policy.usage_aware_fallback());
        assert_eq!(policy.usage_reserve_pct().unwrap(), 10.0);
        assert_eq!(policy.reserve_policy(), "confirm");
        assert_eq!(policy.usage_reserve_policy(), UsageReservePolicy::Confirm);
        assert_eq!(policy.fallback_chains_configured(), &serde_json::json!({}));
        assert!(policy.fallback_chains(&["task".into()]).is_empty());
        assert_eq!(policy.fallback_revert_policy(), ara_cli::retry_fallback::RetryFallbackRevertPolicy::CooldownExpiry);
        assert!(!agent.exists());
        policy.set_enabled(false).unwrap();
        assert!(!policy.enabled());
        assert!(agent.join("config.yml").is_file());
        let reopened = RetryPolicy::load(&agent).unwrap();
        assert!(!reopened.enabled());
        assert_eq!(reopened.max_retries(), 10.0);
        assert!(reopened.model_fallback());
        assert_eq!(reopened.reserve_policy(), "confirm");
        let mut isolated = RetryPolicy::isolated(false);
        isolated.set_enabled(true).unwrap();
        assert!(isolated.enabled());
        assert_eq!(isolated.base_delay_ms(), 500.0);
        assert!(isolated.model_fallback());
        assert_eq!(isolated.usage_reserve_pct().unwrap(), 10.0);
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
        std::fs::write(
            &path,
            "retry:\n  maxRetries: 1.5\n  baseDelayMs: -2.75\n  maxDelayMs: 0\n  usageReservePct: -12.75\n",
        )
        .unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        assert_eq!(policy.max_retries(), 1.5);
        assert_eq!(policy.base_delay_ms(), -2.75);
        assert_eq!(policy.max_delay_ms(), 0.0);
        assert_eq!(policy.usage_reserve_pct().unwrap(), -12.75);
        policy.set_enabled(false).unwrap();
        let reopened = RetryPolicy::load(directory.path()).unwrap();
        assert_eq!(reopened.max_retries(), 1.5);
        assert_eq!(reopened.base_delay_ms(), -2.75);
        assert_eq!(reopened.max_delay_ms(), 0.0);
        assert_eq!(reopened.usage_reserve_pct().unwrap(), -12.75);
        // Number coercion belongs to native reserve division, not the already
        // typed retry-count/delay surface. Keep overflow/unknown values intact.
        for (value, expected) in [
            ("125.5", 125.5),
            ("'12.5'", 12.5),
            ("null", 0.0),
            ("true", 1.0),
            ("[10]", 10.0),
            (".nan", f64::NAN),
            (".inf", f64::INFINITY),
            ("'unknown'", f64::NAN),
        ] {
            std::fs::write(&path, format!("retry:\n  usageReservePct: {value}\n")).unwrap();
            let mut policy = RetryPolicy::load(directory.path()).unwrap();
            let actual = policy.usage_reserve_pct().unwrap();
            assert!(actual == expected || actual.is_nan() && expected.is_nan(), "{value}: {actual}");
            policy.set_enabled(false).unwrap();
            let actual = RetryPolicy::load(directory.path()).unwrap().usage_reserve_pct().unwrap();
            assert!(actual == expected || actual.is_nan() && expected.is_nan(), "reloaded {value}: {actual}");
        }
    }

    #[test]
    fn retry_scoped_write_retains_fallback_custom_and_compaction_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "retry:\n  enabled: true\n  maxRetries: 1\ncustom: old\n").unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        // A scoped toggle reloads live fallback values while retaining every
        // other authored source value and unrelated nested setting.
        std::fs::write(&path, "retry:\n  enabled: true\n  maxRetries: 7\n  baseDelayMs: 12.5\n  maxDelayMs: 0\n  modelFallback: false\n  usageAwareFallback: true\n  usageReservePct: 12.5\n  usageReservePolicy: auto\n  fallbackChains:\n    default: [openai/fallback]\n    task: [openai/task]\n    provider/*: [other/*]\n  fallbackRevertPolicy: never\n  custom: [one, 2, true]\ncompaction:\n  enabled: false\n  methodOrder: []\ncustom:\n  external: changed\n").unwrap();
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
        assert!(!policy.model_fallback());
        assert!(policy.usage_aware_fallback());
        assert_eq!(policy.usage_reserve_pct().unwrap(), 12.5);
        assert_eq!(policy.reserve_policy(), "auto");
        assert_eq!(policy.usage_reserve_policy(), UsageReservePolicy::Auto);
        let chains = policy.fallback_chains(&["task".into(), "advisor".into()]);
        assert_eq!(chains["task"], serde_json::json!(["openai/task"]));
        assert_eq!(chains["advisor"], serde_json::json!(["openai/fallback"]));
        assert_eq!(chains["provider/*"], serde_json::json!(["other/*"]));
        assert_eq!(policy.fallback_revert_policy(), ara_cli::retry_fallback::RetryFallbackRevertPolicy::Never);
        let reopened = RetryPolicy::load(directory.path()).unwrap();
        assert_eq!(reopened.fallback_chains_configured(), policy.fallback_chains_configured());
        assert_eq!(reopened.reserve_policy(), "auto");
    }

    #[test]
    fn retry_fallback_raw_values_keep_native_equality_and_warning_inputs() {
        use ara_cli::retry_fallback::RetryFallbackRevertPolicy::{CooldownExpiry, Never};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        for (source, switches, reserve_policy, typed_policy, revert, configured) in [
            (
                "modelFallback: null\n  usageAwareFallback: 'false'\n  usageReservePolicy: unknown\n  fallbackRevertPolicy: unknown\n  fallbackChains: null",
                (false, true),
                "unknown",
                UsageReservePolicy::Other,
                CooldownExpiry,
                serde_json::Value::Null,
            ),
            (
                "modelFallback: []\n  usageAwareFallback: 0\n  usageReservePolicy: null\n  fallbackRevertPolicy: null\n  fallbackChains: false",
                (true, false),
                "",
                UsageReservePolicy::Other,
                CooldownExpiry,
                serde_json::json!(false),
            ),
            (
                "modelFallback: ''\n  usageAwareFallback: {}\n  usageReservePolicy: fail-closed\n  fallbackRevertPolicy: never\n  fallbackChains: [[openai/a], 7]",
                (false, true),
                "fail-closed",
                UsageReservePolicy::FailClosed,
                Never,
                serde_json::json!([["openai/a"], 7.0]),
            ),
            (
                "modelFallback: true\n  usageAwareFallback: false\n  usageReservePolicy: confirm\n  fallbackChains: {default: [openai/a, null], broken: scalar}",
                (true, false),
                "confirm",
                UsageReservePolicy::Confirm,
                CooldownExpiry,
                serde_json::json!({"default":["openai/a",null],"broken":"scalar"}),
            ),
        ] {
            let source =
                format!("retry:\n  {source}\n'retry.modelFallback': false\n'retry.usageReservePolicy': auto\n");
            std::fs::write(&path, &source).unwrap();
            let mut policy = RetryPolicy::load(directory.path()).unwrap();
            assert_eq!((policy.model_fallback(), policy.usage_aware_fallback()), switches, "{source}");
            assert_eq!(policy.reserve_policy(), reserve_policy, "{source}");
            assert_eq!(policy.usage_reserve_policy(), typed_policy, "{source}");
            assert_eq!(policy.fallback_revert_policy(), revert, "{source}");
            assert_eq!(policy.fallback_chains_configured(), &configured, "{source}");
            let chains = policy.fallback_chains(&["task".into()]);
            if let Some(default) = configured.get("default") {
                assert_eq!(chains["task"], *default);
            } else if configured.is_array() {
                assert_eq!(chains["0"], configured[0]);
                assert_eq!(chains["1"], configured[1]);
            } else {
                assert!(chains.is_empty());
            }
            let before = read_document(&path).unwrap().unwrap().0;
            policy.set_enabled(false).unwrap();
            assert_eq!(policy.reserve_policy(), reserve_policy);
            assert_eq!(policy.fallback_chains_configured(), &configured);
            let after = read_document(&path).unwrap().unwrap().0;
            assert_eq!(after["retry"]["fallbackChains"], before["retry"]["fallbackChains"]);
            assert_eq!(after["retry"]["usageReservePolicy"], before["retry"]["usageReservePolicy"]);
        }
        // A Settings load/toggle never evaluates reserve arithmetic, even
        // when the feature is enabled. Only actual usage preflight can fail
        // with Native Number's authored non-callable toString property.
        for usage_enabled in [false, true] {
            std::fs::write(
                &path,
                format!("retry:\n  usageAwareFallback: {usage_enabled}\n  usageReservePct: {{toString: blocked}}\n"),
            )
            .unwrap();
            let mut policy = RetryPolicy::load(directory.path()).unwrap();
            assert_eq!(policy.usage_aware_fallback(), usage_enabled);
            policy.set_enabled(false).unwrap();
            let reopened = RetryPolicy::load(directory.path()).unwrap();
            assert_eq!(reopened.usage_aware_fallback(), usage_enabled);
            assert_eq!(
                read_document(&path).unwrap().unwrap().0["retry"]["usageReservePct"]["toString"].as_str(),
                Some("blocked")
            );
            if usage_enabled {
                assert!(
                    reopened.usage_reserve_pct().is_err(),
                    "enabled preflight must retain Number conversion failure"
                );
            }
        }
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
        std::fs::write(&path, "retry:\n  enabled: true\n  maxRetries: 2.5\n  baseDelayMs: 25\n  maxDelayMs: 100\n  modelFallback: false\n  usageAwareFallback: true\n  usageReservePct: 20\n  usageReservePolicy: fail-closed\n  fallbackChains: {default: [openai/fallback]}\n  fallbackRevertPolicy: never\n")
            .unwrap();
        let mut policy = RetryPolicy::load(directory.path()).unwrap();
        for source in ["bad: [", "retry:\n  maxRetries: 'bad'\n"] {
            std::fs::write(&path, source).unwrap();
            assert!(policy.set_enabled(false).is_err());
            assert!(policy.enabled());
            assert_eq!(policy.max_retries(), 2.5);
            assert_eq!(policy.base_delay_ms(), 25.0);
            assert_eq!(policy.max_delay_ms(), 100.0);
            assert!(!policy.model_fallback());
            assert!(policy.usage_aware_fallback());
            assert_eq!(policy.usage_reserve_pct().unwrap(), 20.0);
            assert_eq!(policy.reserve_policy(), "fail-closed");
            assert_eq!(policy.fallback_chains_configured(), &serde_json::json!({"default":["openai/fallback"]}));
            assert_eq!(policy.fallback_revert_policy(), ara_cli::retry_fallback::RetryFallbackRevertPolicy::Never);
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

// Fixed OMP provider.appendOnlyContext SettingSignal. Reuse this file's native
// YAML reader, filename precedence and generation-checked atomic writer.
struct AppendContextStore {
    agent_dir: PathBuf,
}

impl AppendContextStore {
    fn document(&self) -> Result<(PathBuf, Yaml, Option<String>)> {
        for filename in MAIN_CONFIG_FILENAMES {
            let path = self.agent_dir.join(filename);
            if let Some((document, original)) = read_document(&path)? {
                return Ok((path, document, Some(original)));
            }
        }
        Ok((self.agent_dir.join(MAIN_CONFIG_FILENAMES[0]), empty_mapping(), None))
    }

    fn preferences(&self) -> Result<ara_cli::append_only_context_mode::HostContextPreferences> {
        use ara_cli::append_only_context_mode::{AppendOnlyContextMode, HostContextPreferences};
        let (_, document, _) = self.document()?;
        let root = document.as_hash().context("native settings root must be a mapping")?;
        let group = |key: &str| -> Result<Option<&Hash>> {
            match root.get(&Yaml::String(key.into())) {
                None | Some(Yaml::Null) => Ok(None),
                Some(value) => Ok(Some(value.as_hash().with_context(|| format!("{key} settings must be a mapping"))?)),
            }
        };
        let provider = group("provider")?;
        let tools = group("tools")?;
        let mode = match provider.and_then(|group| group.get(&Yaml::String("appendOnlyContext".into()))) {
            None | Some(Yaml::Null) => AppendOnlyContextMode::Auto,
            Some(Yaml::String(value)) => AppendOnlyContextMode::parse(value).map_err(anyhow::Error::msg)?,
            Some(_) => bail!("provider.appendOnlyContext must be auto, on or off"),
        };
        let intent_tracing = match tools.and_then(|group| group.get(&Yaml::String("intentTracing".into()))) {
            None | Some(Yaml::Null) => true,
            Some(Yaml::Boolean(value)) => *value,
            Some(_) => bail!("tools.intentTracing must be a boolean"),
        };
        let inline_descriptors = match root.get(&Yaml::String("inlineToolDescriptors".into())) {
            None | Some(Yaml::Null) => AppendOnlyContextMode::Auto,
            // Native settings migration retains legacy boolean meaning.
            Some(Yaml::Boolean(true)) => AppendOnlyContextMode::On,
            Some(Yaml::Boolean(false)) => AppendOnlyContextMode::Off,
            Some(Yaml::String(value)) => AppendOnlyContextMode::parse(value).map_err(anyhow::Error::msg)?,
            Some(_) => bail!("inlineToolDescriptors must be auto, on or off"),
        };
        let tool_format = match tools.and_then(|group| group.get(&Yaml::String("format".into()))) {
            None | Some(Yaml::Null) => "auto".into(),
            Some(Yaml::String(value))
                if [
                    "auto",
                    "native",
                    "glm",
                    "hermes",
                    "kimi",
                    "xml",
                    "anthropic",
                    "deepseek",
                    "harmony",
                    "qwen3",
                    "gemini",
                    "gemma",
                    "minimax",
                ]
                .contains(&value.as_str()) =>
            {
                value.clone()
            }
            Some(_) => bail!("tools.format must be a supported Native tool calling mode"),
        };
        Ok(HostContextPreferences { mode, intent_tracing, inline_descriptors, tool_format })
    }
}

impl ara_cli::append_only_context_mode::AppendOnlyContextSettingsStore for AppendContextStore {
    fn load(&self) -> std::result::Result<ara_cli::append_only_context_mode::HostContextPreferences, String> {
        self.preferences().map_err(|error| format!("{error:#}"))
    }
    fn store_mode(
        &self,
        mode: ara_cli::append_only_context_mode::AppendOnlyContextMode,
    ) -> std::result::Result<(), String> {
        (|| -> Result<()> {
            let (path, mut document, original) = self.document()?;
            // Validate before any mutation; preserve the admitted live mode if
            // malformed disk settings or an external write rejects publication.
            self.preferences()?;
            let root = document.as_mut_hash().context("native settings root must be a mapping")?;
            let provider = root.entry(Yaml::String("provider".into())).or_insert_with(empty_mapping);
            if *provider == Yaml::Null {
                *provider = empty_mapping();
            }
            let provider = provider.as_mut_hash().context("provider settings must be a mapping")?;
            provider.insert(Yaml::String("appendOnlyContext".into()), Yaml::String(mode.as_str().into()));
            let mut encoded = String::new();
            YamlEmitter::new(&mut encoded).dump(&document).context("serializing native append-only setting")?;
            write_atomically(&path, &encoded, original.as_deref(), "append-only-context")
        })()
        .map_err(|error| format!("{error:#}"))
    }
}

pub(super) fn append_context_settings(
    agent_dir: &Path,
) -> Result<std::sync::Arc<ara_cli::append_only_context_mode::AppendOnlyContextSettings>> {
    use ara_cli::append_only_context_mode::AppendOnlyContextSettings;
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    static SETTINGS: OnceLock<Mutex<std::collections::HashMap<PathBuf, Weak<AppendOnlyContextSettings>>>> =
        OnceLock::new();
    let path = std::path::absolute(agent_dir)?;
    let mut settings = SETTINGS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(current) = settings.get(&path).and_then(Weak::upgrade) {
        return Ok(current);
    }
    let current = AppendOnlyContextSettings::new(Arc::new(AppendContextStore { agent_dir: path.clone() }))
        .map_err(anyhow::Error::msg)?;
    settings.insert(path, Arc::downgrade(&current));
    Ok(current)
}

#[cfg(test)]
mod append_context_tests {
    use super::*;
    use ara_cli::append_only_context_mode::{AppendOnlyContextMode as Mode, SessionContextController};
    use std::sync::Arc;

    #[test]
    fn actual_yaml_owner_two_sessions_set_reload_drop_and_rejected_publication_group() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(directory.path().join("config.yaml"), "provider: {appendOnlyContext: off}\n").unwrap();
        std::fs::write(&path, "custom: {preserve: authored}\ntools: {intentTracing: true, format: native}\ninlineToolDescriptors: false\n").unwrap();
        let first_owner = append_context_settings(directory.path()).unwrap();
        let second_owner = append_context_settings(directory.path()).unwrap();
        assert!(Arc::ptr_eq(&first_owner, &second_owner));
        let preferences = first_owner.preferences().unwrap();
        assert_eq!(preferences.mode, Mode::Auto);
        assert!(preferences.intent_tracing);
        assert_eq!(preferences.inline_descriptors, Mode::Off);
        let selected = ara_ai::Model {
            id: "fixture".into(),
            provider: "deepseek".into(),
            api: "openai-completions".into(),
            base_url: "https://remote.example".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        let options = preferences.build_options(&selected.id, None, None).unwrap();
        let first =
            SessionContextController::new(first_owner.clone(), &selected, None, options, &preferences.tool_format);
        let second =
            SessionContextController::new(second_owner.clone(), &selected, None, options, &preferences.tool_format);
        assert_eq!(first_owner.subscription_count(), 2);
        first_owner.set(Mode::Off).unwrap();
        assert!(first.binding().append_only_context.is_none());
        assert!(second.binding().append_only_context.is_none());
        let document = read_document(&path).unwrap().unwrap().0;
        assert_eq!(document["provider"]["appendOnlyContext"].as_str(), Some("off"));
        assert_eq!(document["custom"]["preserve"].as_str(), Some("authored"));
        assert_eq!(document["tools"]["format"].as_str(), Some("native"));
        std::fs::write(&path, "provider: {appendOnlyContext: on}\n").unwrap();
        second_owner.reload().unwrap();
        assert!(first.binding().append_only_context.is_some());
        assert!(second.binding().append_only_context.is_some());
        for malformed in ["bad: [", "provider: {appendOnlyContext: wrong}", "tools: {format: unknown}"] {
            std::fs::write(&path, malformed).unwrap();
            assert!(first_owner.reload().is_err());
            assert!(first_owner.set(Mode::Off).is_err());
            assert_eq!(first_owner.get(), Mode::On);
            assert!(first.binding().append_only_context.is_some());
            assert!(second.binding().append_only_context.is_some());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), malformed);
        }
        drop(first);
        assert_eq!(first_owner.subscription_count(), 1);
        drop(second);
        assert_eq!(first_owner.subscription_count(), 0);
        // Presentation SOURCE POST: exercise the actual global/project Host
        // reader inside this existing coarse settings group, without a gate.
        let agent_dir = directory.path().join("presentation-agent");
        let cwd = directory.path().join("presentation-project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(cwd.join(".ara")).unwrap();
        std::fs::write(agent_dir.join("config.yml"), "display: {cacheMissMarker: true, smoothStreaming: false}\ntui: {maxInlineImageColumns: 90, maxInlineImageRows: 10}\n").unwrap();
        for (project, expected) in [
            ("display: {cacheMissMarker: null}\ntui: {maxInlineImageRows: null}\n", (false, false, 90.0, 20.0)),
            ("display: null\ntui: null\n", (false, true, 100.0, 20.0)),
            ("display: replaced\ntui: replaced\n", (false, true, 100.0, 20.0)),
            ("display: {}\ntui: {maxInlineImageRows: 5}\n", (true, false, 90.0, 5.0)),
            ("display: {smoothStreaming: true}\ntui: {}\n", (true, true, 90.0, 10.0)),
            ("display: {smoothStreaming: null}\ntui: {}\n", (true, false, 90.0, 10.0)),
        ] {
            std::fs::write(cwd.join(".ara/config.yml"), project).unwrap();
            let settings = terminal_presentation_settings(&cwd, &agent_dir).unwrap();
            assert_eq!(
                (settings.cache_miss_marker, settings.smooth_streaming, settings.image_columns, settings.image_rows),
                expected,
                "{project}"
            );
        }
    }
}

/// Fixed display.cacheMissMarker/smoothStreaming and two tui caps; Host owns paths.
pub(super) fn terminal_presentation_settings(
    cwd: &Path,
    agent_dir: &Path,
) -> Result<ara_cli::terminal::render_utils::TerminalPresentationSettings> {
    let mut result = ara_cli::terminal::render_utils::TerminalPresentationSettings::default();
    let mut display = Yaml::BadValue;
    let mut tui = Yaml::BadValue;
    let mut documents = Vec::new();
    for filename in MAIN_CONFIG_FILENAMES {
        if let Some((document, _)) = read_document(&agent_dir.join(filename))? {
            documents.push(document);
            break;
        }
    }
    if let Some((document, _)) = read_document(&cwd.join(".ara/config.yml"))? {
        documents.push(document);
    }
    for document in documents {
        document.as_hash().context("native presentation settings root must be a mapping")?;
        // Only the two relevant groups are merged: absent groups inherit,
        // mappings merge, and authored null/scalars replace the whole group.
        for (name, merged) in [("display", &mut display), ("tui", &mut tui)] {
            let incoming = &document[name];
            if matches!(incoming, Yaml::BadValue) {
                continue;
            }
            if let (Yaml::Hash(previous), Yaml::Hash(next)) = (&mut *merged, incoming) {
                for (key, value) in next {
                    previous.insert(key.clone(), value.clone());
                }
            } else {
                *merged = incoming.clone();
            }
        }
    }
    result.collapse_compacted = match &display["collapseCompacted"] {
        Yaml::BadValue => true,
        Yaml::Null => false,
        Yaml::Boolean(v) => *v,
        Yaml::String(v) => !v.is_empty(),
        Yaml::Integer(v) => *v != 0,
        Yaml::Real(v) => v.parse::<f64>().is_ok_and(|n| n != 0.0 && !n.is_nan()),
        _ => true,
    };
    result.cache_miss_marker = match &display["cacheMissMarker"] {
        Yaml::BadValue | Yaml::Null => false,
        Yaml::Boolean(v) => *v,
        Yaml::String(v) => !v.is_empty(),
        Yaml::Integer(v) => *v != 0,
        Yaml::Real(v) => v.parse::<f64>().is_ok_and(|n| n != 0.0 && !n.is_nan()),
        _ => true,
    };
    result.smooth_streaming = match &display["smoothStreaming"] {
        Yaml::BadValue => true,
        Yaml::Null => false,
        Yaml::Boolean(v) => *v,
        Yaml::String(v) => !v.is_empty(),
        Yaml::Integer(v) => *v != 0,
        Yaml::Real(v) => v.parse::<f64>().is_ok_and(|n| n != 0.0 && !n.is_nan()),
        _ => true,
    };
    for (name, target) in
        [("maxInlineImageColumns", &mut result.image_columns), ("maxInlineImageRows", &mut result.image_rows)]
    {
        match &tui[name] {
            Yaml::BadValue | Yaml::Null => {}
            Yaml::Integer(n) => *target = *n as f64,
            Yaml::Real(n) => *target = n.parse().context("native image cap must be numeric")?,
            _ => bail!("native tui.{name} image cap must be numeric"),
        }
    }
    Ok(result)
}


/// Live exact-agent policy. Credential ownership remains in shared AuthStorage.
pub(super) struct AgentAccountPools(std::collections::BTreeMap<String, ara_cli::auth_storage::OAuthAccountPools>);
impl AgentAccountPools {
    pub fn load(cwd: &Path, agent_dir: &Path) -> Result<Self> {
        let mut documents = Vec::new();
        for filename in MAIN_CONFIG_FILENAMES {
            if let Some((document, _)) = read_document(&agent_dir.join(filename))? { documents.push(document); break; }
        }
        if let Some((document, _)) = read_document(&cwd.join(".ara/config.yml"))? { documents.push(document); }
        let mut value = Yaml::BadValue;
        for document in documents { merge_read_group(&mut value, &document["task"]["agentAccountPools"]); }
        let mut agents = std::collections::BTreeMap::new();
        if matches!(value, Yaml::BadValue | Yaml::Null) { return Ok(Self(agents)); }
        let Yaml::Hash(entries) = value else { bail!("Invalid task.agentAccountPools: expected a map"); };
        for (agent, providers) in entries {
            let Yaml::String(agent) = agent else { bail!("Invalid task.agentAccountPools: agent names must be strings"); };
            if matches!(providers, Yaml::Null) { continue; }
            let Yaml::Hash(providers) = providers else { bail!("Invalid task.agentAccountPools agent entry: expected provider map"); };
            let mut pools = ara_cli::auth_storage::OAuthAccountPools::new();
            for (provider, keys) in providers {
                let Yaml::String(provider) = provider else { bail!("Invalid task.agentAccountPools: provider names must be strings"); };
                let Yaml::Array(keys) = keys else { bail!("Invalid task.agentAccountPools provider entry: expected identity list"); };
                let mut identities = std::collections::BTreeSet::new();
                for key in keys {
                    let Yaml::String(key) = key else { bail!("Invalid task.agentAccountPools: identity must be a string"); };
                    if key.is_empty() || ara_prompt::js::trim(&key) != key { bail!("Invalid task.agentAccountPools: identity must be nonempty without surrounding whitespace"); }
                    identities.insert(key);
                }
                pools.insert(provider, identities);
            }
            agents.insert(agent, pools);
        }
        Ok(Self(agents))
    }
    pub fn for_definition(&self, raw: Option<&str>) -> Option<ara_cli::auth_storage::OAuthAccountPools> {
        raw.filter(|raw| !raw.is_empty()).and_then(|raw| self.0.get(raw)).cloned()
    }
}


#[cfg(test)]
mod cold_account_pool_tests {
    use super::*;
    #[test]
    fn live_account_pools_merge_null_exact_keys_and_empty_denial() {
        let root=tempfile::tempdir().unwrap();let global=root.path().join("agent");let cwd=root.path().join("project");
        std::fs::create_dir_all(&global).unwrap();std::fs::create_dir_all(cwd.join(".ara")).unwrap();
        std::fs::write(global.join("config.yaml"),"task:\n  agentAccountPools:\n    scout:\n      one: [account:A]\n      two: [account:B]\n    clear:\n      one: [account:A]\n    __proto__:\n      one: [account:A]\n").unwrap();
        std::fs::write(cwd.join(".ara/config.yml"),"task:\n  agentAccountPools:\n    scout:\n      one: []\n    clear: null\n").unwrap();
        let policy=AgentAccountPools::load(&cwd,&global).unwrap();let scout=policy.for_definition(Some("scout")).unwrap();
        assert!(scout["one"].is_empty());assert!(scout["two"].contains("account:B"));assert!(policy.for_definition(Some("clear")).is_none());assert!(policy.for_definition(Some("__proto__")).is_some());
        assert!(policy.for_definition(Some("SCOUT")).is_none());assert!(policy.for_definition(None).is_none());assert!(policy.for_definition(Some("")).is_none());
    }
    #[test]
    fn malformed_effective_pool_never_silently_widens_unselected_agent() {
        let root=tempfile::tempdir().unwrap();let global=root.path().join("agent");std::fs::create_dir(&global).unwrap();
        for source in ["task:\n  agentAccountPools: []\n","task:\n  agentAccountPools:\n    unrelated:\n      one: null\n","task:\n  agentAccountPools:\n    unrelated:\n      one: [42]\n","task:\n  agentAccountPools:\n    unrelated:\n      one: [' account:A']\n"] {
            std::fs::write(global.join("config.yml"),source).unwrap();assert!(AgentAccountPools::load(root.path(),&global).is_err());
        }
        std::fs::write(global.join("config.yml"),"task.agentAccountPools: {scout: {one: []}}\n").unwrap();
        assert!(AgentAccountPools::load(root.path(),&global).unwrap().for_definition(Some("scout")).is_none());
    }
}

#[cfg(test)]
mod peer_messaging_policy_tests {
    use super::*;
    #[test]
    fn actual_nested_policy_default_negative_fraction_child_and_host_disable(){
        let global=tempfile::tempdir().unwrap();let cwd=tempfile::tempdir().unwrap();
        assert!(peer_messaging_enabled(cwd.path(),global.path(),0,true).unwrap());
        for (value,expected) in [("0",false),("-1",true),("0.5",true),("2",true)]{
            std::fs::write(global.path().join("config.yml"),format!("task:\n  maxRecursionDepth: {value}\n")).unwrap();
            assert_eq!(peer_messaging_enabled(cwd.path(),global.path(),0,true).unwrap(),expected);
            assert!(peer_messaging_enabled(cwd.path(),global.path(),2,true).unwrap());
            assert!(!peer_messaging_enabled(cwd.path(),global.path(),2,false).unwrap());
        }
        std::fs::create_dir(cwd.path().join(".ara")).unwrap();
        std::fs::write(cwd.path().join(".ara/config.yml"),"task:\n  maxRecursionDepth: 0\n").unwrap();
        assert!(!peer_messaging_enabled(cwd.path(),global.path(),0,true).unwrap());
        std::fs::write(cwd.path().join(".ara/config.yml"),"task:\n  maxRecursionDepth: invalid\n").unwrap();
        assert!(peer_messaging_enabled(cwd.path(),global.path(),0,true).is_err());
        assert!(peer_messaging_enabled(cwd.path(),global.path(),2,true).unwrap());
        assert!(!peer_messaging_enabled(cwd.path(),global.path(),0,false).unwrap());
        let mut journal=ara_session::SessionJournal::create(global.path(),cwd.path()).unwrap();
        let init:ara_session::PersistedChildInit=serde_json::from_value(serde_json::json!({"systemPrompt":[],"task":"branch depth fixture","tools":[],"araChildDepth":2})).unwrap();
        let leaf=journal.append_child_init(&init).unwrap();
        assert_eq!(peer_task_depth(Some(&journal)).unwrap(),2);
        for selected in [None,Some(leaf.as_str())]{
            let fork=journal.fork_at(selected,None).unwrap();
            assert_eq!(peer_branch_task_depth(&journal,selected),peer_task_depth(Some(&fork)).unwrap());
        }
    }
}
