//! Explicit interpreters and temporary script files avoid nested shell quoting.
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolExecutionMode, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const SCRIPT_LIMIT: usize = 64 * 1024;
// Keep source files intact so top-level param()/using statements remain legal.
const POWERSHELL_BOOTSTRAP: &str = "param([Parameter(Mandatory=$true)][string]$SourcePath)\n$ErrorActionPreference = 'Stop'\n[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n$OutputEncoding = [Console]::OutputEncoding\n$global:LASTEXITCODE = 0\n& $SourcePath\nif ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }\n";

pub fn tools(root: PathBuf) -> Vec<Arc<dyn AgentTool>> {
    vec![Arc::new(ScriptTool { root, schema: Tool {
        name: "run_script".into(),
        description: "Run a UTF-8 script with an explicitly selected python, powershell, cmd, or sh interpreter. Pass script source directly; do not wrap it in shell quoting. Uses temporary files removed after execution and the same bounded process-tree lifecycle as exec. No interpreter fallback: Python is python on Windows, python3 elsewhere; PowerShell is powershell.exe on Windows, pwsh elsewhere. Windows PowerShell uses process-local ExecutionPolicy Bypass; it does not change machine policy or override Group Policy. cwd defaults to the working directory. Script maximum 64 KiB, timeout defaults to 60 seconds (maximum 3600), output_limit defaults to 8192 bytes (maximum 131072, split equally between stdout/stderr, retaining head and tail; labels and truncation notices add a small overhead). Optional capture_dir creates a NEW directory beneath an existing parent, relative to cwd or absolute: raw stdout.log/stderr.log, maximum 8 MiB EACH, plus output capture receipts. Re-read long UTF-8 logs with read_file offset/limit/tail; decode other encodings explicitly. Logs remain for caller-managed retention. Nonzero exit, timeout and cancellation are errors; partial filesystem effects may remain. This is not an OS sandbox.".into(),
        parameters: Schema(json!({"type":"object","properties":{
            "language":{"type":"string","enum":["python","powershell","cmd","sh"]},
            "script":{"type":"string"},"cwd":{"type":"string"},
            "timeout":{"type":"integer","minimum":1,"maximum":3600},
            "output_limit":{"type":"integer","minimum":256,"maximum":131072},
            "capture_dir":{"type":"string","minLength":1}
        },"required":["language","script"],"additionalProperties":false})), constrained_sampling: None,
    } })]
}

struct ScriptTool {
    root: PathBuf,
    schema: Tool,
}

#[async_trait]
impl AgentTool for ScriptTool {
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
        let result = async {
            let request = Request::parse(&self.root, &args)?;
            run_request(request, signal.clone(), None).await
        }
        .await;
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        result.map(AgentToolResult::text).map_err(|error| AgentError::Tool(format!("{error:#}")))
    }
}

#[derive(Clone, Copy)]
enum Language {
    Python,
    PowerShell,
    Cmd,
    Sh,
}
impl Language {
    fn executable(self) -> &'static str {
        match self {
            Self::Python => {
                if cfg!(windows) {
                    "python"
                } else {
                    "python3"
                }
            }
            Self::PowerShell => {
                if cfg!(windows) {
                    "powershell.exe"
                } else {
                    "pwsh"
                }
            }
            Self::Cmd => "cmd.exe",
            Self::Sh => "sh",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Python => "py",
            Self::PowerShell => "ps1",
            Self::Cmd => "cmd",
            Self::Sh => "sh",
        }
    }
    fn bytes(self, source: &str) -> Vec<u8> {
        match self {
            Self::PowerShell => format!("\u{feff}{}", source.trim_start_matches('\u{feff}')).into_bytes(),
            Self::Cmd => {
                format!("@echo off\r\nchcp 65001 >nul\r\n{}\r\n", source.replace("\r\n", "\n").replace('\n', "\r\n"))
                    .into_bytes()
            }
            _ => source.as_bytes().to_vec(),
        }
    }
}

struct Request {
    language: Language,
    source: String,
    cwd: PathBuf,
    timeout: Duration,
    output_limit: usize,
    capture_dir: Option<String>,
}
impl Request {
    fn parse(root: &Path, args: &Value) -> Result<Self> {
        let language = match args.get("language").and_then(Value::as_str) {
            Some("python") => Language::Python,
            Some("powershell") => Language::PowerShell,
            Some("cmd") => Language::Cmd,
            Some("sh") => Language::Sh,
            _ => bail!("language must be python, powershell, cmd, or sh"),
        };
        let source = args.get("script").and_then(Value::as_str).context("script must be a string")?;
        if source.trim().is_empty() || source.len() > SCRIPT_LIMIT || source.contains('\0') {
            bail!("script must be non-empty UTF-8 text of at most 64 KiB without NUL");
        }
        let cwd = match args.get("cwd") {
            None => root.to_owned(),
            Some(value) => {
                let value = value.as_str().filter(|s| !s.is_empty()).context("cwd must be a non-empty string")?;
                let path = Path::new(value);
                if path.is_absolute() {
                    path.to_owned()
                } else {
                    root.join(path)
                }
            }
        }
        .canonicalize()
        .context("resolve script cwd")?;
        if !cwd.is_dir() {
            bail!("cwd must be a directory");
        }
        let timeout = number(args, "timeout", 60, 1, 3600)?;
        let output_limit = number(args, "output_limit", 8192, 256, 131072)? as usize;
        let capture_dir = crate::local_tools::capture_argument(args)?.map(str::to_owned);
        Ok(Self {
            language,
            source: source.into(),
            cwd,
            timeout: Duration::from_secs(timeout),
            output_limit,
            capture_dir,
        })
    }
}
fn number(args: &Value, key: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|value| (min..=max).contains(value))
            .with_context(|| format!("{key} must be an integer from {min} to {max}")),
    }
}

struct ScriptFile {
    path: PathBuf,
    removed: bool,
}
impl ScriptFile {
    fn create(directory: &Path, language: Language, source: &str) -> Result<Self> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|error| anyhow!("script random filename: {error}"))?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = directory.join(format!("handwave-script-{name}.{}", language.extension()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).context("create exclusive temporary script")?;
        let script = Self { path, removed: false };
        let written = file.write_all(&language.bytes(source)).context("write temporary script");
        drop(file);
        written?;
        Ok(script)
    }
    fn remove(&mut self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {
                self.removed = true;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.removed = true;
                Ok(())
            }
            Err(error) => Err(error).context("remove temporary script"),
        }
    }
}
impl Drop for ScriptFile {
    fn drop(&mut self) {
        if !self.removed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn command(language: Language, path: &Path, source: &Path, executable: &OsStr) -> Result<Command> {
    let mut command = Command::new(executable);
    match language {
        Language::Python => {
            command.args(["-X", "utf8", "-u"]).arg(path).env("PYTHONIOENCODING", "utf-8");
        }
        Language::PowerShell => {
            command.args(["-NoLogo", "-NoProfile", "-NonInteractive"]);
            // Process-local policy only; Group Policy remains authoritative.
            #[cfg(windows)]
            command.args(["-ExecutionPolicy", "Bypass"]);
            command.arg("-File").arg(path).arg("-SourcePath").arg(source);
        }
        Language::Sh => {
            command.arg(path);
        }
        Language::Cmd => {
            #[cfg(not(windows))]
            bail!("cmd scripts require Windows; no interpreter fallback is used");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let path = path.to_str().context("cmd temporary path must be Unicode")?;
                if path.contains(['%', '"', '\r', '\n']) {
                    bail!("cmd temporary path contains unsupported expansion characters");
                }
                command.args(["/d", "/v:off", "/s", "/c"]);
                command.as_std_mut().raw_arg(format!("\"\"{path}\"\""));
            }
        }
    }
    Ok(command)
}

async fn run_request(request: Request, signal: CancellationToken, executable: Option<&OsStr>) -> Result<String> {
    run_in_temp(request, signal, executable, &std::env::temp_dir()).await
}

async fn run_in_temp(
    request: Request,
    signal: CancellationToken,
    executable: Option<&OsStr>,
    temp: &Path,
) -> Result<String> {
    if signal.is_cancelled() {
        bail!("script canceled before start");
    }
    let mut file = ScriptFile::create(temp, request.language, &request.source)?;
    let mut bootstrap = if matches!(request.language, Language::PowerShell) {
        Some(ScriptFile::create(temp, Language::PowerShell, POWERSHELL_BOOTSTRAP)?)
    } else {
        None
    };
    let result = async {
        let interpreter = executable.unwrap_or_else(|| OsStr::new(request.language.executable()));
        let entry = bootstrap.as_ref().map(|script| script.path.as_path()).unwrap_or(&file.path);
        let cmd = command(request.language, entry, &file.path, interpreter)?;
        let capture = request
            .capture_dir
            .as_deref()
            .map(|path| crate::process_output::CapturePair::create(&request.cwd, path))
            .transpose()
            .context("capture preparation failed; process not_started")?;
        let (text, code) = crate::local_tools::run_process(
            cmd,
            &request.cwd,
            signal,
            request.timeout,
            request.output_limit / 2,
            true,
            capture,
        )
        .await
        .with_context(|| format!("run {} script (no interpreter fallback)", request.language.executable()))?;
        if code != 0 {
            bail!("script exited with code {code}\n{text}");
        }
        Ok(text)
    }
    .await;
    let cleanup = file.remove();
    let bootstrap_cleanup = bootstrap.as_mut().map(ScriptFile::remove).transpose();
    if cleanup.is_err() || bootstrap_cleanup.is_err() {
        let outcome = match &result {
            Ok(text) => text.clone(),
            Err(error) => format!("{error:#}"),
        };
        bail!("temporary script cleanup failed: {cleanup:?}; bootstrap: {bootstrap_cleanup:?}; execution: {outcome}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "handwave script test {} {}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn request(&self, language: &str, script: &str) -> Request {
            Request::parse(&self.0, &json!({"language":language,"script":script})).unwrap()
        }
        fn empty(&self) {
            assert_eq!(std::fs::read_dir(&self.0).unwrap().count(), 0);
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn validates_limits_and_preserves_powershell_bom() {
        let files = Files::new();
        for args in [
            json!({"language":"unknown","script":"x"}),
            json!({"language":"python","script":""}),
            json!({"language":"python","script":"x".repeat(SCRIPT_LIMIT+1)}),
            json!({"language":"python","script":"x","output_limit":1}),
            json!({"language":"python","script":"x","timeout":3601}),
        ] {
            assert!(Request::parse(&files.0, &args).is_err());
        }
        let script = ScriptFile::create(&files.0, Language::PowerShell, "Write-Output '中文'").unwrap();
        let bytes = std::fs::read(&script.path).unwrap();
        assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        drop(script);
        files.empty();
    }

    #[tokio::test]
    async fn missing_interpreter_is_error_and_cleans_script() {
        let files = Files::new();
        let missing = files.0.join("missing-interpreter-do-not-fallback");
        assert!(run_in_temp(
            files.request("python", "print('must not run')"),
            CancellationToken::new(),
            Some(missing.as_os_str()),
            &files.0
        )
        .await
        .is_err());
        files.empty();
    }

    async fn exercise(language: &str, source: &str, exit: &str, sleep: &str) {
        let files = Files::new();
        let result =
            run_in_temp(files.request(language, source), CancellationToken::new(), None, &files.0).await.unwrap();
        assert!(result.contains("中文 'quote' $ & ;"), "{result}");
        files.empty();
        let error =
            run_in_temp(files.request(language, exit), CancellationToken::new(), None, &files.0).await.unwrap_err();
        assert!(error.to_string().contains("code 7"), "{error:#}");
        files.empty();
        let mut request = files.request(language, sleep);
        request.timeout = Duration::from_millis(200);
        let error = run_in_temp(request, CancellationToken::new(), None, &files.0).await.unwrap_err();
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        files.empty();
        let signal = CancellationToken::new();
        let cancel = signal.clone();
        let task = run_in_temp(files.request(language, sleep), signal, None, &files.0);
        let (result, ()) = tokio::join!(task, async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });
        assert!(format!("{:#}", result.unwrap_err()).contains("canceled"));
        files.empty();
    }

    #[tokio::test]
    async fn real_python_utf8_quoting_exit_cancel_timeout_cleanup() {
        exercise("python", "print(\"中文 'quote' $ & ;\")", "raise SystemExit(7)", "import time; time.sleep(30)").await;
    }

    #[tokio::test]
    async fn real_python_compact_output_retains_both_stream_tails() {
        let files = Files::new();
        let mut request = files.request("python", "import sys\nprint('HEAD-OUT' + 'x' * 20000 + 'TAIL-OUT')\nprint('HEAD-ERR' + 'y' * 20000 + 'TAIL-ERR', file=sys.stderr)");
        request.output_limit = 1024;
        let result = run_in_temp(request, CancellationToken::new(), None, &files.0).await.unwrap();
        for marker in ["HEAD-OUT", "TAIL-OUT", "HEAD-ERR", "TAIL-ERR", "truncated"] {
            assert!(result.contains(marker), "{marker}: {result}");
        }
        assert!(result.len() < 1500, "compact output includes bounded labels/notices");
        files.empty();
    }

    #[tokio::test]
    async fn python_descendant_cancellation_prevents_delayed_file_effect() {
        let files = Files::new();
        // Observe readiness before cancellation so this tests a live descendant.
        let source = "import pathlib, subprocess, sys, time\nchild = \"import pathlib, time; pathlib.Path('child-ready').write_text('ready'); time.sleep(2); pathlib.Path('escaped-child').write_text('bad')\"\nsubprocess.Popen([sys.executable, '-c', child])\ntime.sleep(30)";
        let signal = CancellationToken::new();
        let cancel = signal.clone();
        let task = run_in_temp(files.request("python", source), signal, None, &files.0);
        let readiness = files.0.join("child-ready");
        let (result, ready) = tokio::join!(task, async move {
            let ready = tokio::time::timeout(Duration::from_secs(5), async {
                while !readiness.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .is_ok();
            cancel.cancel();
            ready
        });
        assert!(ready, "child never reached its readiness marker");
        assert!(format!("{:#}", result.unwrap_err()).contains("canceled"));
        tokio::time::sleep(Duration::from_millis(2200)).await;
        assert!(!files.0.join("escaped-child").exists(), "canceled descendant wrote delayed effect");
        std::fs::remove_file(files.0.join("child-ready")).unwrap();
        files.empty();
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn real_powershell_utf8_quoting_exit_cancel_timeout_cleanup() {
        exercise("powershell", "Write-Output '中文 ''quote'' $ & ;'", "exit 7", "Start-Sleep -Seconds 30").await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_command_errors_and_native_exit_propagate() {
        let files = Files::new();
        for (source, expected) in
            [("Write-Error 'failure'; Write-Output 'unreachable'", "code 1"), ("& cmd.exe /d /c 'exit 7'", "code 7")]
        {
            let error = run_in_temp(files.request("powershell", source), CancellationToken::new(), None, &files.0)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            files.empty();
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_source_preserves_top_level_param_and_using() {
        let files = Files::new();
        for (source, expected) in [
            ("param([string]$Value = 'PARAM-DEFAULT')\nWrite-Output $Value", "PARAM-DEFAULT"),
            ("using namespace System.Text\n[StringBuilder]::new('USING-OK').ToString()", "USING-OK"),
        ] {
            let text = run_in_temp(files.request("powershell", source), CancellationToken::new(), None, &files.0)
                .await
                .unwrap();
            assert!(text.contains(expected), "{text}");
            files.empty();
        }
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn real_cmd_utf8_quoting_exit_cancel_timeout_cleanup() {
        exercise("cmd", "echo 中文 'quote' $ ^& ;", "exit /b 7", "ping -n 30 127.0.0.1 >nul").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn real_sh_utf8_quoting_exit_cancel_timeout_cleanup() {
        exercise("sh", "printf '%s\\n' \"中文 'quote' \\$ & ;\"", "exit 7", "sleep 30").await;
    }
}
