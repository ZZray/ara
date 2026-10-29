//! Host-only fixed OMP PDF/browser and Bun.Image primitive binding.
//! The shared Rust Core depends on ports, never on this process/package owner.
mod ports;
mod transport;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub use ports::{HostReadSession, ReadHostSnapshot};
use transport::Transport;

const BUN_SHA256: &str = "627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f";
const PACKAGE_SHA256: &str = "bee90ce2b3f3ed49ee284840895f040ed0a2c61b6dc86d562187d7b510326af9";

#[derive(Clone, Debug)]
pub struct HostBrowserOptions {
    pub bun: PathBuf,
    pub package_root: PathBuf,
    pub entry: PathBuf,
    pub runtime_root: PathBuf,
    pub temp_root: PathBuf,
}
impl HostBrowserOptions {
    /// Explicit Host distribution configuration. No home/OMP/live package fallback.
    pub fn from_environment() -> Result<Option<Self>> {
        let bun = std::env::var_os("ARA_PDF_BUN");
        let package = std::env::var_os("ARA_PDF_PACKAGE_ROOT");
        if bun.is_none() && package.is_none() {
            return Ok(None);
        }
        let bun = bun
            .context("ARA_PDF_BUN is required with the private Host package")?
            .into();
        let package_root: PathBuf = package
            .context("ARA_PDF_PACKAGE_ROOT is required with the private Bun")?
            .into();
        let runtime_root = std::env::var_os("ARA_PDF_HOST_ROOT")
            .context("ARA_PDF_HOST_ROOT is required")?
            .into();
        let temp_root = std::env::var_os("ARA_PDF_TEMP")
            .context("ARA_PDF_TEMP is required")?
            .into();
        Ok(Some(Self {
            bun,
            entry: package_root.join("host/entry.ts"),
            package_root,
            runtime_root,
            temp_root,
        }))
    }

    pub fn validate(&self) -> Result<()> {
        for (name, path) in [
            ("Bun", &self.bun),
            ("package", &self.package_root),
            ("runtime", &self.runtime_root),
            ("temp", &self.temp_root),
        ] {
            if !path.is_absolute()
                || !path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("d:")
            {
                bail!("Private PDF Host {name} must use an explicit absolute D path");
            }
        }
        if file_sha256(&self.bun)? != BUN_SHA256 {
            bail!("Private PDF Host Bun does not match the approved binary");
        }
        let manifest_path = self.package_root.join("SOURCE-MANIFEST-P-v7.json");
        if file_sha256(&manifest_path)? != PACKAGE_SHA256 {
            bail!("Private PDF Host package does not match the approved source manifest");
        }
        let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        let root = std::fs::canonicalize(&self.package_root)?;
        for record in manifest["files"]
            .as_array()
            .context("Private Host manifest files missing")?
        {
            let relative = Path::new(
                record["path"]
                    .as_str()
                    .context("Private Host manifest path missing")?,
            );
            if relative.is_absolute()
                || relative.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir | std::path::Component::Prefix(_)
                    )
                })
            {
                bail!("Private Host manifest contains an escaping path");
            }
            let file = root.join(relative);
            if !std::fs::canonicalize(&file)?.starts_with(&root) {
                bail!("Private Host manifest path escapes the approved package");
            }
            if file_sha256(&file)?
                != record["sha256"]
                    .as_str()
                    .context("Private Host file digest missing")?
            {
                bail!("Private Host source pin mismatch: {}", relative.display());
            }
        }
        if std::fs::canonicalize(&self.entry)? != std::fs::canonicalize(root.join("host/entry.ts"))?
        {
            bail!("Private Host entry is outside the approved package");
        }
        std::fs::create_dir_all(&self.runtime_root)?;
        std::fs::create_dir_all(&self.temp_root)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct HostCloseReceipt {
    pub owner_pid: u32,
    pub wrapper_pid: u32,
    pub project_dir: PathBuf,
    pub cleanup: Value,
}

pub struct HostBrowser {
    options: HostBrowserOptions,
    projects: tokio::sync::Mutex<HashMap<PathBuf, Arc<Transport>>>,
    closing: Mutex<bool>,
}
impl HostBrowser {
    /// Called at actual Agent Host startup, before the first READ/Run.
    pub async fn start(options: HostBrowserOptions, project: &Path) -> Result<Arc<Self>> {
        options.validate()?;
        let owner = Arc::new(Self {
            options,
            projects: Default::default(),
            closing: Mutex::new(false),
        });
        owner.project(project).await?;
        Ok(owner)
    }

    pub async fn from_environment(project: &Path) -> Result<Option<Arc<Self>>> {
        match HostBrowserOptions::from_environment()? {
            Some(options) => Self::start(options, project).await.map(Some),
            None => Ok(None),
        }
    }

    async fn project(&self, project: &Path) -> Result<Arc<Transport>> {
        if *self.closing.lock().unwrap() {
            bail!("PDF Host admission is closed");
        }
        let canonical = std::fs::canonicalize(project)?;
        // Startup is singleflight within this Host. The JS original broker
        // separately converges other processes for the same canonical project.
        let mut projects = self.projects.lock().await;
        if *self.closing.lock().unwrap() {
            bail!("PDF Host admission is closed");
        }
        if let Some(transport) = projects.get(&canonical) {
            if transport.is_closed() {
                bail!("PDF Host project wrapper disconnected; requests will not be replayed");
            }
            return Ok(transport.clone());
        }
        let transport = Transport::start(&self.options, &canonical).await?;
        projects.insert(canonical, transport.clone());
        Ok(transport)
    }

    pub async fn session(
        self: &Arc<Self>,
        project: &Path,
        snapshot: ReadHostSnapshot,
    ) -> Result<Arc<HostReadSession>> {
        let transport = self.project(project).await?;
        Ok(HostReadSession::new(transport, snapshot))
    }

    /// Close after active Runs/native jobs settle, including every ACP project.
    pub async fn close(&self) -> Result<Vec<HostCloseReceipt>> {
        *self.closing.lock().unwrap() = true;
        let transports = self
            .projects
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut receipts = Vec::new();
        let mut errors = Vec::new();
        for transport in transports {
            match transport.close().await {
                Ok(receipt) => receipts.push(receipt),
                Err(error) => {
                    errors.push(format!("{}: {error:#}", transport.project_dir.display()))
                }
            }
        }
        if !errors.is_empty() {
            bail!("PDF Host settlement failed: {}", errors.join("; "));
        }
        Ok(receipts)
    }
}

fn file_sha256(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Host pin {}", path.display()))?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
