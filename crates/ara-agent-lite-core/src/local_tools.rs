//! Small host tools. OS account permissions govern file and shell access.
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolExecutionMode, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::SeekFrom,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 128 * 1024;
const FILE_LIMIT: usize = 1024 * 1024;
const DOWNLOAD_LIMIT: u64 = 128 * 1024 * 1024;

pub fn tools(root: PathBuf) -> Vec<Arc<dyn AgentTool>> {
    [
        ("read_file", "Read UTF-8 text. Path alone returns the original complete text (maximum 1 MiB). Optional offset (1-based) and limit select numbered lines; tail selects last N lines, with line_from_end rather than absolute line numbers. Explicit selections have scan/output limits and report partial coverage. Absolute paths are allowed.", json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":2000},"tail":{"type":"integer","minimum":1,"maximum":2000}}), vec!["path"]),
        ("search_files", "Search a non-empty single-line literal in UTF-8 files, recursively from path (default cwd). case_sensitive defaults to true; false uses Unicode lowercase matching. Skips generated directories, links and non-text files. Reports bounded matches and partial coverage; zero matches is not proof about skipped files. Absolute paths are allowed.", json!({"query":{"type":"string","minLength":1},"path":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":1000},"case_sensitive":{"type":"boolean"}}), vec!["query"]),
        ("write_file", "Write UTF-8 text. Relative paths use the working directory; absolute paths are allowed. Parent directories are created.", json!({"path":{"type":"string"},"content":{"type":"string"}}), vec!["path", "content"]),
        ("edit_file", "Replace exactly one matching text occurrence in a file. Absolute paths are allowed.", json!({"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}}), vec!["path", "old_text", "new_text"]),
        ("list_dir", "List directory entries; use search_files for bounded recursive text searches. Absolute paths are allowed.", json!({"path":{"type":"string"}}), vec!["path"]),
        ("download_file", "Download an HTTP/HTTPS file, respecting the configured HTTP proxy. Maximum 128 MiB, 300-second deadline. Returns SHA256; optional expected_sha256 must match before replacing the destination. Existing destination is preserved on transfer or checksum failure. Download does not install or execute the file.", json!({"url":{"type":"string"},"path":{"type":"string"},"expected_sha256":{"type":"string","pattern":"^[a-fA-F0-9]{64}$"}}), vec!["url", "path"]),
        ("exec", "Run a shell command. Windows uses PowerShell; other systems use sh. This is not an OS sandbox. Output is bounded; timeout defaults to 600 seconds, limited by the overall run deadline.", json!({"cmd":{"type":"string"},"cwd":{"type":"string"},"timeout":{"type":"integer","minimum":1,"maximum":3600}}), vec!["cmd"]),
    ].into_iter().map(|(name, description, properties, required)| {
        Arc::new(LocalTool { root: root.clone(), schema: Tool {
            name: name.into(), description: description.into(),
            parameters: Schema(json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})),
            constrained_sampling: None,
        }}) as Arc<dyn AgentTool>
    }).collect()
}

struct LocalTool {
    root: PathBuf,
    schema: Tool,
}

fn argument<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("missing string argument: {name}"))
}

/// Relative paths are based on cwd; this is not a confinement boundary.
fn rooted(root: &Path, value: &str, new_file: bool) -> Result<PathBuf> {
    let input = Path::new(value);
    let candidate = if input.is_absolute() { input.to_owned() } else { root.join(input) };
    let resolved = if new_file && !candidate.exists() {
        let name = candidate.file_name().context("file name required")?;
        let parent = candidate.parent().context("parent required")?;
        std::fs::create_dir_all(parent)?;
        parent.canonicalize()?.join(name)
    } else {
        candidate.canonicalize()?
    };
    Ok(resolved)
}

fn read_text(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > FILE_LIMIT as u64 {
        bail!("file exceeds 1 MiB text limit");
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        bail!("file exceeds 1 MiB text limit");
    }
    String::from_utf8(bytes).context("file is not UTF-8 text")
}

const SCAN_BYTES: usize = 32 * 1024 * 1024;
const LINE_PREFIX: usize = 8 * 1024;
const SCAN_TIME: Duration = Duration::from_secs(5);

fn positive(args: &Value, key: &str, maximum: u64) -> Result<Option<u64>> {
    args.get(key)
        .map(|value| {
            let number = value
                .as_u64()
                .filter(|n| *n > 0 && *n <= maximum)
                .ok_or_else(|| anyhow!("{key} must be a positive integer no greater than {maximum}"))?;
            Ok(number)
        })
        .transpose()
}

#[derive(Clone, Copy)]
enum ReadSelection {
    Range { offset: u64, limit: usize },
    Tail(usize),
}
impl ReadSelection {
    fn parse(args: &Value) -> Result<Option<Self>> {
        let offset = positive(args, "offset", u64::MAX)?;
        let limit = positive(args, "limit", 2000)?;
        let tail = positive(args, "tail", 2000)?;
        if let Some(tail) = tail {
            if offset.is_some() || limit.is_some() {
                bail!("tail cannot be combined with offset or limit");
            }
            return Ok(Some(Self::Tail(tail as usize)));
        }
        if offset.is_some() || limit.is_some() {
            return Ok(Some(Self::Range { offset: offset.unwrap_or(1), limit: limit.unwrap_or(200) as usize }));
        }
        Ok(None)
    }
}

struct ScannedLine {
    bytes: Vec<u8>,
    truncated: bool,
}
struct LineScanner {
    input: BufReader<tokio::io::Take<tokio::fs::File>>,
    bytes: usize,
    available: u64,
    cap: usize,
    prefix: usize,
    started: Instant,
    signal: CancellationToken,
    scan_limit: bool,
    time_limit: bool,
    eof: bool,
}
impl LineScanner {
    fn new(
        file: tokio::fs::File,
        available: u64,
        cap: usize,
        prefix: usize,
        started: Instant,
        signal: CancellationToken,
    ) -> Self {
        Self {
            input: BufReader::with_capacity(8192, file.take(cap as u64)),
            bytes: 0,
            available,
            cap,
            prefix,
            started,
            signal,
            scan_limit: false,
            time_limit: false,
            eof: false,
        }
    }
    async fn next(&mut self) -> Result<Option<ScannedLine>> {
        let mut stored = Vec::new();
        let mut length = 0usize;
        let mut complete_line = false;
        loop {
            if self.signal.is_cancelled() {
                bail!("file scan canceled");
            }
            if self.started.elapsed() >= SCAN_TIME {
                self.time_limit = true;
                break;
            }
            let chunk = self.input.fill_buf().await?;
            if self.signal.is_cancelled() {
                bail!("file scan canceled");
            }
            if chunk.is_empty() {
                self.scan_limit = self.bytes >= self.cap && (self.bytes as u64) < self.available;
                self.eof = !self.scan_limit;
                break;
            }
            let count = chunk.iter().position(|b| *b == b'\n').map(|n| n + 1).unwrap_or(chunk.len());
            let take = count.min(self.prefix.saturating_sub(stored.len()));
            stored.extend_from_slice(&chunk[..take]);
            complete_line = chunk[count - 1] == b'\n';
            self.bytes += count;
            length += count;
            self.input.consume(count);
            if complete_line {
                break;
            }
        }
        if length == 0 {
            return Ok(None);
        }
        let truncated = stored.len() < length || (!complete_line && !self.eof);
        if stored.last() == Some(&b'\n') {
            stored.pop();
            if stored.last() == Some(&b'\r') {
                stored.pop();
            }
        }
        Ok(Some(ScannedLine { bytes: stored, truncated }))
    }
}

fn line_text(line: &ScannedLine) -> Result<String> {
    match std::str::from_utf8(&line.bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(error) if line.truncated && error.error_len().is_none() => {
            Ok(std::str::from_utf8(&line.bytes[..error.valid_up_to()])?.to_owned())
        }
        Err(_) => bail!("selected line is not UTF-8 text"),
    }
}

// Values remain bounded while scanning, then exact JSON size is checked once.
fn render_report(mut report: Value, array: &str, signal: &CancellationToken, started: Instant) -> Result<String> {
    if signal.is_cancelled() {
        bail!("file scan canceled during rendering");
    }
    if started.elapsed() >= SCAN_TIME {
        report["coverage"]["time_limit_hit"] = json!(true);
        report["coverage"]["complete"] = json!(false);
    }
    let mut text = serde_json::to_string(&report)?;
    if text.len() > OUTPUT_LIMIT {
        // Collection already budgets encoded rows. This is one bounded fallback
        // for unusually large metadata, never a repeated serialize/pop loop.
        report[array] = json!([]);
        report["coverage"]["output_truncated"] = json!(true);
        report["coverage"]["complete"] = json!(false);
        text = serde_json::to_string(&report)?;
    }
    if signal.is_cancelled() {
        bail!("file scan canceled during rendering");
    }
    if started.elapsed() >= SCAN_TIME && report["coverage"]["time_limit_hit"] != true {
        report["coverage"]["time_limit_hit"] = json!(true);
        report["coverage"]["complete"] = json!(false);
        text = serde_json::to_string(&report)?;
    }
    if text.len() > OUTPUT_LIMIT {
        bail!("scan metadata exceeds output limit");
    }
    Ok(text)
}

async fn read_selection(path: &Path, selection: ReadSelection, signal: CancellationToken) -> Result<String> {
    let started = Instant::now();
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let start = if matches!(selection, ReadSelection::Tail(_)) { size.saturating_sub(SCAN_BYTES as u64) } else { 0 };
    file.seek(SeekFrom::Start(start)).await?;
    let data_budget =
        OUTPUT_LIMIT.saturating_sub(serde_json::to_vec(&json!({"path":path.to_string_lossy()}))?.len() + 4096);
    let mut scan = LineScanner::new(file, size - start, SCAN_BYTES, LINE_PREFIX, started, signal.clone());
    let mut lines = VecDeque::<(Value, usize)>::new();
    let mut line_number = 0u64;
    let mut output_bytes = 0usize;
    let mut output_truncated = false;
    let mut line_truncated = false;
    let mut discarded_prefix = false;
    let tail = matches!(selection, ReadSelection::Tail(_));
    while let Some(line) = scan.next().await? {
        line_number += 1;
        if tail && start > 0 && !discarded_prefix {
            discarded_prefix = true;
            continue;
        }
        if let ReadSelection::Range { offset, .. } = selection {
            if line_number < offset {
                continue;
            }
        }
        let text = line_text(&line)?;
        let row =
            json!({"line":if tail {Value::Null} else {json!(line_number)},"text":text,"truncated":line.truncated});
        let row_bytes = serde_json::to_vec(&row)?.len() + 1 + if tail { 32 } else { 0 };
        if let ReadSelection::Tail(limit) = selection {
            while !lines.is_empty() && (lines.len() >= limit || output_bytes + row_bytes > data_budget) {
                let (_, cost) = lines.pop_front().unwrap();
                output_bytes -= cost;
                if lines.len() + 1 < limit {
                    output_truncated = true;
                }
            }
            if row_bytes > data_budget {
                output_truncated = true;
                continue;
            }
        } else if output_bytes + row_bytes > data_budget {
            output_truncated = true;
            break;
        }
        output_bytes += row_bytes;
        lines.push_back((row, row_bytes));
        if let ReadSelection::Range { limit, .. } = selection {
            if lines.len() >= limit {
                break;
            }
        }
    }
    // Never label a timed-out forward prefix of the tail window as EOF lines.
    if tail && !scan.eof {
        lines.clear();
        output_truncated = true;
    }
    let enough_tail = match selection {
        ReadSelection::Tail(count) => lines.len() >= count || start == 0,
        _ => true,
    };
    let count = lines.len();
    for (index, (line, _)) in lines.iter_mut().enumerate() {
        line_truncated |= line["truncated"] == true;
        if tail {
            line["line_from_end"] = json!(count - index);
        }
    }
    let lines: Vec<Value> = lines.into_iter().map(|(row, _)| row).collect();
    render_report(
        json!({"path":path.to_string_lossy(),"mode":if tail {"tail"}else{"range"},"lines":lines,
        "coverage":{"scope":if tail {"requested last lines; absolute line numbers unknown"}else{"requested range only"},
        "complete":!scan.scan_limit && !scan.time_limit && !output_truncated && !line_truncated && enough_tail,
        "scan_limit_hit":scan.scan_limit,"time_limit_hit":scan.time_limit,"soft_elapsed_budget_seconds":5,"output_truncated":output_truncated,
        "line_truncated":line_truncated,"bytes_scanned":scan.bytes,"scan_limit_bytes":SCAN_BYTES,
        "snapshot_file_bytes":size,"window_start_byte":start,"prefix_line_discarded":discarded_prefix,
        "absolute_line_numbers":!tail,"path_display_lossy":path.to_str().is_none(),"has_more_file_bytes":start>0 || (scan.bytes as u64)<size}}),
        "lines",
        &signal,
        started,
    )
}

#[derive(Clone, Copy)]
struct SearchLimits {
    depth: usize,
    dirs: usize,
    files: usize,
    entries: usize,
    per_file: usize,
    total: usize,
}
impl Default for SearchLimits {
    fn default() -> Self {
        Self { depth: 16, dirs: 4096, files: 4096, entries: 16384, per_file: 4 * 1024 * 1024, total: SCAN_BYTES }
    }
}
fn link_like(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
fn generated_directory(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        matches!(
            name.to_string_lossy().to_ascii_lowercase().as_str(),
            ".git" | "node_modules" | "target" | "dist" | "build" | "out" | ".venv" | "venv" | "__pycache__" | ".cache"
        )
    })
}

async fn search_files(root: &Path, args: &Value, signal: CancellationToken, limits: SearchLimits) -> Result<String> {
    let query = argument(args, "query")?;
    if query.is_empty() || query.contains(['\n', '\r']) || query.len() > 4096 {
        bail!("query must be a non-empty single-line literal of at most 4096 bytes");
    }
    let sensitive = match args.get("case_sensitive") {
        None => true,
        Some(v) => v.as_bool().context("case_sensitive must be a boolean")?,
    };
    let max_results = positive(args, "max_results", 1000)?.unwrap_or(100) as usize;
    let path_argument = match args.get("path") {
        None => ".",
        Some(v) => v.as_str().context("path must be a string")?,
    };
    let original =
        if Path::new(path_argument).is_absolute() { PathBuf::from(path_argument) } else { root.join(path_argument) };
    let start = Instant::now();
    let data_budget = OUTPUT_LIMIT
        .saturating_sub(serde_json::to_vec(&json!({"path":original.to_string_lossy(),"query":query}))?.len() + 4096);
    let mut pending = VecDeque::from([(original.clone(), 0)]);
    let mut matches = Vec::new();
    let (mut dirs, mut files, mut entries, mut scanned, mut searched, mut output_bytes) = (0, 0, 0, 0, 0, 0);
    let (mut links, mut generated, mut non_files, mut binary, mut invalid_utf8, mut errors, mut limited_files) =
        (0, 0, 0, 0, 0, 0, 0);
    let (mut depth_cut, mut budget_cut, mut time_cut, mut output_cut, mut results_cut) =
        (false, false, false, false, false);
    let mut non_utf8_paths = 0;
    let needle = if sensitive { query.to_owned() } else { query.to_lowercase() };
    while let Some((path, depth)) = pending.pop_front() {
        if path.to_str().is_none() {
            non_utf8_paths += 1;
            continue;
        }
        if signal.is_cancelled() {
            bail!("file search canceled");
        }
        if start.elapsed() >= SCAN_TIME {
            time_cut = true;
            break;
        }
        if scanned >= limits.total {
            budget_cut = true;
            break;
        }
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(m) => m,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        if link_like(&metadata) {
            links += 1;
            continue;
        }
        if metadata.is_dir() {
            if generated_directory(&path) {
                generated += 1;
                continue;
            }
            if depth >= limits.depth {
                depth_cut = true;
                continue;
            }
            if dirs >= limits.dirs {
                budget_cut = true;
                break;
            }
            dirs += 1;
            let mut directory = match tokio::fs::read_dir(&path).await {
                Ok(d) => d,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            loop {
                if signal.is_cancelled() {
                    bail!("file search canceled");
                }
                if start.elapsed() >= SCAN_TIME {
                    time_cut = true;
                    break;
                }
                if entries >= limits.entries || pending.len() >= limits.entries {
                    budget_cut = true;
                    break;
                }
                match directory.next_entry().await {
                    Ok(Some(entry)) => {
                        entries += 1;
                        pending.push_back((entry.path(), depth + 1));
                    }
                    Ok(None) => break,
                    Err(_) => {
                        errors += 1;
                        break;
                    }
                }
            }
            if budget_cut || time_cut {
                break;
            }
            continue;
        }
        if !metadata.is_file() {
            non_files += 1;
            continue;
        }
        if files >= limits.files {
            budget_cut = true;
            break;
        }
        files += 1;
        let file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        let cap = limits.per_file.min(limits.total - scanned);
        let mut scan = LineScanner::new(file, metadata.len(), cap, cap, start, signal.clone());
        let mut number = 0u64;
        let mut file_limited = false;
        loop {
            let line = match scan.next().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    if signal.is_cancelled() {
                        return Err(e);
                    }
                    errors += 1;
                    break;
                }
            };
            number += 1;
            if line.bytes.contains(&0) {
                binary += 1;
                break;
            }
            let text = match line_text(&line) {
                Ok(t) => t,
                Err(_) => {
                    invalid_utf8 += 1;
                    break;
                }
            };
            searched += line.bytes.len();
            if line.truncated {
                file_limited = true;
            }
            if let Some((excerpt, offset, truncated)) = match_excerpt(&text, &needle, sensitive, 1024) {
                let row = json!({"path":path,"line":number,"excerpt":excerpt,
                    "excerpt_byte_offset":offset,"excerpt_truncated":truncated,"line_truncated":line.truncated});
                let bytes = serde_json::to_vec(&row)?.len() + 1;
                if output_bytes + bytes > data_budget {
                    output_cut = true;
                    break;
                }
                output_bytes += bytes;
                matches.push(row);
                if matches.len() >= max_results {
                    results_cut = true;
                    break;
                }
            }
        }
        scanned += scan.bytes;
        if scan.scan_limit {
            file_limited = true;
        }
        if file_limited {
            limited_files += 1;
        }
        if scan.time_limit {
            time_cut = true;
        }
        if output_cut || results_cut || time_cut {
            break;
        }
    }
    let complete = pending.is_empty()
        && !depth_cut
        && !budget_cut
        && !time_cut
        && !output_cut
        && !results_cut
        && links + generated + non_files + binary + invalid_utf8 + errors + limited_files + non_utf8_paths == 0;
    render_report(
        json!({"path":original.to_string_lossy(),"query":query,"matches":matches,"coverage":{"complete":complete,
        "scope":"only inspected bytes in visited regular files; skipped or unvisited content is not searched",
        "dirs_visited":dirs,"files_visited":files,"entries_visited":entries,"bytes_scanned":scanned,"bytes_searched":searched,
        "skipped_links":links,"skipped_generated_dirs":generated,"skipped_non_files":non_files,"skipped_non_utf8_paths":non_utf8_paths,
        "skipped_binary_files":binary,"invalid_utf8_files":invalid_utf8,"io_errors":errors,"limited_files":limited_files,
        "depth_limit_hit":depth_cut,"scan_limit_hit":budget_cut || limited_files>0,"time_limit_hit":time_cut,
        "output_truncated":output_cut,"result_limit_hit":results_cut,"unvisited_entries":pending.len(),
        "limits":{"depth":limits.depth,"directories":limits.dirs,"files":limits.files,"entries":limits.entries,
        "per_file_bytes":limits.per_file,"total_bytes":limits.total,"output_bytes":OUTPUT_LIMIT,"soft_elapsed_budget_seconds":5}}}),
        "matches",
        &signal,
        start,
    )
}

#[async_trait]
impl AgentTool for LocalTool {
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
        self.run(&args, signal).await.map(AgentToolResult::text).map_err(|e| AgentError::Tool(e.to_string()))
    }
}

impl LocalTool {
    async fn run(&self, args: &Value, signal: CancellationToken) -> Result<String> {
        match self.schema.name.as_str() {
            "exec" => {
                let cwd = rooted(
                    &self.root,
                    args.get("cwd").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("."),
                    false,
                )?;
                if !cwd.is_dir() {
                    bail!("cwd is not a directory");
                }
                let seconds = args.get("timeout").and_then(Value::as_u64).unwrap_or(600);
                if !(1..=3600).contains(&seconds) {
                    bail!("timeout must be between 1 and 3600 seconds");
                }
                let (text, code) =
                    run_shell(argument(args, "cmd")?, &cwd, signal, Duration::from_secs(seconds)).await?;
                if code != 0 {
                    bail!("command exited with code {code}\n{text}");
                }
                Ok(text)
            }
            "read_file" => {
                let selection = ReadSelection::parse(args)?;
                let path = rooted(&self.root, argument(args, "path")?, false)?;
                if let Some(selection) = selection {
                    read_selection(&path, selection, signal).await
                } else {
                    read_text(&path)
                }
            }
            "search_files" => search_files(&self.root, args, signal, SearchLimits::default()).await,
            "download_file" => {
                let expected = expected_sha256(args)?;
                let url = reqwest::Url::parse(argument(args, "url")?).context("invalid download URL")?;
                if !matches!(url.scheme(), "http" | "https") {
                    bail!("downloads require HTTP or HTTPS");
                }
                let path = rooted(&self.root, argument(args, "path")?, true)?;
                download_file(url, &path, expected.as_deref(), signal).await
            }
            "write_file" => {
                let text = argument(args, "content")?;
                if text.len() > FILE_LIMIT {
                    bail!("content exceeds 1 MiB limit");
                }
                let path = rooted(&self.root, argument(args, "path")?, true)?;
                std::fs::write(&path, text)?;
                Ok(format!("Wrote {} bytes to {}", text.len(), path.display()))
            }
            "edit_file" => {
                let path = rooted(&self.root, argument(args, "path")?, false)?;
                let text = read_text(&path)?;
                let old = argument(args, "old_text")?;
                if old.is_empty() || text.matches(old).count() != 1 {
                    bail!("old_text must match exactly one non-empty occurrence");
                }
                let new = text.replacen(old, argument(args, "new_text")?, 1);
                if new.len() > FILE_LIMIT {
                    bail!("edited file exceeds 1 MiB limit");
                }
                std::fs::write(&path, new)?;
                Ok(format!("Edited {}", path.display()))
            }
            "list_dir" => {
                let path = rooted(&self.root, argument(args, "path")?, false)?;
                let mut names = Vec::new();
                for entry in std::fs::read_dir(path)? {
                    let entry = entry?;
                    names.push(format!(
                        "{}{}",
                        entry.file_name().to_string_lossy(),
                        if entry.file_type()?.is_dir() { "/" } else { "" }
                    ));
                    if names.len() >= 2048 {
                        names.push("[entry limit reached]".into());
                        break;
                    }
                }
                names.sort();
                Ok(bounded(names.join("\n"), OUTPUT_LIMIT))
            }
            _ => bail!("unknown local tool"),
        }
    }
}

fn expected_sha256(args: &Value) -> Result<Option<String>> {
    match args.get("expected_sha256") {
        None => Ok(None),
        Some(Value::String(value)) if value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()) => {
            Ok(Some(value.to_ascii_lowercase()))
        }
        Some(_) => bail!("expected_sha256 must be exactly 64 hexadecimal characters"),
    }
}

async fn download_file(
    url: reqwest::Url,
    destination: &Path,
    expected: Option<&str>,
    signal: CancellationToken,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .build()?;
    let temporary = destination.with_file_name(format!(
        ".handwave-download-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
    ));
    // Create synchronously and exclusively so cancellation cannot abandon an
    // in-flight asynchronous open whose eventual ownership is unknown.
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create temporary download")?;
    let mut file = tokio::fs::File::from_std(file);
    let result = {
        let operation = async {
            let mut response = client.get(url).send().await.map_err(reqwest::Error::without_url)?;
            if !response.status().is_success() {
                bail!("download HTTP status {}", response.status());
            }
            if response.content_length().is_some_and(|length| length > DOWNLOAD_LIMIT) {
                bail!("download exceeds 128 MiB limit");
            }
            let mut size = 0u64;
            let mut hasher = Sha256::new();
            while let Some(chunk) = response.chunk().await.map_err(reqwest::Error::without_url)? {
                size = size.checked_add(chunk.len() as u64).context("download length overflow")?;
                if size > DOWNLOAD_LIMIT {
                    bail!("download exceeds 128 MiB limit");
                }
                file.write_all(&chunk).await?;
                hasher.update(&chunk);
            }
            file.flush().await?;
            file.sync_all().await?;
            drop(file);
            let hash = format!("{:x}", hasher.finalize());
            if expected.is_some_and(|value| value != hash) {
                bail!("download SHA256 mismatch; existing destination preserved (actual {hash})");
            }
            if signal.is_cancelled() {
                bail!("download canceled before destination replacement");
            }
            // The final effect is synchronous: cancellation cannot drop a detached
            // async rename and then report a failure before that rename executes.
            std::fs::rename(&temporary, destination).context("replace download destination")?;
            Ok::<_, anyhow::Error>(format!("Downloaded {size} bytes to {}\nSHA256: {hash}", destination.display()))
        };
        tokio::select! {
            result = operation => result,
            _ = signal.cancelled() => Err(anyhow!("download canceled; existing destination preserved")),
            _ = tokio::time::sleep(Duration::from_secs(300)) => Err(anyhow!("download timed out; existing destination preserved")),
        }
    };
    // Only our exclusively created adjacent temporary file may be removed.
    // A complete rename has already moved it away; NotFound is expected then.
    match tokio::fs::remove_file(&temporary).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow!(
                "download temporary cleanup failed: {error}; result: {}",
                result.as_ref().map(|s| s.as_str()).unwrap_or("download failed")
            ))
        }
    }
    result
}

// Locate in the original UTF-8 line, including lowercase expansions such as İ.
fn match_excerpt<'a>(text: &'a str, needle: &str, sensitive: bool, limit: usize) -> Option<(&'a str, usize, bool)> {
    let hit = if sensitive {
        text.find(needle)?
    } else {
        let lowered = text.to_lowercase();
        let hit = lowered.find(needle)?;
        // String::to_lowercase has contextual mappings (Greek sigma), but their
        // encoded lengths match per-character mappings; expansions remain mapped
        // to the source character rather than used as source byte offsets.
        let mut position = 0;
        text.char_indices().find_map(|(offset, ch)| {
            let end = position + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
            let found = (hit < end).then_some(offset);
            position = end;
            found
        })?
    };
    let mut start = hit.saturating_sub(limit / 4);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + limit).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some((&text[start..end], start, start != 0 || end != text.len()))
}

pub fn bounded(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[output truncated]");
    }
    text
}

async fn drain<R: AsyncRead + Unpin>(mut reader: R, limit: usize, keep_tail: bool) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut tail = VecDeque::new();
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        let size = reader.read(&mut buf).await?;
        if size == 0 {
            break;
        }
        let head_limit = if keep_tail { limit / 2 } else { limit };
        let keep = size.min(head_limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buf[..keep]);
        if keep_tail {
            for byte in &buf[keep..size] {
                if tail.len() == limit - head_limit {
                    tail.pop_front();
                    truncated = true;
                }
                tail.push_back(*byte);
            }
        } else {
            truncated |= keep < size;
        }
    }
    if keep_tail {
        if truncated {
            bytes.extend_from_slice(b"\n[... middle output omitted ...]\n");
        }
        bytes.extend(tail);
    }
    Ok((bytes, truncated))
}

fn compact_stream(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= limit {
        return text.into_owned();
    }
    const MARKER: &str = "\n[... middle output omitted ...]\n";
    let available = limit.saturating_sub(MARKER.len());
    let mut head = available / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - (available - available / 2);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{}{}", &text[..head], MARKER, &text[tail..])
}

/// Run one command with bounded output and process-tree cleanup before settlement.
pub async fn run_shell(
    command: &str,
    cwd: &Path,
    cancel: CancellationToken,
    timeout: Duration,
) -> Result<(String, i32)> {
    if cancel.is_cancelled() {
        bail!("command canceled before start");
    }
    let cmd = if cfg!(windows) {
        let mut c = Command::new("powershell.exe");
        let script = format!("[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [Console]::OutputEncoding; {command}");
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    run_process(cmd, cwd, cancel, timeout, OUTPUT_LIMIT, false).await
}

/// Shared process-tree lifecycle for shell commands and explicitly selected scripts.
/// Compact mode reserves independent stdout/stderr budgets and retains their tails.
pub(crate) async fn run_process(
    mut cmd: Command,
    cwd: &Path,
    cancel: CancellationToken,
    timeout: Duration,
    stream_limit: usize,
    compact: bool,
) -> Result<(String, i32)> {
    if cancel.is_cancelled() {
        bail!("command canceled before start");
    }
    cmd.current_dir(cwd)
        .env_remove("HW_TOKEN")
        .env_remove("HW_NODE_STATE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x08000004); // CREATE_SUSPENDED | CREATE_NO_WINDOW
    let mut child = cmd.spawn().context("start shell")?;
    let pid = child.id().context("shell PID unavailable")?;
    let tree = match ProcessTree::new(pid) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
    };
    #[cfg(windows)]
    if let Err(error) = resume_process(pid) {
        tree.terminate();
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(error);
    }
    let mut stdout = tokio::spawn(drain(child.stdout.take().context("stdout unavailable")?, stream_limit, compact));
    let mut stderr = tokio::spawn(drain(child.stderr.take().context("stderr unavailable")?, stream_limit, compact));
    let outcome = tokio::select! {
        result = child.wait() => result.map_err(anyhow::Error::from),
        _ = cancel.cancelled() => Err(anyhow!("command canceled; partial effects may remain")),
        _ = tokio::time::sleep(timeout) => Err(anyhow!("command timed out; partial effects may remain")),
    };
    // Also terminate descendants after the root exits, preventing background jobs
    // from escaping the run or holding stdout open indefinitely.
    tree.terminate();
    if outcome.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let outputs =
        match tokio::time::timeout(Duration::from_secs(5), async { Ok::<_, anyhow::Error>((&mut stdout).await??) })
            .await
        {
            Ok(result) => result?,
            Err(_) => {
                stdout.abort();
                stderr.abort();
                bail!("shell stdout cleanup timed out");
            }
        };
    let errors = match tokio::time::timeout(Duration::from_secs(5), &mut stderr).await {
        Ok(result) => result??,
        Err(_) => {
            stderr.abort();
            bail!("shell stderr cleanup timed out");
        }
    };
    let (out, out_cut) = outputs;
    let (err, err_cut) = errors;
    let mut text = if compact {
        format!(
            "stdout{}:\n{}\nstderr{}:\n{}",
            if out_cut { " (truncated)" } else { "" },
            compact_stream(&out, stream_limit),
            if err_cut { " (truncated)" } else { "" },
            compact_stream(&err, stream_limit)
        )
    } else {
        String::from_utf8_lossy(&out).into_owned()
    };
    if !compact && !err.is_empty() {
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&err));
    }
    if !compact && (out_cut || err_cut) {
        text.push_str("\n[output truncated]");
    }
    let text = if compact { text } else { bounded(text, OUTPUT_LIMIT) };
    let status = outcome.map_err(|e| anyhow!("{e}\n{text}"))?;
    Ok((text, status.code().unwrap_or(-1)))
}

#[cfg(unix)]
struct ProcessTree(u32, std::sync::atomic::AtomicBool);
#[cfg(unix)]
impl ProcessTree {
    fn new(pid: u32) -> Result<Self> {
        Ok(Self(pid, std::sync::atomic::AtomicBool::new(false)))
    }
    fn terminate(&self) {
        if self.1.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(windows)]
struct ProcessTree(isize);
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
impl ProcessTree {
    fn new(pid: u32) -> Result<Self> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{JobObjects::*, Threading::*},
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                let e = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(e.into());
            }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                let e = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(e.into());
            }
            let assigned = AssignProcessToJobObject(job, process);
            let error = std::io::Error::last_os_error();
            CloseHandle(process);
            if assigned == 0 {
                CloseHandle(job);
                return Err(error.into());
            }
            Ok(Self(job as isize))
        }
    }
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1);
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

#[cfg(windows)]
fn resume_process(pid: u32) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut available = Thread32First(snapshot, &mut entry);
        let mut resumed = false;
        let mut failure = None;
        while available != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    failure = Some(std::io::Error::last_os_error());
                    break;
                }
                let result = ResumeThread(thread);
                let error = std::io::Error::last_os_error();
                CloseHandle(thread);
                if result == u32::MAX {
                    failure = Some(error);
                    break;
                }
                resumed = true;
            }
            available = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        if let Some(error) = failure {
            return Err(error.into());
        }
        if !resumed {
            bail!("no suspended shell thread found");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excerpts_show_hits_and_map_unicode_lowercase_to_source() {
        let text = format!("{}İ PRECISE_ERROR 中文尾部", "日志".repeat(1500));
        for (needle, sensitive) in [("PRECISE_ERROR", true), ("i\u{307} precise_error", false)] {
            let (excerpt, offset, cut) = match_excerpt(&text, needle, sensitive, 1024).unwrap();
            assert!(excerpt.contains("PRECISE_ERROR"));
            assert!(cut && offset > 0 && excerpt.len() <= 1024);
            assert_eq!(excerpt, &text[offset..offset + excerpt.len()]);
        }
        assert_eq!(match_excerpt("ΟΣ END", "ος", false, 1024).unwrap().0, "ΟΣ END");
        assert!(match_excerpt("no hit", "absent", false, 1024).is_none());
    }

    #[tokio::test]
    async fn compact_output_preserves_both_ends_and_utf8_byte_budget() {
        let original = [b"HEAD".as_slice(), &vec![b'x'; 20000], b"TAIL"].concat();
        let (bytes, cut) = drain(original.as_slice(), 4096, true).await.unwrap();
        let output = compact_stream(&bytes, 4096);
        assert!(cut && output.starts_with("HEAD") && output.ends_with("TAIL"));
        assert!(output.len() <= 4096);
        let invalid = [vec![255; 4096], b"ERROR_TAIL".to_vec()].concat();
        let output = compact_stream(&invalid, 4096);
        assert!(output.len() <= 4096 && output.ends_with("ERROR_TAIL"));
    }

    #[tokio::test]
    async fn actual_search_long_line_returns_matching_region() {
        let files = Files::new();
        std::fs::write(files.0.join("long.log"), format!("{}İ ERROR_42", "中".repeat(4000))).unwrap();
        let result = files.run("search_files", json!({"query":"i\u{307} error_42","case_sensitive":false})).await;
        let hit = &result["matches"][0];
        assert!(hit["excerpt"].as_str().unwrap().contains("ERROR_42"));
        assert!(hit["excerpt_byte_offset"].as_u64().unwrap() > 0);
        assert_eq!(hit["excerpt_truncated"], true);
    }

    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "handwave-read-tools-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn tool(&self, name: &str) -> LocalTool {
            LocalTool {
                root: self.0.clone(),
                schema: tools(self.0.clone())
                    .into_iter()
                    .find(|tool| tool.schema().name == name)
                    .unwrap()
                    .schema()
                    .clone(),
            }
        }
        async fn run(&self, name: &str, args: Value) -> Value {
            serde_json::from_str(&self.tool(name).run(&args, CancellationToken::new()).await.unwrap()).unwrap()
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[tokio::test]
    async fn download_checksum_validates_before_effects_and_preserves_destination_on_mismatch() {
        let files = Files::new();
        let tool = files.tool("download_file");
        for value in [json!(null), json!(42), json!("bad"), json!("z".repeat(64))] {
            assert!(tool
                .run(
                    &json!({"url":"http://127.0.0.1:1/", "path":"new/file", "expected_sha256":value}),
                    CancellationToken::new()
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("expected_sha256"));
            assert!(!files.0.join("new").exists());
        }
        let payload = b"verified tool payload\n";
        let hash = format!("{:x}", Sha256::digest(payload));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).await.unwrap();
                socket
                    .write_all(
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len())
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(payload).await.unwrap();
            }
        });
        let url = format!("http://{address}/tool");
        std::fs::write(files.0.join("tool"), b"original").unwrap();
        let error = tool
            .run(&json!({"url":url,"path":"tool","expected_sha256":"0".repeat(64)}), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("SHA256 mismatch"));
        assert_eq!(std::fs::read(files.0.join("tool")).unwrap(), b"original");
        let receipt = tool
            .run(
                &json!({"url":url,"path":"tool","expected_sha256":hash.to_ascii_uppercase()}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(receipt.contains(&format!("SHA256: {hash}")));
        assert_eq!(std::fs::read(files.0.join("tool")).unwrap(), payload);
        let receipt = tool.run(&json!({"url":url,"path":"other"}), CancellationToken::new()).await.unwrap();
        assert!(receipt.contains(&format!("SHA256: {hash}")));
        server.await.unwrap();
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 2, "temporary download file leaked");
    }

    #[tokio::test]
    async fn path_only_text_remains_exact_and_options_are_strict() {
        let files = Files::new();
        let original = "第一行\r\nsecond\n末行";
        std::fs::write(files.0.join("small.txt"), original).unwrap();
        let tool = files.tool("read_file");
        assert_eq!(tool.run(&json!({"path":"small.txt"}), CancellationToken::new()).await.unwrap(), original);
        std::fs::write(files.0.join("large.txt"), vec![b'x'; FILE_LIMIT + 1]).unwrap();
        assert!(tool
            .run(&json!({"path":"large.txt"}), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("1 MiB"));
        for invalid in [
            json!({"offset":null}),
            json!({"offset":0}),
            json!({"offset":-1}),
            json!({"limit":"2"}),
            json!({"limit":1.5}),
            json!({"tail":false}),
            json!({"tail":2001}),
            json!({"tail":1,"offset":1}),
            json!({"tail":1,"limit":1}),
        ] {
            assert!(ReadSelection::parse(&invalid).is_err(), "accepted {invalid}");
        }
        let selected = files.run("read_file", json!({"path":"small.txt","offset":2,"limit":2})).await;
        assert_eq!(selected["lines"][0]["line"], 2);
        assert_eq!(selected["lines"][0]["text"], "second");
        assert_eq!(selected["lines"][1]["text"], "末行");
        assert_eq!(selected["coverage"]["complete"], true);
    }

    #[tokio::test]
    async fn real_large_log_range_tail_and_giant_line_are_bounded() {
        let files = Files::new();
        let path = files.0.join("log.txt");
        let mut text = String::new();
        for i in 1..=40000 {
            text.push_str(&format!("record {i:05} 中文日志内容\r\n"));
        }
        text.push_str("final 无换行");
        assert!(text.len() > FILE_LIMIT);
        std::fs::write(&path, text).unwrap();
        let range = files.run("read_file", json!({"path":"log.txt","offset":39999,"limit":3})).await;
        assert_eq!(range["lines"][0]["line"], 39999);
        assert_eq!(range["lines"][2]["text"], "final 无换行");
        let tail = files.run("read_file", json!({"path":"log.txt","tail":2})).await;
        assert!(tail["lines"][0]["line"].is_null());
        assert_eq!(tail["lines"][0]["line_from_end"], 2);
        assert_eq!(tail["lines"][1]["text"], "final 无换行");
        let huge = files.0.join("huge.txt");
        std::fs::write(&huge, vec![b'x'; SCAN_BYTES + 4096]).unwrap();
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(&huge).unwrap().write_all(b"\npenultimate\nreal eof").unwrap();
        let tail = files.run("read_file", json!({"path":"huge.txt","tail":2})).await;
        assert_eq!(tail["lines"][0]["text"], "penultimate");
        assert_eq!(tail["lines"][1]["text"], "real eof");
        assert_eq!(tail["coverage"]["complete"], true);
        assert!(tail["coverage"]["window_start_byte"].as_u64().unwrap() > 0);
        assert!(tail["coverage"]["bytes_scanned"].as_u64().unwrap() <= SCAN_BYTES as u64);
        let range = files.run("read_file", json!({"path":"huge.txt","offset":1,"limit":1})).await;
        assert_eq!(range["lines"][0]["truncated"], true);
        assert_eq!(range["coverage"]["complete"], false);
        assert!(range["lines"][0]["text"].as_str().unwrap().len() <= LINE_PREFIX);
    }

    #[tokio::test]
    async fn search_real_files_literal_case_skips_and_budget_are_explicit() {
        let files = Files::new();
        std::fs::write(files.0.join("one.txt"), "first\r\nNeedle 中文\nneedle\n").unwrap();
        std::fs::create_dir(files.0.join("nested")).unwrap();
        std::fs::write(files.0.join("nested/two.txt"), "Needle literal.*\n").unwrap();
        for name in [".git", "node_modules", "target"] {
            std::fs::create_dir(files.0.join(name)).unwrap();
            std::fs::write(files.0.join(name).join("skip.txt"), "Needle").unwrap();
        }
        std::fs::write(files.0.join("binary.dat"), b"\0Needle").unwrap();
        std::fs::write(files.0.join("invalid.txt"), [0xff, 0xff]).unwrap();
        let report = files.run("search_files", json!({"query":"needle","case_sensitive":false})).await;
        assert_eq!(report["matches"].as_array().unwrap().len(), 3);
        assert_eq!(report["coverage"]["skipped_generated_dirs"], 3);
        assert_eq!(report["coverage"]["skipped_binary_files"], 1);
        assert_eq!(report["coverage"]["invalid_utf8_files"], 1);
        assert_eq!(report["coverage"]["complete"], false);
        let literal = files.run("search_files", json!({"query":"literal.*"})).await;
        assert_eq!(literal["matches"].as_array().unwrap().len(), 1);
        assert_eq!(literal["matches"][0]["line"], 1);
        let limits = SearchLimits { per_file: 8, total: 8, ..SearchLimits::default() };
        let cut: Value = serde_json::from_str(
            &search_files(
                &files.0,
                &json!({"query":"not-in-prefix","path":"one.txt"}),
                CancellationToken::new(),
                limits,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cut["coverage"]["complete"], false);
        assert_eq!(cut["coverage"]["limited_files"], 1);
        assert!(cut["coverage"]["bytes_searched"].as_u64().unwrap() <= 8);
        for invalid in [
            json!({"query":""}),
            json!({"query":"a\nb"}),
            json!({"query":"x","case_sensitive":null}),
            json!({"query":"x","max_results":0}),
            json!({"query":"x","path":7}),
        ] {
            assert!(files.tool("search_files").run(&invalid, CancellationToken::new()).await.is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recursive_search_skips_real_directory_and_file_symlinks() {
        let files = Files::new();
        let outside = Files::new();
        std::fs::write(outside.0.join("secret.txt"), "find-me").unwrap();
        std::os::unix::fs::symlink(&outside.0, files.0.join("dir-link")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret.txt"), files.0.join("file-link")).unwrap();
        let report = files.run("search_files", json!({"query":"find-me"})).await;
        assert_eq!(report["matches"].as_array().unwrap().len(), 0);
        assert_eq!(report["coverage"]["skipped_links"], 2);
        assert_eq!(report["coverage"]["complete"], false);
    }

    #[tokio::test]
    async fn actual_chunked_scan_cancel_settles_and_leaves_files_unchanged() {
        let files = Files::new();
        let path = files.0.join("long.txt");
        std::fs::write(&path, vec![b'x'; SCAN_BYTES + 1]).unwrap();
        let signal = CancellationToken::new();
        let scan_signal = signal.clone();
        let tool = files.tool("read_file");
        let start = Instant::now();
        let scan =
            tokio::spawn(async move { tool.run(&json!({"path":"long.txt","offset":1,"limit":1}), scan_signal).await });
        tokio::task::yield_now().await;
        signal.cancel();
        assert!(tokio::time::timeout(Duration::from_secs(2), scan)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("canceled"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(std::fs::metadata(path).unwrap().len(), (SCAN_BYTES + 1) as u64);
        let signal = CancellationToken::new();
        signal.cancel();
        assert!(search_files(&files.0, &json!({"query":"x"}), signal, SearchLimits::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("canceled"));
    }

    #[tokio::test]
    async fn text_prefix_utf8_output_and_search_limits_are_explicit() {
        let files = Files::new();
        let mut split = vec![b'x'; LINE_PREFIX - 1];
        split.extend_from_slice("汉\n".as_bytes());
        std::fs::write(files.0.join("split.txt"), split).unwrap();
        let range = files.run("read_file", json!({"path":"split.txt","limit":1})).await;
        assert_eq!(range["lines"][0]["text"].as_str().unwrap().len(), LINE_PREFIX - 1);
        assert_eq!(range["lines"][0]["truncated"], true);
        std::fs::write(files.0.join("many.txt"), format!("MARK {}\n", "x".repeat(1024)).repeat(1000)).unwrap();
        let range = files.run("read_file", json!({"path":"many.txt","limit":1000})).await;
        assert_eq!(range["coverage"]["output_truncated"], true);
        assert!(serde_json::to_vec(&range).unwrap().len() <= OUTPUT_LIMIT);
        let one = files.run("search_files", json!({"query":"MARK","path":"many.txt","max_results":1})).await;
        assert_eq!(one["matches"].as_array().unwrap().len(), 1);
        assert_eq!(one["coverage"]["result_limit_hit"], true);
        let report = files.run("search_files", json!({"query":"MARK","path":"many.txt","max_results":1000})).await;
        assert_eq!(report["coverage"]["output_truncated"], true);
        assert!(serde_json::to_vec(&report).unwrap().len() <= OUTPUT_LIMIT);
        std::fs::write(files.0.join("case.txt"), "ÄBC\n").unwrap();
        let folded = files.run("search_files", json!({"query":"äbc","path":"case.txt","case_sensitive":false})).await;
        assert_eq!(folded["matches"].as_array().unwrap().len(), 1);
        let limits = SearchLimits { files: 0, ..SearchLimits::default() };
        let bounded: Value = serde_json::from_str(
            &search_files(&files.0, &json!({"query":"x"}), CancellationToken::new(), limits).await.unwrap(),
        )
        .unwrap();
        assert_eq!(bounded["coverage"]["scan_limit_hit"], true);
        assert_eq!(bounded["coverage"]["complete"], false);
        let limits = SearchLimits { depth: 0, ..SearchLimits::default() };
        let bounded: Value = serde_json::from_str(
            &search_files(&files.0, &json!({"query":"x"}), CancellationToken::new(), limits).await.unwrap(),
        )
        .unwrap();
        assert_eq!(bounded["coverage"]["depth_limit_hit"], true);
        let missing = files.run("search_files", json!({"query":"x","path":"missing"})).await;
        assert_eq!(missing["coverage"]["io_errors"], 1);
        assert_eq!(missing["coverage"]["complete"], false);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_file_is_reported_as_partial_not_no_results() {
        use std::os::unix::fs::PermissionsExt;
        let files = Files::new();
        let path = files.0.join("denied.txt");
        std::fs::write(&path, "needle").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
        let report = files.run("search_files", json!({"query":"needle"})).await;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(report["coverage"]["io_errors"], 1);
        assert_eq!(report["coverage"]["complete"], false);
    }

    #[test]
    fn absolute_paths_and_utf8_boundaries() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(rooted(&root, root.to_str().unwrap(), false).unwrap(), root);
        assert!(rooted(&root, "./", false).is_ok());
        assert!(bounded("你好吗".into(), 4).starts_with("你\n"));
    }
    #[tokio::test]
    async fn real_shell_captures_output_and_exit() {
        let command =
            if cfg!(windows) { "Write-Output 'receipt 中文'; exit 7" } else { "printf 'receipt 中文'; exit 7" };
        let (out, code) =
            run_shell(command, &std::env::temp_dir(), CancellationToken::new(), Duration::from_secs(10)).await.unwrap();
        assert_eq!(code, 7);
        assert!(out.contains("receipt"));
        assert!(out.contains("中文"));
    }
    #[tokio::test]
    async fn real_shell_timeout_settles() {
        let command = if cfg!(windows) { "Start-Sleep -Seconds 20" } else { "sleep 20" };
        let start = std::time::Instant::now();
        let error = run_shell(command, &std::env::temp_dir(), CancellationToken::new(), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn real_cancellation_terminates_descendant_process() {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{
                OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            },
        };
        let path = std::env::temp_dir().join(format!(
            "handwave-child-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let escaped = path.display().to_string().replace('\'', "''");
        // ShellExecute can delegate launch to a broker outside the job and may
        // wait on desktop integration. Direct process creation exercises the
        // descendant inheritance this host promises for its own commands.
        let command = format!("$info = [System.Diagnostics.ProcessStartInfo]::new(); $info.FileName = 'powershell.exe'; $info.Arguments = '-NoProfile -NonInteractive -Command \"Start-Sleep -Seconds 60\"'; $info.UseShellExecute = $false; $info.CreateNoWindow = $true; $p = [System.Diagnostics.Process]::Start($info); [IO.File]::WriteAllText('{escaped}', $p.Id.ToString()); Start-Sleep -Seconds 60");
        let token = CancellationToken::new();
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            run_shell(&command, &std::env::temp_dir(), task_token, Duration::from_secs(90)).await
        });
        let start = std::time::Instant::now();
        let mut descendant_pid = None;
        while start.elapsed() < Duration::from_secs(30) {
            descendant_pid = std::fs::read_to_string(&path).ok().and_then(|text| text.parse::<u32>().ok());
            if descendant_pid.is_some() || task.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let Some(pid) = descendant_pid else {
            token.cancel();
            let diagnostic = task.await;
            panic!("child PID receipt was not produced after {:?}; shell result: {diagnostic:?}", start.elapsed());
        };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
        token.cancel();
        let result = task.await.unwrap().unwrap_err();
        assert!(result.to_string().contains("canceled"));
        if !process.is_null() {
            let status = unsafe { WaitForSingleObject(process, 5000) };
            unsafe {
                CloseHandle(process);
            }
            assert_eq!(status, WAIT_OBJECT_0, "descendant did not exit after cancellation");
        } else {
            panic!("could not open live descendant before cancellation");
        }
        std::fs::remove_file(path).unwrap();
    }
}
