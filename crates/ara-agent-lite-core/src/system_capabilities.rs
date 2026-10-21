//! Small, read-only environment discovery. This never starts a discovered program.
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde_json::{json, Value};
use std::{ffi::OsStr, io, path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const MAX_DIRECTORIES: usize = 64;
const MAX_PATH_BYTES: usize = 32 * 1024;
const MAX_PROBES: usize = 512;
const MAX_OUTPUT: usize = 4096;

pub fn tools(root: PathBuf) -> Vec<Arc<dyn AgentTool>> {
    vec![Arc::new(SystemInfo {
        root,
        schema: Tool {
            name: "system_info".into(),
            description: "Inspect OS, architecture, cwd, default shell and bounded PATH candidates for Python, cmd, PowerShell, sh, git and rg. Does not execute programs or read windows/screens. Found files are not proof a program runs or of its version; inspect coverage and errors.".into(),
            parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
            constrained_sampling: None,
        },
    })]
}

struct SystemInfo {
    root: PathBuf,
    schema: Tool,
}

#[async_trait]
impl AgentTool for SystemInfo {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "system_info"
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
        if !args.as_object().is_some_and(|object| object.is_empty()) {
            return Err(AgentError::Tool("system_info accepts an empty object".into()));
        }
        let path = std::env::var_os("PATH");
        let (directories, partial) = path_directories(path.as_deref(), &self.root);
        inspect(&self.root, directories, partial, cfg!(windows), signal, MAX_PROBES).await.map(AgentToolResult::text)
    }
}

fn path_directories(path: Option<&OsStr>, root: &std::path::Path) -> (Vec<PathBuf>, bool) {
    let Some(path) = path else {
        return (Vec::new(), true);
    };
    let mut directories = Vec::new();
    let mut bytes = 0usize;
    for entry in std::env::split_paths(path) {
        bytes = bytes.saturating_add(entry.as_os_str().as_encoded_bytes().len());
        if directories.len() == MAX_DIRECTORIES || bytes > MAX_PATH_BYTES {
            return (directories, true);
        }
        directories.push(if entry.is_absolute() { entry } else { root.join(entry) });
    }
    (directories, false)
}

fn names(windows: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    if windows {
        vec![
            ("python", vec!["python.exe", "python.com"]),
            ("cmd", vec!["cmd.exe"]),
            ("powershell", vec!["powershell.exe", "pwsh.exe"]),
            ("sh", vec!["sh.exe"]),
            ("git", vec!["git.exe", "git.com"]),
            ("rg", vec!["rg.exe", "rg.com"]),
        ]
    } else {
        vec![
            ("python", vec!["python3", "python"]),
            ("cmd", vec!["cmd"]),
            ("powershell", vec!["pwsh"]),
            ("sh", vec!["sh"]),
            ("git", vec!["git"]),
            ("rg", vec!["rg"]),
        ]
    }
}

#[derive(Default)]
struct Observation {
    found: Option<PathBuf>,
    denied: usize,
    errors: usize,
    not_executable: usize,
}

fn record_error(observation: &mut Observation, kind: io::ErrorKind) {
    match kind {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {}
        io::ErrorKind::PermissionDenied => observation.denied += 1,
        _ => observation.errors += 1,
    }
}

fn executable(metadata: &std::fs::Metadata, windows: bool) -> bool {
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    if !windows {
        use std::os::unix::fs::PermissionsExt;
        return metadata.permissions().mode() & 0o111 != 0;
    }
    let _ = windows;
    true
}

fn shortened(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

async fn inspect(
    root: &std::path::Path,
    directories: Vec<PathBuf>,
    mut partial: bool,
    windows: bool,
    signal: CancellationToken,
    probe_budget: usize,
) -> Result<String, AgentError> {
    let programs = names(windows);
    let mut observations: Vec<_> = programs.iter().map(|_| Observation::default()).collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut probes = 0;
    'directories: for directory in &directories {
        for ((_, candidates), observation) in programs.iter().zip(observations.iter_mut()) {
            if observation.found.is_some() {
                continue;
            }
            for candidate in candidates {
                if signal.is_cancelled() {
                    return Err(AgentError::Abort);
                }
                if probes >= probe_budget {
                    partial = true;
                    break 'directories;
                }
                probes += 1;
                let path = directory.join(candidate);
                let result = tokio::select! {
                    biased;
                    _ = signal.cancelled() => return Err(AgentError::Abort),
                    _ = tokio::time::sleep_until(deadline) => {
                        partial = true;
                        break 'directories;
                    },
                    result = tokio::fs::metadata(&path) => result,
                };
                match result {
                    Ok(metadata) if executable(&metadata, windows) => {
                        observation.found = Some(path);
                        break;
                    }
                    Ok(_) => observation.not_executable += 1,
                    Err(error) => record_error(observation, error.kind()),
                }
            }
        }
    }
    if signal.is_cancelled() {
        return Err(AgentError::Abort);
    }
    let mut rows = serde_json::Map::new();
    for ((name, _), observation) in programs.iter().zip(observations) {
        let status = if observation.found.is_some() {
            "found_candidate"
        } else if observation.denied > 0 {
            "permission_denied"
        } else if observation.errors > 0 {
            "io_error"
        } else if partial {
            "not_found_in_partial_scan"
        } else {
            "not_found"
        };
        let path = observation.found.map(|path| path.to_string_lossy().into_owned());
        rows.insert(
            (*name).into(),
            json!({
                "status": status,
                "path": path.as_deref().map(|path| shortened(path, 240)),
                "path_truncated": path.as_ref().is_some_and(|path| path.len() > 240),
                "permission_denied": observation.denied,
                "io_errors": observation.errors,
                "non_executable": observation.not_executable,
            }),
        );
    }
    let cwd = root.to_string_lossy();
    let mut value = json!({
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "cwd": shortened(&cwd, 320), "cwd_truncated": cwd.len() > 320,
        "default_shell": if windows { "powershell.exe" } else { "sh" },
        "programs": rows,
        "coverage": {"partial": partial, "path_directories_considered": directories.len(), "file_probes": probes},
        "computer_ui": "unsupported", "vision_input": "unsupported",
        "note": "PATH file candidates only; no program executed, version checked or runtime success established. Windows probes fixed .exe/.com names only, not PATHEXT scripts. Truncated paths are display-only; do not execute them."
    });
    let mut output = value.to_string();
    if output.len() > MAX_OUTPUT {
        // JSON escaping can amplify path size. Preserve statuses, never cut JSON bytes.
        value["cwd"] = Value::Null;
        value["cwd_truncated"] = json!(true);
        for row in value["programs"].as_object_mut().unwrap().values_mut() {
            if !row["path"].is_null() {
                row["path"] = Value::Null;
                row["path_truncated"] = json!(true);
            }
        }
        value["output_paths_omitted"] = json!(true);
        output = value.to_string();
    }
    if output.len() > MAX_OUTPUT {
        return Err(AgentError::Tool("system_info output budget exceeded".into()));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("hw-capabilities-{}", crate::fresh_id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) {
            let path = self.0.join(name);
            std::fs::write(&path, b"fixture; never execute").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn discovers_only_fixed_platform_candidates_without_execution() {
        let fixture = Fixture::new();
        fixture.file("python.exe");
        fixture.file("python3");
        fixture.file("git.cmd");
        for windows in [true, false] {
            let value: Value = serde_json::from_str(
                &inspect(&fixture.0, vec![fixture.0.clone()], false, windows, CancellationToken::new(), MAX_PROBES)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(value["programs"]["python"]["status"], "found_candidate");
            assert!(value["programs"]["python"]["path"].as_str().unwrap().ends_with(if windows {
                "python.exe"
            } else {
                "python3"
            }));
            assert_eq!(value["programs"]["git"]["status"], "not_found");
            assert_eq!(value["computer_ui"], "unsupported");
        }
    }

    #[tokio::test]
    async fn budgets_and_cancellation_are_explicit() {
        let fixture = Fixture::new();
        let value: Value = serde_json::from_str(
            &inspect(&fixture.0, vec![fixture.0.clone()], false, true, CancellationToken::new(), 1).await.unwrap(),
        )
        .unwrap();
        assert_eq!(value["coverage"]["partial"], true);
        assert_eq!(value["coverage"]["file_probes"], 1);
        assert_eq!(value["programs"]["rg"]["status"], "not_found_in_partial_scan");
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(inspect(&fixture.0, vec![], false, true, token, MAX_PROBES).await, Err(AgentError::Abort)));
        let noisy_root = PathBuf::from("\u{1}".repeat(1000));
        let output = inspect(&noisy_root, vec![], true, true, CancellationToken::new(), MAX_PROBES).await.unwrap();
        assert!(output.len() <= MAX_OUTPUT);
        assert!(serde_json::from_str::<Value>(&output).is_ok());
    }

    #[test]
    fn path_budget_and_error_classes_do_not_misreport_absence() {
        let fixture = Fixture::new();
        let path = std::env::join_paths(std::iter::repeat_n(&fixture.0, MAX_DIRECTORIES + 1)).unwrap();
        let (directories, partial) = path_directories(Some(&path), &fixture.0);
        assert!(partial);
        assert_eq!(directories.len(), MAX_DIRECTORIES);
        assert!(path_directories(None, &fixture.0).1);
        let mut observation = Observation::default();
        record_error(&mut observation, io::ErrorKind::NotFound);
        assert_eq!(observation.errors, 0);
        record_error(&mut observation, io::ErrorKind::PermissionDenied);
        assert_eq!(observation.denied, 1);
        record_error(&mut observation, io::ErrorKind::Other);
        assert_eq!(observation.errors, 1);
    }
}
