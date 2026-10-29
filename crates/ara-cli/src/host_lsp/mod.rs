//! Host LSP startup/settings factory, fixed OMP 596f2da sdk.ts:4036-4086.
//! Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md. This module has no product/Core state.

pub mod client;
pub mod config;
mod custom;
pub(crate) use custom::{OwnedProcess, spawn as spawn_owned_process, terminate as terminate_owned_process};
mod diagnostics;
pub mod mux;
pub mod types;
pub mod writethrough;

use crate::host_edit_session::EditSuppliedServices;
use ara_tools::edit_host::EditLspOptions;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspSettings {
    pub enabled: bool,
    pub shared: bool,
    pub lazy: bool,
    pub format_on_write: bool,
    pub diagnostics_on_edit: bool,
    pub diagnostics_on_write: bool,
    pub diagnostics_deduplicate: bool,
}
impl Default for LspSettings {
    fn default() -> Self {
        // Fixed settings-schema.ts:3786-3855. EDIT and WRITE deliberately
        // capture distinct diagnostics leaves from the same settings owner.
        Self {
            enabled: true,
            shared: true,
            lazy: true,
            format_on_write: false,
            diagnostics_on_edit: false,
            diagnostics_on_write: true,
            diagnostics_deduplicate: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct LspStartupSnapshot {
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub settings: LspSettings,
    pub config: Option<config::LspConfig>,
}
impl LspStartupSnapshot {
    pub fn load(cwd: &Path, agent_dir: &Path, settings: LspSettings) -> io::Result<Self> {
        // Fixed launch/paths.ts permits missing paths and EISDIR only. A
        // permissions/I/O failure must not silently create a different scope.
        let resolved = std::path::absolute(cwd)?;
        let mut lexical = PathBuf::new();
        for component in resolved.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    lexical.pop();
                }
                _ => lexical.push(component.as_os_str()),
            }
        }
        let cwd = match std::fs::canonicalize(&lexical) {
            Ok(real) => types::absolute(&real),
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::IsADirectory) => lexical,
            Err(error) => return Err(error),
        };
        let agent_dir = types::absolute(agent_dir);
        let config = if settings.enabled { Some(config::load_config(&cwd, &agent_dir)?) } else { None };
        Ok(Self { cwd, agent_dir, settings, config })
    }
}

/// Host tool-snapshot owner. It retains process services across `/new`, while
/// HostEditSession creates a fresh ledger. Changed settings create a new owner
/// without replacing an admitted tool's original supplier or callback.
pub struct HostLspStartup {
    pub snapshot: LspStartupSnapshot,
    pub pool: Option<Arc<client::ClientPool>>,
    writer: Option<Arc<writethrough::HostLspWritethrough>>,
    _project_presence: Option<Arc<mux::ProjectPresence>>,
    lifetime: CancellationToken,
    warmed: AtomicBool,
    warmup: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl HostLspStartup {
    pub fn new(snapshot: LspStartupSnapshot) -> Arc<Self> {
        // Independent project process lease is registered before any LSP cold
        // start, including disabled/lazy hosts. It creates no server or worker.
        let project_presence = match mux::register_project_presence(&snapshot.cwd, &snapshot.agent_dir) {
            Ok(presence) => Some(presence),
            Err(error) => {
                eprintln!("ara: LSP project presence registration failed: {error}");
                None
            }
        };
        let pool = snapshot.config.clone().map(|config| {
            client::ClientPool::new(snapshot.cwd.clone(), snapshot.agent_dir.clone(), config, snapshot.settings.shared)
        });
        let writer = pool.as_ref().map(|pool| writethrough::HostLspWritethrough::new(pool.clone()));
        Arc::new(Self {
            snapshot,
            pool,
            writer,
            _project_presence: project_presence,
            lifetime: CancellationToken::new(),
            warmed: AtomicBool::new(false),
            warmup: Mutex::new(None),
        })
    }
    pub fn supplied(&self) -> EditSuppliedServices {
        let settings = &self.snapshot.settings;
        let options = |diagnostics| EditLspOptions {
            enable_format: settings.format_on_write,
            enable_diagnostics: diagnostics,
            deduplicate_diagnostics: settings.diagnostics_deduplicate,
        };
        EditSuppliedServices {
            enable_lsp: Some(settings.enabled),
            lsp: self
                .writer
                .as_ref()
                .map(|writer| writer.clone() as Arc<dyn ara_tools::edit_host::EditLspWritethrough>),
            lsp_options: options(settings.diagnostics_on_edit),
            write_lsp_options: Some(options(settings.diagnostics_on_write)),
            ..Default::default()
        }
    }
    pub fn schedule_warmup(&self, has_ui: bool) {
        // Print/script/RPC skip warmup even with lazy=false. Availability is
        // configuration discovery; it must not pretend a process initialized.
        if !has_ui
            || self.snapshot.settings.lazy
            || !self.snapshot.settings.enabled
            || self.warmed.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let Some(pool) = self.pool.clone() else {
            return;
        };
        let cancel = self.lifetime.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = pool.warmup(Some(cancel)).await {
                eprintln!("ara: LSP warmup failed: {error}");
            }
        });
        *self.warmup.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
    }
    pub async fn shutdown(&self) -> io::Result<()> {
        self.lifetime.cancel();
        if let Some(writer) = &self.writer {
            writer.shutdown().await;
        }
        let task = self.warmup.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            let _ = task.await;
        }
        if let Some(pool) = &self.pool {
            pool.shutdown().await?;
        }
        Ok(())
    }
}
impl Drop for HostLspStartup {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}
