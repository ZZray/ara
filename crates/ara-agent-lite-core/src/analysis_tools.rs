//! Optional Host-selected analysis backends. Targets are snapshotted, never executed.
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolExecutionMode, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{agent::ToolRegistration, process_output::CapturePair};

const RESULT_LIMIT: usize = 16_384;
const SOURCE_LIMIT: u64 = 4 * 1024 * 1024;
const BINARY_LIMIT: u64 = 64 * 1024 * 1024;
const BACKEND_LIMIT: u64 = 128 * 1024 * 1024;
const ROW_LIMIT: usize = 20_000;
const RECORD_LIMIT: usize = 64 * 1024;

/// Paths and retention are owned by the embedding Host. No backend is discovered or installed.
pub struct AnalysisConfig {
    pub cwd: PathBuf,
    pub artifact_root: PathBuf,
    pub ctags: Option<PathBuf>,
    pub objdump: Option<PathBuf>,
    pub nm: Option<PathBuf>,
}

#[derive(Clone)]
struct Backend {
    name: &'static str,
    path: PathBuf,
    sha256: String,
    version: String,
}

struct AnalysisTool {
    schema: Tool,
    cwd: PathBuf,
    artifacts: PathBuf,
    ctags: Option<Backend>,
    objdump: Option<Backend>,
    nm: Option<Backend>,
    languages: HashSet<String>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    if !file.metadata()?.is_file() {
        bail!("input must be a regular file");
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("file exceeds {limit} byte analysis limit");
    }
    Ok(bytes)
}

fn checked_path(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    if path.to_str().is_none() || path.to_string_lossy().len() > 512 {
        bail!("analysis paths must be UTF-8 and at most 512 bytes after resolution");
    }
    Ok(path)
}

fn shorten(text: &str, max: usize) -> (String, bool) {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), end < text.len())
}

struct Output {
    capture: Value,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<i32>,
    error: Option<String>,
}
impl Output {
    fn complete(&self) -> bool {
        self.error.is_none() && self.capture["complete"] == true
    }
}

fn stream_bytes(capture: &Value, stream: &str) -> Result<Vec<u8>> {
    let receipt = &capture[stream];
    let path = receipt["path"].as_str().context("capture stream path missing")?;
    let bytes = read_bounded(Path::new(path), crate::process_output::CAPTURE_LIMIT)?;
    if receipt["stored_bytes"].as_u64() != Some(bytes.len() as u64)
        || receipt["sha256"].as_str() != Some(digest(&bytes).as_str())
    {
        bail!("captured {stream} changed after receipt");
    }
    Ok(bytes)
}

async fn invoke(
    backend: &Backend,
    argv: &[String],
    cwd: &Path,
    capture: CapturePair,
    signal: CancellationToken,
) -> Result<Output> {
    if digest(&read_bounded(&backend.path, BACKEND_LIMIT)?) != backend.sha256 {
        bail!("configured {} backend changed since registration", backend.name);
    }
    let mut cmd = Command::new(&backend.path);
    cmd.args(argv)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("DEBUGINFOD_URLS")
        .env_remove("DEBUGINFOD_CACHE_PATH");
    let result =
        crate::local_tools::run_process(cmd, cwd, signal, Duration::from_secs(10), 1024, true, Some(capture.clone()))
            .await;
    let receipt = capture.receipt();
    let (code, mut error) = match result {
        Ok((_, code)) => (Some(code), None),
        Err(error) => (None, Some(shorten(&error.to_string(), 1024).0)),
    };
    let stdout = match stream_bytes(&receipt, "stdout") {
        Ok(bytes) => bytes,
        Err(e) => {
            error = Some(shorten(&e.to_string(), 1024).0);
            Vec::new()
        }
    };
    let stderr = match stream_bytes(&receipt, "stderr") {
        Ok(bytes) => bytes,
        Err(e) => {
            error = Some(shorten(&e.to_string(), 1024).0);
            Vec::new()
        }
    };
    match read_bounded(&backend.path, BACKEND_LIMIT) {
        Ok(bytes) if digest(&bytes) == backend.sha256 => (),
        _ => error = Some(format!("configured {} backend changed during invocation", backend.name)),
    }
    Ok(Output { capture: receipt, stdout, stderr, code, error })
}

fn new_capture(cwd: &Path, artifacts: &Path) -> Result<CapturePair> {
    let directory = artifacts.join(format!("analysis-{}", crate::fresh_id()));
    CapturePair::create(cwd, directory.to_str().context("artifact path is not UTF-8")?)
}

async fn bind(name: &'static str, path: PathBuf, cwd: &Path, artifacts: &Path) -> Result<Backend> {
    let path = checked_path(&path)?;
    let mut backend =
        Backend { name, sha256: digest(&read_bounded(&path, BACKEND_LIMIT)?), path, version: String::new() };
    let argv =
        if name == "ctags" { vec!["--options=NONE".into(), "--version".into()] } else { vec!["--version".into()] };
    let output = invoke(&backend, &argv, cwd, new_capture(cwd, artifacts)?, CancellationToken::new()).await?;
    if !output.complete() || output.code != Some(0) {
        bail!("{name} version probe failed; capture: {}", output.capture);
    }
    let version = std::str::from_utf8(&output.stdout).context("backend version is not UTF-8")?;
    backend.version = shorten(version.trim(), 1024).0;
    let family = match name {
        "ctags" => "Universal Ctags",
        "nm" => "GNU nm",
        "objdump" => "GNU objdump",
        _ => unreachable!(),
    };
    if !backend.version.starts_with(family) {
        bail!("{name} backend must identify as {family}");
    }
    Ok(backend)
}

/// Register optional analysis tools through RunRequest.extra_tools. Retained files are never cleaned up here.
pub async fn register(config: AnalysisConfig) -> Result<Vec<ToolRegistration>> {
    if config.ctags.is_none() && config.objdump.is_none() && config.nm.is_none() {
        bail!("analysis requires at least one explicitly configured backend");
    }
    let cwd = checked_path(&config.cwd)?;
    let artifact_identity = checked_path(&config.artifact_root)?;
    // Keep the Host's absolute operational spelling. GNU's Windows cwd/file
    // handling does not accept verbatim paths even when Win32 resolves them.
    let artifacts = std::path::absolute(&config.artifact_root)?;
    if artifacts.canonicalize()? != artifact_identity {
        bail!("artifact_root operational path differs from its resolved identity");
    }
    if !cwd.is_dir() || !artifacts.is_dir() {
        bail!("cwd and artifact_root must be existing directories");
    }
    let ctags = match config.ctags {
        Some(path) => Some(bind("ctags", path, &cwd, &artifacts).await?),
        None => None,
    };
    let objdump = match config.objdump {
        Some(path) => Some(bind("objdump", path, &cwd, &artifacts).await?),
        None => None,
    };
    let nm = match config.nm {
        Some(path) => Some(bind("nm", path, &cwd, &artifacts).await?),
        None => None,
    };
    let mut languages = HashSet::new();
    if let Some(backend) = &ctags {
        let features = invoke(
            backend,
            &["--options=NONE".into(), "--list-features".into()],
            &cwd,
            new_capture(&cwd, &artifacts)?,
            CancellationToken::new(),
        )
        .await?;
        if !features.complete()
            || features.code != Some(0)
            || !std::str::from_utf8(&features.stdout)?
                .lines()
                .any(|line| line.split_whitespace().next() == Some("json"))
        {
            bail!("ctags backend requires JSON support; capture: {}", features.capture);
        }
        let output = invoke(
            backend,
            &["--options=NONE".into(), "--list-languages".into()],
            &cwd,
            new_capture(&cwd, &artifacts)?,
            CancellationToken::new(),
        )
        .await?;
        if !output.complete() || output.code != Some(0) {
            bail!("ctags language probe failed; capture: {}", output.capture);
        }
        for line in std::str::from_utf8(&output.stdout)?.lines() {
            let name = line.split_whitespace().next().unwrap_or("");
            if !name.is_empty() && !line.contains("[disabled]") {
                languages.insert(name.to_owned());
            }
        }
        if languages.is_empty() {
            bail!("ctags returned no supported languages");
        }
    }
    let mut tools = Vec::new();
    for source in [true, false] {
        if (source && ctags.is_none()) || (!source && objdump.is_none() && nm.is_none()) {
            continue;
        }
        let mut properties = json!({"path":{"type":"string","minLength":1},"expected_sha256":{"type":["string","null"],"pattern":"^[a-fA-F0-9]{64}$"},"offset":{"type":["integer","null"],"minimum":0},"limit":{"type":["integer","null"],"minimum":1,"maximum":100}});
        properties["expected_page_token"] = json!({"type":["string","null"],"pattern":"^[a-fA-F0-9]{64}$"});
        let (name, description, required) = if source {
            properties["language"] = json!({"type":["string","null"],"minLength":1,"maxLength":64});
            ("source_symbols", "Extract definitions from ONE source snapshot using Host-configured ctags. Returns original-path line numbers and retained snapshot/output receipts; not references, LSP, or whole-project coverage. offset>0 requires expected_sha256 and expected_page_token from page.page_token, binding the input/backend/query. Null optional values are treated as omitted. Unsupported extensions require an explicit backend-supported language. Maximum source 4 MiB; bounded pages may truncate fields explicitly. Parsing retains at most 20000 records, each <=64 KiB; excess is partial.", vec!["path"])
        } else {
            properties["action"] = json!({"type":"string","enum":["headers","symbols","disassemble"]});
            for key in ["start_vma", "stop_vma"] {
                properties[key] = json!({"type":["string","null"],"pattern":"^0x[0-9a-fA-F]{1,16}$"});
            }
            ("binary_inspect", "Inspect ONE binary snapshot with Host-configured GNU objdump/nm, without executing it. action=disassemble requires start_vma and stop_vma (link-time addresses, not file offsets or ASLR addresses), increasing span <=65536. Other actions reject VMA arguments. offset>0 requires expected_sha256 and expected_page_token from page.page_token, binding input/backend/action/VMA. Null optional values are treated as omitted. Maximum binary 64 MiB, 20000 retained lines, 64 KiB per line; excess is partial. Lines are backend text, not recovered source or complete reverse engineering.", vec!["path", "action"])
        };
        tools.push(ToolRegistration { allow_in_plan: true, tool: Arc::new(AnalysisTool {
            schema: Tool { name: name.into(), description: description.into(), parameters: Schema(json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})), constrained_sampling: None },
            cwd: cwd.clone(), artifacts: artifacts.clone(), ctags: ctags.clone(), objdump: objdump.clone(), nm: nm.clone(), languages: languages.clone(),
        }) });
    }
    Ok(tools)
}

fn string_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args[name].as_str().with_context(|| format!("{name} must be a string"))
}

/// Only this analysis tool's optional fields accept null as omission. Required
/// fields and unknown fields remain visible to the existing checks.
fn analysis_arguments(args: &Value, source: bool) -> Value {
    let optional: &[&str] = if source {
        &["language", "expected_sha256", "expected_page_token", "offset", "limit"]
    } else {
        &["expected_sha256", "expected_page_token", "offset", "limit", "start_vma", "stop_vma"]
    };
    let mut normalized = args.clone();
    if let Some(object) = normalized.as_object_mut() {
        for key in optional {
            if object.get(*key).is_some_and(Value::is_null) {
                object.remove(*key);
            }
        }
    }
    normalized
}
fn number_arg(args: &Value, name: &str, default: u64, max: u64) -> Result<u64> {
    match args.get(name) {
        None => Ok(default),
        Some(v) => v.as_u64().filter(|n| *n <= max).with_context(|| format!("{name} must be an integer <= {max}")),
    }
}
fn vma(args: &Value, name: &str) -> Result<u64> {
    let raw = string_arg(args, name)?;
    let hex = raw.strip_prefix("0x").context("VMA must start with 0x")?;
    if hex.is_empty() || hex.len() > 16 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid VMA");
    }
    Ok(u64::from_str_radix(hex, 16)?)
}
fn inferred_language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "c" | "h" => Some("C"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some("C++"),
        "rs" => Some("Rust"),
        "py" | "pyi" => Some("Python"),
        "js" | "jsx" | "mjs" | "cjs" => Some("JavaScript"),
        "ts" | "tsx" => Some("TypeScript"),
        "go" => Some("Go"),
        "java" => Some("Java"),
        "cs" => Some("C#"),
        "rb" => Some("Ruby"),
        "sh" => Some("Sh"),
        "php" => Some("PHP"),
        "swift" => Some("Swift"),
        _ => None,
    }
}

fn parse_symbols(stdout: &[u8], complete: bool) -> Result<(Vec<Value>, bool, bool)> {
    // A quota may end in the middle of JSON/UTF-8. Only complete lines are parseable evidence.
    let retained = if complete || stdout.ends_with(b"\n") {
        stdout
    } else {
        &stdout[..stdout.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1)]
    };
    let text = std::str::from_utf8(retained).context("ctags output is not UTF-8")?;
    let mut symbols = Vec::new();
    let mut truncated = false;
    let mut parsed_all = true;
    for (index, line) in text.lines().filter(|line| !line.trim().is_empty()).enumerate() {
        if index >= ROW_LIMIT || line.len() > RECORD_LIMIT {
            parsed_all = false;
            break;
        }
        let tag: Value = serde_json::from_str(line).context("malformed ctags JSON line")?;
        match tag["_type"].as_str() {
            Some("ptag") => continue,
            Some("tag") => (),
            _ => bail!("unsupported ctags record type"),
        }
        let name = tag["name"].as_str().filter(|s| !s.is_empty()).context("ctags tag missing name")?;
        let kind = tag["kind"].as_str().context("ctags tag missing kind")?;
        let line = tag["line"].as_u64().filter(|n| *n > 0).context("ctags tag missing positive line")?;
        let (name, name_cut) = shorten(name, 1024);
        let (kind, kind_cut) = shorten(kind, 128);
        truncated |= name_cut || kind_cut;
        let mut symbol = json!({"name":name,"kind":kind,"line":line});
        for field in ["scope", "signature"] {
            if let Some(raw) = tag.get(field) {
                let (text, cut) = shorten(raw.as_str().with_context(|| format!("invalid ctags {field}"))?, 1024);
                truncated |= cut;
                symbol[field] = json!(text);
            }
        }
        if let Some(end) = tag.get("end") {
            symbol["end"] = json!(end.as_u64().filter(|n| *n >= line).context("invalid ctags end")?);
        }
        symbols.push(symbol);
    }
    Ok((symbols, truncated, parsed_all))
}

impl AnalysisTool {
    async fn analyze(&self, args: &Value, signal: CancellationToken) -> Result<Value> {
        let source = self.schema.name == "source_symbols";
        let normalized = analysis_arguments(args, source);
        let args = &normalized;
        let allowed: &[&str] = if source {
            &["path", "language", "expected_sha256", "expected_page_token", "offset", "limit"]
        } else {
            &["path", "action", "expected_sha256", "expected_page_token", "offset", "limit", "start_vma", "stop_vma"]
        };
        let object = args.as_object().context("arguments must be an object")?;
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            bail!("unknown analysis argument");
        }
        let raw = string_arg(args, "path")?;
        if raw.is_empty() || raw.len() > 4096 || raw.contains('\0') {
            bail!("invalid input path");
        }
        let supplied = Path::new(raw);
        let original =
            checked_path(&if supplied.is_absolute() { supplied.to_owned() } else { self.cwd.join(supplied) })?;
        let offset = number_arg(args, "offset", 0, usize::MAX as u64)? as usize;
        let limit = number_arg(args, "limit", if source { 30 } else { 40 }, 100)? as usize;
        if limit == 0 {
            bail!("limit must be >=1");
        }
        let expected =
            args.get("expected_sha256").map(|v| v.as_str().context("expected_sha256 must be a string")).transpose()?;
        if expected.is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit())) {
            bail!("invalid expected_sha256");
        }
        if offset > 0 && expected.is_none() {
            bail!("offset>0 requires expected_sha256");
        }
        let expected_page = args
            .get("expected_page_token")
            .map(|v| v.as_str().context("expected_page_token must be a string"))
            .transpose()?;
        if expected_page.is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit())) {
            bail!("invalid expected_page_token");
        }
        if offset > 0 && expected_page.is_none() {
            bail!("offset>0 requires expected_page_token");
        }
        let action = if source { "definitions" } else { string_arg(args, "action")? };
        let mut selected_language = None;
        let mut argv;
        let backend = if source {
            let language = match args.get("language") {
                Some(v) => v.as_str().context("language must be a string")?,
                None => inferred_language(&original).context("unsupported extension: specify a supported language")?,
            };
            if !self.languages.contains(language) {
                bail!("unsupported ctags language: {language}");
            }
            selected_language = Some(language);
            // Keep file-scope definitions (for example static C functions), omit reference/qualified extras.
            argv = vec![
                "--options=NONE".into(),
                "--output-format=json".into(),
                "--sort=no".into(),
                "--fields=+nKseS".into(),
                "--extras=F".into(),
                format!("--language-force={language}"),
                "-f".into(),
                "-".into(),
            ];
            self.ctags.as_ref().context("ctags is not configured")?
        } else {
            if action != "disassemble" && (args.get("start_vma").is_some() || args.get("stop_vma").is_some()) {
                bail!("VMA arguments require disassemble");
            }
            match action {
                "headers" => {
                    argv = vec!["-f".into(), "-h".into(), "-p".into()];
                    self.objdump.as_ref().context("headers requires objdump")?
                }
                "symbols" => {
                    if let Some(nm) = &self.nm {
                        argv = vec![
                            "--format=posix".into(),
                            "--no-demangle".into(),
                            "--print-size".into(),
                            "--numeric-sort".into(),
                        ];
                        nm
                    } else {
                        argv = vec!["-t".into()];
                        self.objdump.as_ref().context("symbols requires nm or objdump")?
                    }
                }
                "disassemble" => {
                    let start = vma(args, "start_vma")?;
                    let stop = vma(args, "stop_vma")?;
                    if stop <= start || stop - start > 65536 {
                        bail!("disassembly must have increasing VMA span <=65536");
                    }
                    argv =
                        vec!["-d".into(), format!("--start-address=0x{start:x}"), format!("--stop-address=0x{stop:x}")];
                    self.objdump.as_ref().context("disassemble requires objdump")?
                }
                _ => bail!("unsupported binary action"),
            }
        };
        let bytes = read_bounded(&original, if source { SOURCE_LIMIT } else { BINARY_LIMIT })?;
        let sha = digest(&bytes);
        if expected.is_some_and(|s| !s.eq_ignore_ascii_case(&sha)) {
            bail!("input SHA256 differs from expected_sha256");
        }
        let page_token = digest(&serde_json::to_vec(&json!({"input_sha256":sha,"path":original,
            "backend_sha256":backend.sha256,"backend_path":backend.path,"backend_version":backend.version,
            "argv":argv}))?);
        if expected_page.is_some_and(|s| !s.eq_ignore_ascii_case(&page_token)) {
            bail!("analysis query/backend differs from expected_page_token");
        }
        let capture = new_capture(&self.cwd, &self.artifacts)?;
        let receipt = capture.receipt();
        let directory = Path::new(receipt["directory"].as_str().context("capture directory missing")?);
        let operational_directory =
            self.artifacts.join(directory.file_name().context("capture directory name missing")?);
        if operational_directory.canonicalize()? != directory {
            bail!("analysis operational directory differs from captured directory identity");
        }
        // Fixed ASCII operands also work with backends that do not understand
        // Windows verbatim paths. The process runs in this exact snapshot dir;
        // source language is explicit and evidence keeps the canonical paths.
        let operand = if source { "input.source" } else { "input.bin" };
        let snapshot = directory.join(operand);
        // A new private artifact directory prevents colliding with another call's input.
        use std::io::Write;
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&snapshot)?;
        file.write_all(&bytes)?;
        file.flush()?;
        drop(file);
        if digest(&read_bounded(&snapshot, if source { SOURCE_LIMIT } else { BINARY_LIMIT })?) != sha {
            bail!("input snapshot verification failed");
        }
        argv.push(operand.to_owned());
        let mut output = match invoke(backend, &argv, &operational_directory, capture.clone(), signal).await {
            Ok(output) => output,
            Err(error) => Output {
                capture: capture.receipt(),
                stdout: Vec::new(),
                stderr: Vec::new(),
                code: None,
                error: Some(shorten(&error.to_string(), 1024).0),
            },
        };
        match read_bounded(&snapshot, if source { SOURCE_LIMIT } else { BINARY_LIMIT }) {
            Ok(bytes) if digest(&bytes) == sha => (),
            _ => output.error = Some("input snapshot changed during analysis".into()),
        }
        let complete = output.complete();
        let mut result = json!({"schema":"ara.analysis.v1","tool":self.schema.name,"status":"failed",
            "input":{"path":original,"sha256":sha,"bytes":bytes.len(),"snapshot_path":snapshot},
            "backend":{"name":backend.name,"version":backend.version,"path":backend.path,"sha256":backend.sha256},
            "action":action,"analysis":{"complete":complete,"backend_complete":complete,"capture":output.capture,"exit_code":output.code},
            "page":{"page_token":page_token,"offset":offset,"returned":0,"limit":limit,"next_offset":null,"total":null,"complete":false}});
        if let Some(language) = selected_language {
            result["language"] = json!(language);
        }
        if let Some(error) = output.error {
            result["analysis"]["complete"] = json!(false);
            result["analysis"]["error"] = json!(error);
            return Ok(result);
        }
        let stderr = match std::str::from_utf8(&output.stderr) {
            Ok(text) => text,
            Err(_) => {
                result["analysis"]["complete"] = json!(false);
                result["analysis"]["error"] = json!("backend stderr is not UTF-8");
                return Ok(result);
            }
        };
        if !stderr.is_empty() {
            result["analysis"]["diagnostic"] = json!(shorten(stderr, 512).0);
        }
        let no_symbols = backend.name == "nm"
            && output.code == Some(1)
            && output.stdout.is_empty()
            && stderr.lines().count() == 1
            && stderr.trim_end().ends_with(": no symbols");
        if output.code != Some(0) && !no_symbols {
            result["analysis"]["complete"] = json!(false);
            result["analysis"]["error"] = json!("backend exited unsuccessfully");
            return Ok(result);
        }
        let parsed = if source {
            parse_symbols(&output.stdout, complete)
        } else {
            std::str::from_utf8(&output.stdout).context("binary output is not UTF-8").map(|text| {
                let mut cut = false;
                let mut parsed_all = true;
                let mut lines = Vec::new();
                for (index, line) in text.lines().enumerate() {
                    if index >= ROW_LIMIT || line.len() > RECORD_LIMIT {
                        parsed_all = false;
                        break;
                    }
                    let (line, truncated) = shorten(line, 2048);
                    cut |= truncated;
                    lines.push(json!(line));
                }
                (lines, cut, parsed_all)
            })
        };
        let (rows, fields_truncated, parsed_all) = match parsed {
            Ok(rows) => rows,
            Err(error) => {
                result["analysis"]["complete"] = json!(false);
                result["analysis"]["error"] = json!(shorten(&error.to_string(), 1024).0);
                return Ok(result);
            }
        };
        let complete = complete && parsed_all;
        result["analysis"]["complete"] = json!(complete);
        result["analysis"]["parse_limit_reached"] = json!(!parsed_all);
        if offset > rows.len() {
            result["analysis"]["complete"] = json!(false);
            result["analysis"]["error"] = json!("offset exceeds retained rows");
            return Ok(result);
        }
        let key = if source { "symbols" } else { "lines" };
        let mut returned = limit.min(rows.len() - offset);
        loop {
            let end = offset + returned;
            result[key] = json!(&rows[offset..end]);
            result["status"] = json!(if complete && !fields_truncated { "completed" } else { "partial" });
            result["page"] = json!({"page_token":page_token,"offset":offset,"returned":returned,"limit":limit,"next_offset":if end < rows.len() {Some(end)} else {None},"total":if complete {Some(rows.len())} else {None},"retained_rows":rows.len(),"complete":complete && end == rows.len(),"fields_truncated":fields_truncated});
            if returned == 0 && offset < rows.len() {
                result["status"] = json!("failed");
                result["analysis"]["complete"] = json!(false);
                result["analysis"]["error"] = json!("one analysis record cannot fit result byte budget");
                result["page"]["next_offset"] = Value::Null;
                return Ok(result);
            }
            if serde_json::to_vec(&result)?.len() <= RESULT_LIMIT {
                break;
            }
            if returned == 0 {
                bail!("analysis metadata exceeds result byte budget");
            }
            returned -= 1;
        }
        Ok(result)
    }
}

#[async_trait]
impl AgentTool for AnalysisTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let result = self.analyze(&args, signal.clone()).await;
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                json!({"schema":"ara.analysis.v1","tool":self.schema.name,"status":"failed","error":shorten(&error.to_string(), 2048).0})
            }
        };
        let failed = value["status"] == "failed";
        let text = serde_json::to_string(&value).map_err(|e| AgentError::Tool(e.to_string()))?;
        if text.len() > RESULT_LIMIT {
            return Err(AgentError::Tool("analysis failure metadata exceeds byte budget".into()));
        }
        if failed {
            Err(AgentError::Tool(text))
        } else {
            Ok(AgentToolResult::text(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_null_normalization_preserves_required_unknown_and_nonnull_fields() {
        let binary = json!({"path":null,"action":null,"expected_sha256":null,"expected_page_token":null,
            "offset":null,"limit":null,"start_vma":null,"stop_vma":null,"language":null,"unknown":null});
        assert_eq!(
            analysis_arguments(&binary, false),
            json!({"path":null,"action":null,"language":null,"unknown":null})
        );
        let source = json!({"path":null,"language":null,"expected_sha256":null,"expected_page_token":null,
            "offset":null,"limit":null,"action":null,"start_vma":null,"unknown":null});
        assert_eq!(
            analysis_arguments(&source, true),
            json!({"path":null,"action":null,"start_vma":null,"unknown":null})
        );
        assert!(binary.get("offset").is_some(), "normalization must not change the original receipt");
        let nonnull = json!({"path":"fixture.c","action":"headers","language":"C","offset":0,"limit":100,
            "expected_sha256":"0".repeat(64),"expected_page_token":"0".repeat(64),"start_vma":"0x0","stop_vma":"0x0"});
        assert_eq!(analysis_arguments(&nonnull, false), nonnull);
        assert_eq!(analysis_arguments(&nonnull, true), nonnull);
    }

    struct Input(PathBuf);
    impl Input {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ara-analysis-null-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("fixture.c"), b"int fixture(void) { return 1; }\n").unwrap();
            Self(root)
        }
        fn tool(&self, source: bool, configured: bool) -> AnalysisTool {
            let backend = || Backend {
                name: "unit-unavailable",
                path: self.0.join("absent-backend"),
                sha256: "0".repeat(64),
                version: "unit fixture; never executed".into(),
            };
            AnalysisTool {
                schema: Tool {
                    name: (if source { "source_symbols" } else { "binary_inspect" }).into(),
                    description: String::new(),
                    parameters: Schema::empty_object(),
                    constrained_sampling: None,
                },
                cwd: self.0.clone(),
                artifacts: self.0.clone(),
                ctags: (source && configured).then(backend),
                objdump: (!source && configured).then(backend),
                nm: None,
                languages: HashSet::from(["C".into()]),
            }
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[tokio::test]
    async fn analysis_optional_nulls_reach_the_same_backend_admission_as_omission() {
        let input = Input::new();
        for source in [true, false] {
            // No backend is configured: the same downstream admission error
            // proves both forms passed argument checks, without a process.
            let tool = input.tool(source, false);
            let omitted =
                if source { json!({"path":"fixture.c"}) } else { json!({"path":"fixture.c","action":"headers"}) };
            let mut nullable = omitted.clone();
            for key in if source {
                &["language", "expected_sha256", "expected_page_token", "offset", "limit"][..]
            } else {
                &["expected_sha256", "expected_page_token", "offset", "limit", "start_vma", "stop_vma"][..]
            } {
                nullable[*key] = Value::Null;
            }
            let expected = tool.analyze(&omitted, CancellationToken::new()).await.unwrap_err().to_string();
            assert!(expected.contains(if source { "ctags is not configured" } else { "headers requires objdump" }));
            assert_eq!(tool.analyze(&nullable, CancellationToken::new()).await.unwrap_err().to_string(), expected);
        }
    }

    #[tokio::test]
    async fn analysis_nullable_arguments_keep_existing_prechecks() {
        let input = Input::new();
        let binary = input.tool(false, true);
        for (args, error) in [
            (json!({"path":null,"action":"headers"}), "path must be a string"),
            (json!({"path":"fixture.c","action":null}), "action must be a string"),
            (json!({"path":"fixture.c","action":"headers","unknown":null}), "unknown analysis argument"),
            (json!({"path":"fixture.c","action":"headers","language":null}), "unknown analysis argument"),
            (
                json!({"path":"fixture.c","action":"headers","start_vma":"0x0","stop_vma":"0x0"}),
                "VMA arguments require disassemble",
            ),
            (json!({"path":"fixture.c","action":"symbols","start_vma":"0x0"}), "VMA arguments require disassemble"),
            (
                json!({"path":"fixture.c","action":"disassemble","start_vma":null,"stop_vma":null}),
                "start_vma must be a string",
            ),
            (
                json!({"path":"fixture.c","action":"disassemble","start_vma":"0x0","stop_vma":"0x0"}),
                "increasing VMA span",
            ),
            (
                json!({"path":"fixture.c","action":"disassemble","start_vma":"0x0","stop_vma":"0x10001"}),
                "increasing VMA span",
            ),
            (
                json!({"path":"fixture.c","action":"headers","offset":1,"expected_sha256":null}),
                "offset>0 requires expected_sha256",
            ),
            (
                json!({"path":"fixture.c","action":"headers","offset":1,"expected_sha256":"0".repeat(64),"expected_page_token":null}),
                "offset>0 requires expected_page_token",
            ),
            (json!({"path":"fixture.c","action":"headers","expected_sha256":"0".repeat(64)}), "input SHA256 differs"),
            (
                json!({"path":"fixture.c","action":"headers","expected_page_token":"0".repeat(64)}),
                "analysis query/backend differs",
            ),
            (json!({"path":"fixture.c","action":"headers","limit":0}), "limit must be >=1"),
        ] {
            assert!(
                binary.analyze(&args, CancellationToken::new()).await.unwrap_err().to_string().contains(error),
                "{args}"
            );
        }
        let source = input.tool(true, true);
        for (args, error) in [
            (json!({"path":null,"language":null}), "path must be a string"),
            (json!({"path":"fixture.c","start_vma":null}), "unknown analysis argument"),
            (json!({"path":"fixture.c","language":"unsupported"}), "unsupported ctags language"),
            (
                json!({"path":"fixture.c","language":null,"offset":1,"expected_sha256":null}),
                "offset>0 requires expected_sha256",
            ),
            (json!({"path":"fixture.c","language":null,"expected_sha256":"0".repeat(64)}), "input SHA256 differs"),
        ] {
            assert!(
                source.analyze(&args, CancellationToken::new()).await.unwrap_err().to_string().contains(error),
                "{args}"
            );
        }
        assert_eq!(fs::read(input.0.join("fixture.c")).unwrap(), b"int fixture(void) { return 1; }\n");
        assert_eq!(fs::read_dir(&input.0).unwrap().count(), 1, "prechecks must not create backend artifacts");
    }

    #[test]
    fn parses_definitions_and_rejects_malformed_records() {
        let bytes = b"{\"_type\":\"tag\",\"name\":\"hello\",\"kind\":\"function\",\"line\":2,\"end\":4}\n";
        let (rows, truncated, parsed_all) = parse_symbols(bytes, true).unwrap();
        assert_eq!(rows[0]["line"], 2);
        assert!(!truncated);
        assert!(parsed_all);
        assert!(parse_symbols(b"{\"_type\":\"tag\",\"name\":\"x\"}\n", true).is_err());
        assert!(parse_symbols(b"not json\n", true).is_err());
        assert!(parse_symbols(b"\xff\n", true).is_err());
    }
    #[test]
    fn partial_json_tail_is_not_a_definition() {
        let (rows, _, _) = parse_symbols(b"{\"_type\":\"tag\",\"name\":", false).unwrap();
        assert!(rows.is_empty());
        assert!(parse_symbols(b"{\"_type\":\"tag\",\"name\":", true).is_err());
    }
    #[test]
    fn record_and_count_caps_are_explicitly_partial() {
        let oversized = vec![b'x'; RECORD_LIMIT + 1];
        let (rows, _, parsed_all) = parse_symbols(&oversized, true).unwrap();
        assert!(rows.is_empty());
        assert!(!parsed_all);
        let many = "{\"_type\":\"ptag\"}\n".repeat(ROW_LIMIT + 1);
        let (_, _, parsed_all) = parse_symbols(many.as_bytes(), true).unwrap();
        assert!(!parsed_all);
    }
    #[test]
    fn validates_vma_and_utf8_budgets() {
        assert_eq!(vma(&json!({"v":"0x1f"}), "v").unwrap(), 31);
        for value in ["1f", "0x", "0x-1", "0x10000000000000000"] {
            assert!(vma(&json!({"v":value}), "v").is_err());
        }
        assert_eq!(shorten("中文", 4), ("中".into(), true));
    }
}
