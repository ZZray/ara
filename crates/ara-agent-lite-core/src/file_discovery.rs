//! Bounded, metadata-only file discovery. Host OS permissions still apply.
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolExecutionMode, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 16 * 1024;
const ROW_BUDGET: usize = OUTPUT_LIMIT - 4096;

pub fn tools(root: PathBuf) -> Vec<Arc<dyn AgentTool>> {
    vec![Arc::new(FindFiles {
        root,
        schema: Tool {
            name: "find_files".into(),
            description: "Find regular files by a non-empty literal substring of their relative path (including filename), without reading file contents. path defaults to cwd and must be a directory; absolute paths are allowed. Use / between path components. Unicode lowercase matching is the default; set case_sensitive for exact case. Skips links and generated directories. Bounded to 5 seconds, depth 16, 2048 directories, 16384 entries, and 16 KiB output. max_results defaults to 50, maximum 200. Inspect coverage: zero results never proves a file does not exist outside the inspected scope. Use search_files for file contents.".into(),
            parameters: Schema(json!({"type":"object","properties":{
                "query":{"type":"string","minLength":1,"maxLength":1024},
                "path":{"type":"string"},
                "case_sensitive":{"type":"boolean"},
                "max_results":{"type":"integer","minimum":1,"maximum":200}
            },"required":["query"],"additionalProperties":false})),
            constrained_sampling: None,
        },
    })]
}

struct FindFiles {
    root: PathBuf,
    schema: Tool,
}

#[async_trait]
impl AgentTool for FindFiles {
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
        let result = find_files(&self.root, &args, signal.clone(), Limits::default()).await;
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        result.map(AgentToolResult::text).map_err(|e| AgentError::Tool(e.to_string()))
    }
}

#[derive(Clone, Copy)]
struct Limits {
    depth: usize,
    directories: usize,
    entries: usize,
    time: Duration,
    rows: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { depth: 16, directories: 2048, entries: 16384, time: Duration::from_secs(5), rows: ROW_BUDGET }
    }
}

#[derive(Default)]
struct Scan {
    matches: Vec<Value>,
    output_bytes: usize,
    directories: usize,
    entries: usize,
    files: usize,
    links: usize,
    generated: usize,
    non_files: usize,
    non_utf8: usize,
    errors: usize,
    depth_limit: bool,
    directory_limit: bool,
    entry_limit: bool,
    result_limit: bool,
    output_limit: bool,
    time_limit: bool,
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

fn generated(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        matches!(
            name.to_string_lossy().to_ascii_lowercase().as_str(),
            ".git" | "node_modules" | "target" | "dist" | "build" | "out" | ".venv" | "venv" | "__pycache__" | ".cache"
        )
    })
}

impl Scan {
    async fn walk(
        &mut self,
        root: &Path,
        needle: &str,
        sensitive: bool,
        max_results: usize,
        limits: Limits,
    ) -> Result<()> {
        // Do not canonicalize: that would conceal a symlink supplied as the root.
        // Inspect ancestors too: `link/child` and `link/.` must not bypass skipping.
        let mut ancestor = PathBuf::new();
        for component in root.components() {
            ancestor.push(component.as_os_str());
            if matches!(component, std::path::Component::Normal(_)) {
                let metadata = tokio::fs::symlink_metadata(&ancestor).await.context("inspect search path ancestor")?;
                if link_like(&metadata) {
                    self.links += 1;
                    return Ok(());
                }
            }
        }
        let metadata = tokio::fs::symlink_metadata(root).await.context("inspect search directory")?;
        if link_like(&metadata) {
            self.links += 1;
            return Ok(());
        }
        if !metadata.is_dir() {
            bail!("path must be a directory");
        }
        if generated(root) {
            self.generated += 1;
            return Ok(());
        }
        let mut pending = VecDeque::from([(root.to_owned(), 0)]);
        while let Some((directory, depth)) = pending.pop_front() {
            if self.directories >= limits.directories {
                self.directory_limit = true;
                break;
            }
            // Recheck queued directories so a replacement by a link is not followed.
            match tokio::fs::symlink_metadata(&directory).await {
                Ok(metadata) if link_like(&metadata) => {
                    self.links += 1;
                    continue;
                }
                Ok(metadata) if metadata.is_dir() => (),
                _ => {
                    self.errors += 1;
                    continue;
                }
            }
            self.directories += 1;
            let mut reader = match tokio::fs::read_dir(&directory).await {
                Ok(reader) => reader,
                Err(_) => {
                    self.errors += 1;
                    continue;
                }
            };
            loop {
                if self.entries >= limits.entries {
                    self.entry_limit = true;
                    return Ok(());
                }
                let entry = match reader.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(_) => {
                        self.errors += 1;
                        break;
                    }
                };
                self.entries += 1;
                let path = entry.path();
                let Some(relative) = path.strip_prefix(root).ok().and_then(Path::to_str) else {
                    self.non_utf8 += 1;
                    continue;
                };
                let metadata = match tokio::fs::symlink_metadata(&path).await {
                    Ok(metadata) => metadata,
                    Err(_) => {
                        self.errors += 1;
                        continue;
                    }
                };
                if link_like(&metadata) {
                    self.links += 1;
                    continue;
                }
                if metadata.is_dir() {
                    if generated(&path) {
                        self.generated += 1;
                        continue;
                    }
                    if depth >= limits.depth {
                        self.depth_limit = true;
                        continue;
                    }
                    // Bound the queued directories as well as the directories visited.
                    if self.directories + pending.len() >= limits.directories {
                        self.directory_limit = true;
                        continue;
                    }
                    pending.push_back((path, depth + 1));
                    continue;
                }
                if !metadata.is_file() {
                    self.non_files += 1;
                    continue;
                }
                self.files += 1;
                #[cfg(windows)]
                let relative = relative.replace('\\', "/");
                #[cfg(not(windows))]
                let relative = relative.to_owned();
                let haystack = if sensitive { relative.clone() } else { relative.to_lowercase() };
                if !haystack.contains(needle) {
                    continue;
                }
                let row = json!({"relative_path":relative,"size_bytes":metadata.len()});
                let bytes = serde_json::to_vec(&row)?.len() + 1;
                if self.output_bytes + bytes > limits.rows {
                    self.output_limit = true;
                    return Ok(());
                }
                self.output_bytes += bytes;
                self.matches.push(row);
                if self.matches.len() >= max_results {
                    self.result_limit = true;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn render(self) -> Result<String> {
        let complete = self.links == 0
            && self.generated == 0
            && self.non_files == 0
            && self.non_utf8 == 0
            && self.errors == 0
            && !self.depth_limit
            && !self.directory_limit
            && !self.entry_limit
            && !self.result_limit
            && !self.output_limit
            && !self.time_limit;
        let text = serde_json::to_string(&json!({"matches":self.matches,"coverage":{
            "complete":complete,
            "scope":"Only metadata of visited regular files was inspected. Contents were not read. Zero matches does not establish absence in skipped or unvisited paths. Filesystem changes during traversal are not a snapshot.",
            "directories_visited":self.directories,"entries_inspected":self.entries,"files_inspected":self.files,
            "skipped_links":self.links,"skipped_generated_dirs":self.generated,"skipped_non_files":self.non_files,
            "skipped_non_utf8_paths":self.non_utf8,"io_errors":self.errors,
            "depth_limit_hit":self.depth_limit,"directory_limit_hit":self.directory_limit,"entry_limit_hit":self.entry_limit,
            "result_limit_hit":self.result_limit,"output_limit_hit":self.output_limit,"time_limit_hit":self.time_limit
        }}))?;
        if text.len() > OUTPUT_LIMIT {
            bail!("file discovery report exceeded output limit");
        }
        Ok(text)
    }
}

async fn find_files(root: &Path, args: &Value, signal: CancellationToken, limits: Limits) -> Result<String> {
    let query = args.get("query").and_then(Value::as_str).context("query must be a string")?;
    if query.trim().is_empty() || query.len() > 1024 || query.contains(['\r', '\n', '\0']) {
        bail!("query must be a non-empty single-line literal of at most 1024 bytes");
    }
    let sensitive = match args.get("case_sensitive") {
        None => false,
        Some(value) => value.as_bool().context("case_sensitive must be a boolean")?,
    };
    let max_results = match args.get("max_results") {
        None => 50,
        Some(value) => {
            value.as_u64().filter(|n| (1..=200).contains(n)).context("max_results must be an integer from 1 to 200")?
                as usize
        }
    };
    let path = match args.get("path") {
        None => ".",
        Some(value) => value
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0'))
            .context("path must be a non-empty string of at most 4096 bytes")?,
    };
    let directory = if Path::new(path).is_absolute() { PathBuf::from(path) } else { root.join(path) };
    let needle = if sensitive { query.to_owned() } else { query.to_lowercase() };
    let mut scan = Scan::default();
    tokio::select! {
        biased;
        _ = signal.cancelled() => bail!("file discovery canceled"),
        _ = tokio::time::sleep(limits.time) => scan.time_limit = true,
        result = scan.walk(&directory, &needle, sensitive, max_results, limits) => result?,
    }
    if signal.is_cancelled() {
        bail!("file discovery canceled");
    }
    scan.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "handwave-find-files-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, path: &str, content: &[u8]) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        async fn run(&self, args: Value, limits: Limits) -> Value {
            let text = find_files(&self.0, &args, CancellationToken::new(), limits).await.unwrap();
            assert!(text.len() <= OUTPUT_LIMIT);
            serde_json::from_str(&text).unwrap()
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[tokio::test]
    async fn names_paths_unicode_and_binary_metadata() {
        let files = Files::new();
        files.write("资料/Ä日志.PY", &[0xff, 0, 0xfe]);
        files.write("other.txt", b"this content says missing-file.py");
        let report = files.run(json!({"query":"资料/ä日志"}), Limits::default()).await;
        assert_eq!(report["matches"][0], json!({"relative_path":"资料/Ä日志.PY","size_bytes":3}));
        assert_eq!(report["coverage"]["complete"], true);
        let report = files.run(json!({"query":"ä日志","case_sensitive":true}), Limits::default()).await;
        assert!(report["matches"].as_array().unwrap().is_empty());
        let report = files.run(json!({"query":"missing-file.py"}), Limits::default()).await;
        assert!(report["matches"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn generated_directories_and_depth_are_explicitly_partial() {
        let files = Files::new();
        for name in [".git", "node_modules", "target", ".venv"] {
            files.write(&format!("{name}/needle.txt"), b"hidden");
        }
        files.write("nested/deep/needle.txt", b"deep");
        let report = files.run(json!({"query":"needle"}), Limits { depth: 0, ..Limits::default() }).await;
        assert_eq!(report["coverage"]["skipped_generated_dirs"], 4);
        assert_eq!(report["coverage"]["depth_limit_hit"], true);
        assert_eq!(report["coverage"]["complete"], false);
        assert!(report["matches"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bounded_results_entries_directories_output_and_time() {
        let files = Files::new();
        for i in 0..55 {
            files.write(&format!("file-{i}.txt"), b"a");
        }
        files.write("nested/file.txt", b"b");
        let report = files.run(json!({"query":"file"}), Limits::default()).await;
        assert_eq!(report["matches"].as_array().unwrap().len(), 50);
        assert_eq!(report["coverage"]["result_limit_hit"], true);
        for (limits, field) in [
            (Limits { entries: 1, ..Limits::default() }, "entry_limit_hit"),
            (Limits { directories: 1, ..Limits::default() }, "directory_limit_hit"),
            (Limits { rows: 1, ..Limits::default() }, "output_limit_hit"),
            (Limits { time: Duration::ZERO, ..Limits::default() }, "time_limit_hit"),
        ] {
            let report = files.run(json!({"query":"file","max_results":200}), limits).await;
            assert_eq!(report["coverage"][field], true, "{field}: {report}");
            assert_eq!(report["coverage"]["complete"], false);
        }
    }

    #[tokio::test]
    async fn output_is_bounded_for_long_paths() {
        let files = Files::new();
        for i in 0..100 {
            files.write(&format!("{}-{i}.txt", "x".repeat(180)), b"");
        }
        let report = files.run(json!({"query":".txt","max_results":200}), Limits::default()).await;
        assert_eq!(report["coverage"]["output_limit_hit"], true);
        assert!(!report["matches"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_arguments_and_cancellation_are_errors() {
        let files = Files::new();
        for args in [
            json!({"query":""}),
            json!({"query":"a\nb"}),
            json!({"query":"x","max_results":201}),
            json!({"query":"x","max_results":1.5}),
            json!({"query":"x","case_sensitive":"false"}),
            json!({"query":"x","path":false}),
        ] {
            assert!(find_files(&files.0, &args, CancellationToken::new(), Limits::default()).await.is_err());
        }
        let signal = CancellationToken::new();
        signal.cancel();
        let tool = tools(files.0.clone()).remove(0);
        assert!(matches!(
            tool.execute("cancel", json!({"query":"x"}), signal, Arc::new(|_| {})).await,
            Err(AgentError::Abort)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn skips_actual_file_directory_and_root_symlinks() {
        use std::os::unix::fs::symlink;
        let files = Files::new();
        let outside = Files::new();
        outside.write("needle.txt", b"external");
        symlink(outside.0.join("needle.txt"), files.0.join("needle-link")).unwrap();
        symlink(&outside.0, files.0.join("directory-link")).unwrap();
        let report = files.run(json!({"query":"needle"}), Limits::default()).await;
        assert_eq!(report["coverage"]["skipped_links"], 2);
        assert!(report["matches"].as_array().unwrap().is_empty());
        let report = files.run(json!({"query":"needle","path":"directory-link"}), Limits::default()).await;
        assert_eq!(report["coverage"]["skipped_links"], 1);
        let report = files.run(json!({"query":"needle","path":"directory-link/."}), Limits::default()).await;
        assert_eq!(report["coverage"]["skipped_links"], 1);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn skips_actual_windows_directory_junction() {
        let files = Files::new();
        let outside = Files::new();
        outside.write("needle.txt", b"external");
        let link = files.0.join("directory-link");
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside.0)
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        let report = files.run(json!({"query":"needle"}), Limits::default()).await;
        assert_eq!(report["coverage"]["skipped_links"], 1);
        assert!(report["matches"].as_array().unwrap().is_empty());
        let report = files.run(json!({"query":"needle","path":"directory-link/."}), Limits::default()).await;
        assert_eq!(report["coverage"]["skipped_links"], 1);
        // Unlink the junction itself; never recursively remove its target.
        std::fs::remove_dir(&link).unwrap();
    }
}
