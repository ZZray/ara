//! Synchronous host-side port of fixed OMP `config/model-config-values.ts`.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d (MIT). This is the
//! models configuration resolver, not the separate asynchronous resolver.
//! Successful command values are cached for the process, keyed only by the
//! JavaScript-trimmed command; failures retry after 30 seconds. The host supplies
//! the current project directory and exact-case environment on every read.
//!
//! The shell receives a termination signal at ten seconds. As with `execSync`,
//! Unix SIGTERM settlement can take longer if the child ignores the signal.
//! Killing that child does not establish containment or undo external effects.
//! Unlike Node's default `execSync`, raw stderr is never echoed: only safe error
//! codes are exposed to the host. Secret-bearing types have no Debug/Serialize.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const COMMAND_FAILURE_RETRY_MS: u64 = 30_000;
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
pub const COMMAND_MAX_BUFFER: usize = 1024 * 1024;

pub trait ConfigValueEnvironment {
    /// Exact spelling, including case on Windows, as in OMP `$envExact`.
    fn get(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> ConfigValueEnvironment for F {
    fn get(&self, name: &str) -> Option<String> {
        self(name)
    }
}

pub struct ProcessConfigEnvironment;

impl ConfigValueEnvironment for ProcessConfigEnvironment {
    fn get(&self, name: &str) -> Option<String> {
        // Windows' direct environment getter folds case; enumeration preserves
        // the spelling of the real key, which the fixed upstream checks too.
        std::env::vars_os()
            .find(|(key, _)| key == std::ffi::OsStr::new(name))
            .map(|(_, value)| value.to_string_lossy().into_owned())
    }
}

pub struct ConfigValueContext<'a> {
    pub project_dir: &'a Path,
    pub environment: &'a dyn ConfigValueEnvironment,
}

#[derive(Clone, Copy, Default)]
pub struct ResolveConfigValueOptions {
    pub force_command_refresh: bool,
}

pub struct CommandApiKeyResolution {
    pub configured: bool,
    pub value: Option<String>,
}

pub fn is_command_config_value(value_config: Option<&str>) -> bool {
    value_config.is_some_and(|value| value.starts_with('!'))
}

fn js_trim(value: &str) -> &str {
    value.trim_matches(|c| {
        matches!(c, '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
            | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
    })
}

pub trait ConfigValueClock: Send + Sync {
    fn now_millis(&self) -> u64;
}

pub struct SystemConfigValueClock;

impl ConfigValueClock for SystemConfigValueClock {
    fn now_millis(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64
    }
}

/// Closed, non-sensitive failure metadata. No invocation or output is retained.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConfigCommandFailure {
    NotFound,
    PermissionDenied,
    InvalidInput,
    NonZeroExit,
    Timeout,
    OutputLimit,
    Io,
    Runtime,
}

impl ConfigCommandFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFound => "ENOENT",
            Self::PermissionDenied => "EACCES",
            Self::InvalidInput => "EINVAL",
            Self::Timeout => "ETIMEDOUT",
            Self::OutputLimit => "ENOBUFS",
            Self::NonZeroExit | Self::Io | Self::Runtime => "unknown",
        }
    }
}

pub struct ConfigValueWarning {
    pub code: &'static str,
}

pub trait ConfigCommandExecutor: Send + Sync {
    fn directory_is_enterable(&self, project_dir: &Path) -> bool {
        directory_is_enterable(project_dir)
    }

    /// Raw UTF-8-decoded stdout. The resolver, not the executor, trims it.
    fn execute(&self, command: &str, project_dir: &Path) -> Result<String, ConfigCommandFailure>;
}

#[derive(Default)]
struct CommandCacheState {
    values: HashMap<String, String>,
    failure_retry_at: HashMap<String, u64>,
}

/// Can be shared explicitly by isolated test/embedded hosts. Production always
/// uses one process-wide cache, matching the fixed upstream module's Maps.
#[derive(Default)]
pub struct CommandConfigCache {
    state: Mutex<CommandCacheState>,
}

fn process_cache() -> Arc<CommandConfigCache> {
    static CACHE: OnceLock<Arc<CommandConfigCache>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(CommandConfigCache::default())).clone()
}

#[derive(Clone)]
pub struct ConfigValueResolver {
    cache: Arc<CommandConfigCache>,
    clock: Arc<dyn ConfigValueClock>,
    executor: Arc<dyn ConfigCommandExecutor>,
    warnings: Arc<Mutex<Vec<ConfigValueWarning>>>,
}

impl Default for ConfigValueResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigValueResolver {
    pub fn new() -> Self {
        Self::with_ports(process_cache(), Arc::new(SystemConfigValueClock), Arc::new(SyncShellCommandExecutor))
    }

    pub fn isolated() -> Self {
        Self::with_ports(
            Arc::new(CommandConfigCache::default()),
            Arc::new(SystemConfigValueClock),
            Arc::new(SyncShellCommandExecutor),
        )
    }

    pub fn with_ports(
        cache: Arc<CommandConfigCache>,
        clock: Arc<dyn ConfigValueClock>,
        executor: Arc<dyn ConfigCommandExecutor>,
    ) -> Self {
        Self { cache, clock, executor, warnings: Arc::new(Mutex::new(Vec::new())) }
    }

    pub fn resolve_config_value(
        &self,
        value_config: &str,
        context: &ConfigValueContext<'_>,
        options: ResolveConfigValueOptions,
    ) -> Option<String> {
        self.resolve_config_value_checked(value_config, context, options, &|| true).ok().flatten()
    }

    fn resolve_config_value_checked(
        &self,
        value_config: &str,
        context: &ConfigValueContext<'_>,
        options: ResolveConfigValueOptions,
        can_continue: &dyn Fn() -> bool,
    ) -> Result<Option<String>, ConfigHeaderCancelled> {
        if !can_continue() {
            return Err(ConfigHeaderCancelled);
        }
        let Some(command) = value_config.strip_prefix('!') else {
            return Ok(Some(
                context
                    .environment
                    .get(value_config)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| value_config.to_owned()),
            ));
        };
        let command = js_trim(command);
        // OMP's synchronous JavaScript execution serializes all command reads.
        // Keep execution, cache publication and invalidation under one mutex so
        // a completed older command cannot overwrite a forced refresh.
        let mut cache = self.cache.state.lock().unwrap_or_else(|error| error.into_inner());
        // A different synchronous command may have held the shared cache lock
        // while this Host was cancelled. Do not start queued work afterwards.
        if !can_continue() {
            return Err(ConfigHeaderCancelled);
        }
        if options.force_command_refresh {
            cache.values.remove(command);
            cache.failure_retry_at.remove(command);
        }
        if let Some(value) = cache.values.get(command) {
            return Ok(Some(value.clone()));
        }
        if cache.failure_retry_at.get(command).is_some_and(|retry_at| self.clock.now_millis() < *retry_at) {
            return Ok(None);
        }
        if !self.executor.directory_is_enterable(context.project_dir) {
            cache
                .failure_retry_at
                .insert(command.to_owned(), self.clock.now_millis().saturating_add(COMMAND_FAILURE_RETRY_MS));
            return Ok(None);
        }
        if !can_continue() {
            return Err(ConfigHeaderCancelled);
        }
        match self.executor.execute(command, context.project_dir) {
            Ok(stdout) => {
                let value = js_trim(&stdout);
                if !value.is_empty() {
                    cache.failure_retry_at.remove(command);
                    cache.values.insert(command.to_owned(), value.to_owned());
                    return Ok(Some(value.to_owned()));
                }
            }
            Err(failure) => {
                self.warnings
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(ConfigValueWarning { code: failure.code() });
            }
        }
        // TTL starts after the command settles, not when it was dispatched.
        cache
            .failure_retry_at
            .insert(command.to_owned(), self.clock.now_millis().saturating_add(COMMAND_FAILURE_RETRY_MS));
        Ok(None)
    }

    pub fn invalidate_command_config(&self, value_config: Option<&str>) {
        let Some(command) = value_config.and_then(|value| value.strip_prefix('!')).map(js_trim) else { return };
        let mut cache = self.cache.state.lock().unwrap_or_else(|error| error.into_inner());
        cache.values.remove(command);
        cache.failure_retry_at.remove(command);
    }

    pub fn take_warnings(&self) -> Vec<ConfigValueWarning> {
        std::mem::take(&mut *self.warnings.lock().unwrap_or_else(|error| error.into_inner()))
    }
}

pub fn resolve_config_value(
    value_config: &str,
    context: &ConfigValueContext<'_>,
    options: ResolveConfigValueOptions,
    resolver: &ConfigValueResolver,
) -> Option<String> {
    resolver.resolve_config_value(value_config, context, options)
}

pub fn invalidate_command_config(value_config: Option<&str>) {
    ConfigValueResolver::new().invalidate_command_config(value_config);
}

pub struct SyncShellCommandExecutor;

fn io_failure(error: std::io::Error) -> ConfigCommandFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => ConfigCommandFailure::NotFound,
        std::io::ErrorKind::PermissionDenied => ConfigCommandFailure::PermissionDenied,
        std::io::ErrorKind::InvalidInput => ConfigCommandFailure::InvalidInput,
        _ => ConfigCommandFailure::Io,
    }
}

fn directory_is_enterable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
        unsafe extern "C" {
            fn access(pathname: *const std::ffi::c_char, mode: std::ffi::c_int) -> std::ffi::c_int;
        }
        // OMP utils/dirs.ts uses access(X_OK), not an existence-only check.
        if unsafe { access(path.as_ptr(), 1) } != 0 {
            return false;
        }
    }
    // Windows access(X_OK) checks existence; execute permissions have no POSIX
    // equivalent there. Spawn still reports an inaccessible cwd safely.
    path.metadata().is_ok_and(|metadata| metadata.is_dir())
}

impl ConfigCommandExecutor for SyncShellCommandExecutor {
    fn execute(&self, command: &str, project_dir: &Path) -> Result<String, ConfigCommandFailure> {
        let command = command.to_owned();
        let project_dir = project_dir.to_owned();
        // The public contract remains synchronous even when called inside a
        // Tokio host. A private runtime gives pipe reads nonblocking ownership;
        // inherited pipes are dropped on timeout rather than joining readers.
        std::thread::Builder::new()
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| ConfigCommandFailure::Runtime)?;
                runtime.block_on(execute_shell(command, project_dir))
            })
            .map_err(io_failure)?
            .join()
            .map_err(|_| ConfigCommandFailure::Runtime)?
    }
}

fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Node's default comes from ComSpec. Its cmd-specific switches and
        // verbatim quoting differ from Bash and the separate async resolver.
        let shell = std::env::var_os("ComSpec").filter(|shell| !shell.is_empty()).unwrap_or_else(|| "cmd.exe".into());
        let name = shell.to_string_lossy();
        let basename = name.rsplit('\\').next().unwrap_or(&name);
        let is_cmd = basename.eq_ignore_ascii_case("cmd") || basename.eq_ignore_ascii_case("cmd.exe");
        let mut child = tokio::process::Command::new(shell);
        if is_cmd {
            child.as_std_mut().raw_arg(format!("/d /s /c \"{command}\""));
        } else {
            child.arg("-c").arg(command);
        }
        // windowsHide: no console window for the helper process.
        child.creation_flags(0x0800_0000);
        child
    }
    #[cfg(not(windows))]
    {
        let mut child =
            tokio::process::Command::new(if cfg!(target_os = "android") { "/system/bin/sh" } else { "/bin/sh" });
        child.arg("-c").arg(command);
        child
    }
}

async fn read_output(
    mut reader: impl AsyncRead + Unpin,
    collect: bool,
    total: &std::cell::Cell<usize>,
) -> Result<Vec<u8>, ConfigCommandFailure> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await.map_err(io_failure)?;
        if count == 0 {
            return Ok(output);
        }
        let next = total.get().saturating_add(count);
        total.set(next);
        // spawnSync counts stdout and stderr together, as directly probed with
        // Node v24.18.0. No stderr payload is retained or echoed.
        if next > COMMAND_MAX_BUFFER {
            return Err(ConfigCommandFailure::OutputLimit);
        }
        if collect {
            output.extend_from_slice(&buffer[..count]);
        }
    }
}

async fn terminate_shell(child: &mut tokio::process::Child) -> Result<(), ConfigCommandFailure> {
    let mut signal_failure = None;
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe extern "C" {
            fn kill(pid: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
        }
        // Node's default killSignal is SIGTERM. Deliberately do not escalate to
        // SIGKILL or create a process group not present in the upstream code.
        let result = unsafe { kill(pid as std::ffi::c_int, 15) };
        if result != 0 {
            signal_failure = Some(io_failure(std::io::Error::last_os_error()));
        }
    }
    #[cfg(windows)]
    if let Err(error) = child.start_kill() {
        signal_failure = Some(io_failure(error));
    }
    // A shell can exit between timeout and signal dispatch. Even if sending
    // the signal fails, retain ownership until that direct child is reaped.
    child.wait().await.map_err(io_failure)?;
    signal_failure.map_or(Ok(()), Err)
}

async fn execute_shell(command: String, project_dir: std::path::PathBuf) -> Result<String, ConfigCommandFailure> {
    let mut process = shell_command(&command);
    process.current_dir(project_dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = process.spawn().map_err(io_failure)?;
    drop(process);
    let stdout = child.stdout.take().ok_or(ConfigCommandFailure::Io)?;
    let stderr = child.stderr.take().ok_or(ConfigCommandFailure::Io)?;
    let total = std::cell::Cell::new(0_usize);
    let settled = tokio::time::timeout(COMMAND_TIMEOUT, async {
        tokio::try_join!(
            async { child.wait().await.map_err(io_failure) },
            read_output(stdout, true, &total),
            read_output(stderr, false, &total),
        )
    })
    .await;
    match settled {
        Ok(Ok((status, stdout, _))) if status.success() => Ok(String::from_utf8_lossy(&stdout).into_owned()),
        Ok(Ok(_)) => Err(ConfigCommandFailure::NonZeroExit),
        Ok(Err(failure)) => {
            // The output futures and their pipes have been dropped. Only the
            // directly spawned shell is signalled and then awaited.
            let _ = terminate_shell(&mut child).await;
            Err(failure)
        }
        Err(_) => {
            let _ = terminate_shell(&mut child).await;
            Err(ConfigCommandFailure::Timeout)
        }
    }
}

#[derive(Clone, Default)]
struct HeaderEntries(Vec<(String, String)>);

impl HeaderEntries {
    fn set(&mut self, key: String, value: String) {
        if let Some((_, old)) = self.0.iter_mut().find(|(name, _)| name == &key) {
            *old = value;
        } else {
            self.0.push((key, value));
        }
    }

    fn delete(&mut self, key: &str) -> bool {
        let old_len = self.0.len();
        self.0.retain(|(name, _)| name != key);
        self.0.len() != old_len
    }

    fn ordered(&self) -> Vec<(String, String)> {
        // JS record enumeration puts canonical array-index keys first and
        // preserves insertion order for the other strings.
        fn index(key: &str) -> Option<u32> {
            let value = key.parse::<u32>().ok()?;
            (value != u32::MAX && value.to_string() == key).then_some(value)
        }
        let mut pairs = self.0.clone();
        pairs.sort_by_key(|(key, _)| index(key).map(|n| (false, n)).unwrap_or((true, 0)));
        pairs
    }
}

/// Retains the original mutable configuration record across all live wrappers.
#[derive(Clone, Default)]
pub struct HeaderConfigRecord {
    values: Arc<RwLock<HeaderEntries>>,
}

impl HeaderConfigRecord {
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        let record = Self::default();
        for (name, value) in pairs {
            record.set(name, value);
        }
        record
    }

    pub fn set(&self, key: impl Into<String>, value: impl Into<String>) {
        self.values.write().unwrap_or_else(|error| error.into_inner()).set(key.into(), value.into());
    }

    pub fn delete(&self, key: &str) -> bool {
        self.values.write().unwrap_or_else(|error| error.into_inner()).delete(key)
    }

    pub(crate) fn snapshot(&self) -> Vec<(String, String)> {
        self.values.read().unwrap_or_else(|error| error.into_inner()).ordered()
    }
}

#[derive(Clone)]
pub enum HeaderSource {
    Config(HeaderConfigRecord),
    Live(LiveConfigHeaders),
}

impl HeaderSource {
    /// Auth preflight can inspect configured ownership without resolving a
    /// value, invoking a helper, or exposing private configuration values.
    pub(crate) fn configured_header_names(&self) -> Vec<String> {
        match self {
            Self::Config(record) => record.snapshot().into_iter().map(|(name, _)| name).collect(),
            Self::Live(headers) => headers.configured_header_names(),
        }
    }
}

#[derive(Clone, Default)]
pub struct HeaderResolutionOptions {
    pub auth_header: bool,
    pub api_key_config: Option<String>,
}

/// A snapshot is already materialized. Merging it never resolves values again.
#[derive(Clone)]
pub struct ResolvedConfigHeaders {
    entries: Vec<(String, String)>,
}

/// Host cancellation stops unstarted configuration work. No configuration or
/// command value is carried by this error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigHeaderCancelled;

impl ResolvedConfigHeaders {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries.iter().map(|(name, _)| name.clone()).collect()
    }

    pub fn pairs(&self) -> &[(String, String)] {
        &self.entries
    }

    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.entries
    }
}

pub struct ConfigHeaderProperty {
    pub configurable: bool,
    pub enumerable: bool,
    pub value: String,
    pub writable: bool,
}

struct LiveHeaderState {
    sources: Vec<HeaderSource>,
    local: HeaderConfigRecord,
    options: HeaderResolutionOptions,
}

#[derive(Clone)]
pub struct LiveConfigHeaders {
    state: Arc<LiveHeaderState>,
}

pub fn create_live_config_headers(
    sources: &[Option<HeaderSource>],
    options: HeaderResolutionOptions,
) -> Option<LiveConfigHeaders> {
    let sources: Vec<_> = sources.iter().flatten().cloned().collect();
    if sources.is_empty()
        && (!options.auth_header || !options.api_key_config.as_ref().is_some_and(|key| !key.is_empty()))
    {
        return None;
    }
    Some(LiveConfigHeaders {
        state: Arc::new(LiveHeaderState { sources, local: HeaderConfigRecord::default(), options }),
    })
}

fn materialize_sources(
    sources: &[HeaderSource],
    local: Option<&HeaderConfigRecord>,
    options: &HeaderResolutionOptions,
    resolver: &ConfigValueResolver,
    context: &ConfigValueContext<'_>,
) -> Option<ResolvedConfigHeaders> {
    materialize_sources_checked(sources, local, options, resolver, context, &|| true).ok().flatten()
}

fn materialize_sources_checked(
    sources: &[HeaderSource],
    local: Option<&HeaderConfigRecord>,
    options: &HeaderResolutionOptions,
    resolver: &ConfigValueResolver,
    context: &ConfigValueContext<'_>,
    can_continue: &dyn Fn() -> bool,
) -> Result<Option<ResolvedConfigHeaders>, ConfigHeaderCancelled> {
    if !can_continue() {
        return Err(ConfigHeaderCancelled);
    }
    let mut resolved = HeaderEntries::default();
    fn resolve_record(
        record: &HeaderConfigRecord,
        resolved: &mut HeaderEntries,
        resolver: &ConfigValueResolver,
        context: &ConfigValueContext<'_>,
        can_continue: &dyn Fn() -> bool,
    ) -> Result<(), ConfigHeaderCancelled> {
        // Release the record lock before invoking a command or nested source.
        for (key, config) in record.snapshot() {
            if !can_continue() {
                return Err(ConfigHeaderCancelled);
            }
            // A started synchronous helper must settle before observing a
            // cancellation; the next configured value gets its own gate.
            if let Some(value) = resolver
                .resolve_config_value_checked(&config, context, ResolveConfigValueOptions::default(), can_continue)?
                .filter(|value| !value.is_empty())
            {
                resolved.set(key, value);
            }
        }
        Ok(())
    }
    for source in sources {
        if !can_continue() {
            return Err(ConfigHeaderCancelled);
        }
        match source {
            HeaderSource::Config(record) => resolve_record(record, &mut resolved, resolver, context, can_continue)?,
            HeaderSource::Live(headers) => {
                // Hidden OMP LIVE_HEADER_RESOLVER equivalent: one complete
                // snapshot, never trap/resolve each nested property separately.
                if let Some(snapshot) = headers.snapshot_checked(resolver, context, can_continue)? {
                    for (key, value) in snapshot.into_pairs() {
                        resolved.set(key, value);
                    }
                }
            }
        }
    }
    if let Some(local) = local {
        resolve_record(local, &mut resolved, resolver, context, can_continue)?;
    }
    if options.auth_header
        && let Some(config) = options.api_key_config.as_ref().filter(|key| !key.is_empty())
    {
        if !can_continue() {
            return Err(ConfigHeaderCancelled);
        }
        if let Some(key) = resolver
            .resolve_config_value_checked(config, context, ResolveConfigValueOptions::default(), can_continue)?
            .filter(|key| !key.is_empty())
        {
            resolved.set("Authorization".into(), format!("Bearer {key}"));
        }
    }
    if !can_continue() {
        return Err(ConfigHeaderCancelled);
    }
    Ok((!resolved.0.is_empty()).then(|| ResolvedConfigHeaders { entries: resolved.ordered() }))
}

impl LiveConfigHeaders {
    pub(crate) fn configured_header_names(&self) -> Vec<String> {
        let mut names = HeaderEntries::default();
        for source in &self.state.sources {
            for name in source.configured_header_names() {
                names.set(name, String::new());
            }
        }
        for (name, _) in self.state.local.snapshot() {
            names.set(name, String::new());
        }
        if self.state.options.auth_header
            && self.state.options.api_key_config.as_ref().is_some_and(|key| !key.is_empty())
        {
            names.set("Authorization".into(), String::new());
        }
        names.ordered().into_iter().map(|(name, _)| name).collect()
    }

    pub fn snapshot(
        &self,
        resolver: &ConfigValueResolver,
        context: &ConfigValueContext<'_>,
    ) -> Option<ResolvedConfigHeaders> {
        materialize_sources(&self.state.sources, Some(&self.state.local), &self.state.options, resolver, context)
    }

    /// Host lifecycle adapter for the synchronous resolver. Check before each
    /// configured value across nested sources, local overlays and auth headers;
    /// already-started helpers settle normally before cancellation is returned.
    pub fn snapshot_checked(
        &self,
        resolver: &ConfigValueResolver,
        context: &ConfigValueContext<'_>,
        can_continue: &dyn Fn() -> bool,
    ) -> Result<Option<ResolvedConfigHeaders>, ConfigHeaderCancelled> {
        materialize_sources_checked(
            &self.state.sources,
            Some(&self.state.local),
            &self.state.options,
            resolver,
            context,
            can_continue,
        )
    }

    pub fn get(&self, key: &str, resolver: &ConfigValueResolver, context: &ConfigValueContext<'_>) -> Option<String> {
        self.snapshot(resolver, context).and_then(|headers| headers.get(key).map(str::to_owned))
    }

    pub fn has(&self, key: &str, resolver: &ConfigValueResolver, context: &ConfigValueContext<'_>) -> bool {
        self.get(key, resolver, context).is_some()
    }

    pub fn keys(&self, resolver: &ConfigValueResolver, context: &ConfigValueContext<'_>) -> Vec<String> {
        self.snapshot(resolver, context).map(|headers| headers.keys()).unwrap_or_default()
    }

    pub fn set(&self, key: impl Into<String>, value: impl Into<String>) -> bool {
        self.state.local.set(key, value);
        true
    }

    /// As in the Proxy delete trap, deleting an absent local key succeeds and
    /// deleting an overlay reveals the original source value on the next read.
    pub fn delete(&self, key: &str) -> bool {
        self.state.local.delete(key);
        true
    }

    pub fn get_own_property_descriptor(
        &self,
        key: &str,
        resolver: &ConfigValueResolver,
        context: &ConfigValueContext<'_>,
    ) -> Option<ConfigHeaderProperty> {
        self.get(key, resolver, context).map(|value| ConfigHeaderProperty {
            configurable: true,
            enumerable: true,
            value,
            writable: true,
        })
    }
}

pub fn resolve_config_headers(
    headers: Option<&HeaderSource>,
    resolver: &ConfigValueResolver,
    context: &ConfigValueContext<'_>,
) -> Option<ResolvedConfigHeaders> {
    materialize_sources(
        headers.map(std::slice::from_ref).unwrap_or_default(),
        None,
        &HeaderResolutionOptions::default(),
        resolver,
        context,
    )
}
