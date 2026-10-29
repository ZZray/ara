//! Fixed Host argv through Core's public process-tree owner. No model shell API.
use anyhow::{ensure, Result};
use ara_agent_lite_harness::core;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn save_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
pub fn text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) {
        String::from_utf16_lossy(
            &bytes[2..].chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(bytes).trim_start_matches('\u{feff}').to_owned()
    }
}
pub fn require_program(path: &Path) -> Result<()> {
    ensure!(path.is_absolute() && path.is_file(), "explicit absolute executable file required: {}", path.display());
    ensure!(
        !path.components().any(|part| matches!(part, std::path::Component::ParentDir)),
        "parent traversal in executable path"
    );
    Ok(())
}
fn quote(value: &str) -> String {
    if cfg!(windows) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}
#[derive(Clone, Serialize)]
pub struct Record {
    pub sequence: usize,
    pub label: String,
    pub program: PathBuf,
    pub program_sha256: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub status: String,
    pub exit_code: Option<i32>,
    pub core_process_future_joined: bool,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stdout_present: bool,
    pub stderr_present: bool,
    pub stream_capture: String,
    pub receipt_path: PathBuf,
    pub control_output_path: PathBuf,
    pub control_output_sha256: String,
}
impl Record {
    pub fn success(&self) -> bool {
        self.status == "exited" && self.exit_code == Some(0)
    }
    pub fn stdout(&self) -> Result<String> {
        Ok(text(&fs::read(&self.stdout_path)?))
    }
}
pub struct Processes {
    directory: PathBuf,
    next: AtomicUsize,
    unknown: AtomicBool,
    pub timeout: Duration,
}
impl Processes {
    pub fn new(directory: PathBuf, timeout: Duration) -> Result<Self> {
        fs::create_dir(&directory)?;
        Ok(Self { directory, next: AtomicUsize::new(0), unknown: AtomicBool::new(false), timeout })
    }
    pub fn unknown(&self) -> bool {
        self.unknown.load(Ordering::SeqCst)
    }
    pub fn mark_unknown(&self) {
        self.unknown.store(true, Ordering::SeqCst);
    }
    pub async fn run(
        &self,
        label: &str,
        program: &Path,
        args: Vec<String>,
        cwd: &Path,
        signal: CancellationToken,
    ) -> Result<Record> {
        require_program(program)?;
        let program_sha256 = hash(&fs::read(program)?);
        let sequence = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let directory = self.directory.join(format!("process-{sequence}"));
        fs::create_dir(&directory)?;
        let stdout_path = directory.join("stdout.bin");
        let stderr_path = directory.join("stderr.bin");
        let receipt_path = directory.join("receipt.json");
        let control_output_path = directory.join("core-control-output.txt");
        let invocation = std::iter::once(quote(&program.to_string_lossy()))
            .chain(args.iter().map(|arg| quote(arg)))
            .collect::<Vec<_>>()
            .join(" ");
        // Only Host-selected executables and argv enter this shell. Core owns
        // the shell and its descendant tree, cancellation, timeout and joining.
        // Windows PowerShell redirects native output with its own encoding;
        // preserve those actual bytes, with this encoding limitation explicit.
        let command = if cfg!(windows) {
            format!("$env:ARA_CAPABILITY_MODEL_ENDPOINT=$null; $env:ARA_CAPABILITY_MODEL_ID=$null; $env:ARA_CAPABILITY_MODEL_KEY=$null; $global:LASTEXITCODE=237; & {invocation} 1> {} 2> {}; exit $LASTEXITCODE",quote(&stdout_path.to_string_lossy()),quote(&stderr_path.to_string_lossy()))
        } else {
            format!("unset ARA_CAPABILITY_MODEL_ENDPOINT ARA_CAPABILITY_MODEL_ID ARA_CAPABILITY_MODEL_KEY; {invocation} > {} 2> {}; exit $?",quote(&stdout_path.to_string_lossy()),quote(&stderr_path.to_string_lossy()))
        };
        let outcome = core::local_tools::run_shell(&command, cwd, signal, self.timeout).await;
        let (mut status, exit_code, control_output) = match outcome {
            Ok((output, code)) => ("exited".to_owned(), Some(code), output),
            Err(error) => {
                self.unknown.store(true, Ordering::SeqCst);
                ("effects_unknown".to_owned(), None, format!("{error:#}"))
            }
        };
        let stdout_present = stdout_path.is_file();
        let stderr_present = stderr_path.is_file();
        let stdout = fs::read(&stdout_path);
        let stderr = fs::read(&stderr_path);
        if stdout.is_err() || stderr.is_err() {
            self.unknown.store(true, Ordering::SeqCst);
            status = "effects_unknown".into();
        }
        let stdout = stdout.unwrap_or_default();
        let stderr = stderr.unwrap_or_default();
        if fs::write(&control_output_path, &control_output).is_err() {
            self.unknown.store(true, Ordering::SeqCst);
            status = "effects_unknown".into();
        }
        let record=Record{sequence,label:label.into(),program:program.into(),program_sha256,argv:args,cwd:cwd.into(),status,exit_code,
            core_process_future_joined:true,stdout_path,stderr_path,stdout_sha256:hash(&stdout),stderr_sha256:hash(&stderr),stdout_present,stderr_present,
            stream_capture:"Core-owned fixed shell with separate redirected streams; Windows redirection may encode UTF-16; exit is shell-propagated child status".into(),receipt_path:receipt_path.clone(),control_output_path,control_output_sha256:hash(control_output.as_bytes())};
        if let Err(error) = save_new(&receipt_path, &record) {
            self.unknown.store(true, Ordering::SeqCst);
            return Err(error);
        }
        Ok(record)
    }
}
