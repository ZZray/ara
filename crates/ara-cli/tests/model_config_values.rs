//! Native scenario families for fixed OMP model-config-values.ts. Host
//! request/401/cancellation integration is exercised by the owning CLI tests.

use ara_cli::model_config_values::{
    COMMAND_FAILURE_RETRY_MS, COMMAND_TIMEOUT, CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure,
    ConfigHeaderCancelled, ConfigValueClock, ConfigValueContext, ConfigValueEnvironment, ConfigValueResolver,
    HeaderConfigRecord, HeaderResolutionOptions, HeaderSource, ProcessConfigEnvironment, ResolveConfigValueOptions,
    create_live_config_headers, is_command_config_value, resolve_config_headers,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn no_env(_: &str) -> Option<String> {
    None
}

struct Clock(AtomicU64);
impl ConfigValueClock for Clock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Executor {
    enterable: AtomicBool,
    calls: AtomicUsize,
    output: Mutex<Result<String, ConfigCommandFailure>>,
    commands: Mutex<Vec<String>>,
    clock: Arc<Clock>,
}

impl ConfigCommandExecutor for Executor {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        self.enterable.load(Ordering::SeqCst)
    }

    fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.commands.lock().unwrap().push(command.to_owned());
        self.clock.0.fetch_add(1000, Ordering::SeqCst);
        self.output.lock().unwrap().clone()
    }
}

fn ports(output: Result<String, ConfigCommandFailure>) -> (ConfigValueResolver, Arc<Clock>, Arc<Executor>) {
    let clock = Arc::new(Clock(AtomicU64::new(1000)));
    let executor = Arc::new(Executor {
        enterable: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
        output: Mutex::new(output),
        commands: Mutex::new(Vec::new()),
        clock: clock.clone(),
    });
    let resolver =
        ConfigValueResolver::with_ports(Arc::new(CommandConfigCache::default()), clock.clone(), executor.clone());
    (resolver, clock, executor)
}

#[test]
fn value_resolution_cache_refresh_ttl_and_invalidation_are_serialized() {
    let (resolver, clock, executor) = ports(Err(ConfigCommandFailure::NonZeroExit));
    let env_values = HashMap::from([("EXACT_NAME", "environment"), ("EMPTY_NAME", "")]);
    let env = |name: &str| env_values.get(name).map(|value| (*value).to_owned());
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &env };
    let options = ResolveConfigValueOptions::default();
    assert!(is_command_config_value(Some("!command")));
    assert!(!is_command_config_value(None));
    assert!(!is_command_config_value(Some(" !command")));
    for (input, expected) in [
        ("EXACT_NAME", "environment"),
        ("exact_name", "exact_name"),
        ("EMPTY_NAME", "EMPTY_NAME"),
        ("literal", "literal"),
        ("", ""),
    ] {
        assert_eq!(resolver.resolve_config_value(input, &context, options).as_deref(), Some(expected));
    }
    // Read the actual process's key spelling without changing global env.
    let environment = ProcessConfigEnvironment;
    if let Some((name, value)) = std::env::vars_os().find_map(|(key, value)| {
        let key = key.into_string().ok()?;
        key.chars().any(|c| c.is_ascii_uppercase()).then_some((key, value.to_string_lossy().into_owned()))
    }) {
        assert_eq!(environment.get(&name).as_deref(), Some(value.as_str()));
        let alternate = name.to_ascii_lowercase();
        if !std::env::vars_os().any(|(key, _)| key == std::ffi::OsStr::new(&alternate)) {
            assert!(environment.get(&alternate).is_none());
        }
    }

    let config = "!\u{FEFF}\u{A0}mint-token\u{3000}\n";
    assert!(resolver.resolve_config_value(config, &context, options).is_none());
    assert_eq!(executor.commands.lock().unwrap().as_slice(), ["mint-token"]);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    let retry_at = 2000 + COMMAND_FAILURE_RETRY_MS;
    clock.0.store(retry_at - 1, Ordering::SeqCst);
    assert!(resolver.resolve_config_value("!mint-token", &context, options).is_none());
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    *executor.output.lock().unwrap() = Ok("\u{FEFF}  fresh-token \r\n".into());
    clock.0.store(retry_at, Ordering::SeqCst);
    assert_eq!(resolver.resolve_config_value("!mint-token", &context, options).as_deref(), Some("fresh-token"));
    *executor.output.lock().unwrap() = Ok("rotated-token".into());
    clock.0.store(u32::MAX.into(), Ordering::SeqCst);
    assert_eq!(resolver.resolve_config_value(config, &context, options).as_deref(), Some("fresh-token"));
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        resolver
            .resolve_config_value(config, &context, ResolveConfigValueOptions { force_command_refresh: true })
            .as_deref(),
        Some("rotated-token")
    );
    resolver.invalidate_command_config(Some("not-a-command"));
    resolver.invalidate_command_config(None);
    resolver.invalidate_command_config(Some(config));
    *executor.output.lock().unwrap() = Ok("\u{0085}preserved\u{0085}".into());
    assert_eq!(resolver.resolve_config_value(config, &context, options).as_deref(), Some("\u{0085}preserved\u{0085}"));
    assert_eq!(resolver.take_warnings().iter().map(|warning| warning.code).collect::<Vec<_>>(), ["unknown"]);
    // Empty output and an inaccessible project both back off without warnings.
    resolver.invalidate_command_config(Some(config));
    *executor.output.lock().unwrap() = Ok("\u{FEFF}\n".into());
    assert!(resolver.resolve_config_value(config, &context, options).is_none());
    let before = executor.calls.load(Ordering::SeqCst);
    assert!(resolver.resolve_config_value(config, &context, options).is_none());
    assert_eq!(executor.calls.load(Ordering::SeqCst), before);
    resolver.invalidate_command_config(Some(config));
    executor.enterable.store(false, Ordering::SeqCst);
    assert!(resolver.resolve_config_value(config, &context, options).is_none());
    assert_eq!(executor.calls.load(Ordering::SeqCst), before);
    assert!(resolver.take_warnings().is_empty());
    executor.enterable.store(true, Ordering::SeqCst);
    *executor.output.lock().unwrap() = Ok("recovered".into());
    assert_eq!(
        resolver
            .resolve_config_value(config, &context, ResolveConfigValueOptions { force_command_refresh: true })
            .as_deref(),
        Some("recovered")
    );

    struct BlockingExecutor {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        calls: AtomicUsize,
    }
    impl ConfigCommandExecutor for BlockingExecutor {
        fn execute(&self, _: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv_timeout(Duration::from_secs(3)).unwrap();
            }
            Ok(if call == 0 { "old" } else { "new" }.into())
        }
    }
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let blocking =
        Arc::new(BlockingExecutor { entered: entered_tx, release: Mutex::new(release_rx), calls: AtomicUsize::new(0) });
    let concurrent = ConfigValueResolver::with_ports(Arc::new(CommandConfigCache::default()), clock, blocking.clone());
    let first = concurrent.clone();
    let first = std::thread::spawn(move || {
        first.resolve_config_value(
            "!single-flight",
            &ConfigValueContext { project_dir: Path::new("."), environment: &no_env },
            options,
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let invalidate = concurrent.clone();
    let (attempted_tx, attempted_rx) = mpsc::channel();
    let invalidate = std::thread::spawn(move || {
        attempted_tx.send(()).unwrap();
        invalidate.invalidate_command_config(Some("!single-flight"));
    });
    attempted_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(first.join().unwrap().as_deref(), Some("old"));
    invalidate.join().unwrap();
    assert_eq!(concurrent.resolve_config_value("!single-flight", &context, options).as_deref(), Some("new"));
    assert_eq!(blocking.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn live_header_sources_snapshot_once_and_preserve_mutable_overlays() {
    let (resolver, _, executor) = ports(Err(ConfigCommandFailure::NonZeroExit));
    let values = Mutex::new(HashMap::from([
        ("ENV_HEADER".to_owned(), "literal-token".to_owned()),
        ("literal-token".to_owned(), "must-not-reresolve".to_owned()),
        ("AUTH_KEY".to_owned(), "bearer-one".to_owned()),
    ]));
    let env_calls = AtomicUsize::new(0);
    let env = |name: &str| {
        env_calls.fetch_add(1, Ordering::SeqCst);
        values.lock().unwrap().get(name).cloned()
    };
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &env };
    assert!(create_live_config_headers(&[], HeaderResolutionOptions::default()).is_none());
    assert!(
        create_live_config_headers(
            &[],
            HeaderResolutionOptions { auth_header: true, api_key_config: Some(String::new()) }
        )
        .is_none()
    );
    assert!(resolve_config_headers(None, &resolver, &context).is_none());
    let empty = create_live_config_headers(
        &[Some(HeaderSource::Config(HeaderConfigRecord::default()))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    assert!(empty.snapshot(&resolver, &context).is_none());
    let original = HeaderConfigRecord::from_pairs(vec![
        ("X-Token".into(), "ENV_HEADER".into()),
        ("X-Shared".into(), "original".into()),
        ("Authorization".into(), "source-authorization".into()),
        ("10".into(), "ten".into()),
        ("2".into(), "two".into()),
    ]);
    let overriding =
        HeaderConfigRecord::from_pairs(vec![("X-Shared".into(), "!failure".into()), ("X-Empty".into(), String::new())]);
    let base = create_live_config_headers(
        &[Some(HeaderSource::Config(original.clone())), None, Some(HeaderSource::Config(overriding))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    let mut deep = base.clone();
    for _ in 0..24 {
        deep =
            create_live_config_headers(&[Some(HeaderSource::Live(deep))], HeaderResolutionOptions::default()).unwrap();
    }
    env_calls.store(0, Ordering::SeqCst);
    let snapshot = deep.snapshot(&resolver, &context).unwrap();
    assert_eq!(snapshot.get("X-Token"), Some("literal-token"));
    assert_eq!(snapshot.get("X-Shared"), Some("original"));
    assert!(snapshot.get("X-Empty").is_none());
    assert_eq!(env_calls.load(Ordering::SeqCst), 6, "nested snapshots resolve each original config once");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    assert_eq!(snapshot.keys(), ["2", "10", "X-Token", "X-Shared", "Authorization"]);
    original.set("X-Added", "added-now");
    values.lock().unwrap().insert("ENV_HEADER".into(), "rotated-header".into());
    assert_eq!(deep.get("X-Token", &resolver, &context).as_deref(), Some("rotated-header"));
    assert!(deep.has("X-Added", &resolver, &context));
    assert!(deep.set("X-Shared", "ENV_HEADER"));
    assert_eq!(deep.get("X-Shared", &resolver, &context).as_deref(), Some("rotated-header"));
    let descriptor = deep.get_own_property_descriptor("X-Shared", &resolver, &context).unwrap();
    assert!(descriptor.configurable && descriptor.enumerable && descriptor.writable);
    assert_eq!(descriptor.value, "rotated-header");
    assert!(deep.delete("X-Shared"));
    assert!(deep.delete("does-not-exist"));
    assert_eq!(deep.get("X-Shared", &resolver, &context).as_deref(), Some("original"));
    assert!(deep.get_own_property_descriptor("does-not-exist", &resolver, &context).is_none());
    original.delete("X-Added");
    assert!(!deep.has("X-Added", &resolver, &context));

    let auth = create_live_config_headers(
        &[Some(HeaderSource::Live(deep))],
        HeaderResolutionOptions { auth_header: true, api_key_config: Some("AUTH_KEY".into()) },
    )
    .unwrap();
    auth.set("Authorization", "local-authorization");
    assert_eq!(auth.get("Authorization", &resolver, &context).as_deref(), Some("Bearer bearer-one"));
    values.lock().unwrap().insert("AUTH_KEY".into(), "bearer-two".into());
    assert_eq!(auth.get("Authorization", &resolver, &context).as_deref(), Some("Bearer bearer-two"));
    let failed_auth = create_live_config_headers(
        &[Some(HeaderSource::Live(auth))],
        HeaderResolutionOptions { auth_header: true, api_key_config: Some("!failure".into()) },
    )
    .unwrap();
    assert_eq!(failed_auth.get("Authorization", &resolver, &context).as_deref(), Some("Bearer bearer-two"));
    let source = HeaderSource::Live(failed_auth);
    assert_eq!(
        resolve_config_headers(Some(&source), &resolver, &context).unwrap().get("X-Token"),
        Some("rotated-header")
    );
}

#[test]
fn checked_header_snapshots_stop_unstarted_nested_overlay_and_auth_work() {
    struct CancellingExecutor {
        can_continue: Arc<AtomicBool>,
        commands: Mutex<Vec<String>>,
        settled: AtomicBool,
    }
    impl ConfigCommandExecutor for CancellingExecutor {
        fn directory_is_enterable(&self, _: &Path) -> bool {
            true
        }
        fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
            self.commands.lock().unwrap().push(command.to_owned());
            if command == "stop" {
                self.can_continue.store(false, Ordering::SeqCst);
                // Complete this already-started synchronous invocation before
                // the materializer can return cancellation to its Host.
                self.settled.store(true, Ordering::SeqCst);
            }
            Ok(format!("resolved-{command}"))
        }
    }
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &no_env };
    for (name, stop_in_overlay, later_value) in [
        ("nested values", false, true),
        ("nested auth", false, false),
        ("overlay values", true, true),
        ("overlay auth", true, false),
    ] {
        let can_continue = Arc::new(AtomicBool::new(true));
        let executor = Arc::new(CancellingExecutor {
            can_continue: Arc::clone(&can_continue),
            commands: Mutex::new(Vec::new()),
            settled: AtomicBool::new(false),
        });
        let resolver = ConfigValueResolver::with_ports(
            Arc::new(CommandConfigCache::default()),
            Arc::new(Clock(AtomicU64::new(1000))),
            executor.clone(),
        );
        let first_record = HeaderConfigRecord::from_pairs(vec![(
            "X-First".into(),
            if stop_in_overlay { "literal-first" } else { "!stop" }.into(),
        )]);
        if !stop_in_overlay && later_value {
            first_record.set("X-Second", "!nested-second");
        }
        let first = create_live_config_headers(
            &[Some(HeaderSource::Config(first_record))],
            if stop_in_overlay {
                HeaderResolutionOptions::default()
            } else {
                HeaderResolutionOptions { auth_header: true, api_key_config: Some("!nested-auth".into()) }
            },
        )
        .unwrap();
        let nested =
            create_live_config_headers(&[Some(HeaderSource::Live(first))], HeaderResolutionOptions::default()).unwrap();
        let later_source = HeaderConfigRecord::from_pairs(vec![(
            "X-Later".into(),
            if stop_in_overlay { "literal-later" } else { "!later-source" }.into(),
        )]);
        let headers = create_live_config_headers(
            &[Some(HeaderSource::Live(nested)), Some(HeaderSource::Config(later_source))],
            HeaderResolutionOptions { auth_header: true, api_key_config: Some("!outer-auth".into()) },
        )
        .unwrap();
        headers.set("X-Overlay", if stop_in_overlay { "!stop" } else { "!overlay-after-stop" });
        if stop_in_overlay && later_value {
            headers.set("X-Overlay-Second", "!overlay-second");
        }
        let gate = || can_continue.load(Ordering::SeqCst);
        assert!(matches!(headers.snapshot_checked(&resolver, &context, &gate), Err(ConfigHeaderCancelled)), "{name}");
        assert!(executor.settled.load(Ordering::SeqCst), "{name}");
        assert_eq!(executor.commands.lock().unwrap().as_slice(), ["stop"], "{name}: no later helper starts");
        // An already-cancelled traversal starts no helper, including a cached
        // command or an auth-only source.
        assert!(matches!(headers.snapshot_checked(&resolver, &context, &gate), Err(ConfigHeaderCancelled)));
        assert_eq!(executor.commands.lock().unwrap().len(), 1);
    }

    let (resolver, _, _) = ports(Ok("helper-value".into()));
    let base = create_live_config_headers(
        &[Some(HeaderSource::Config(HeaderConfigRecord::from_pairs(vec![
            ("X-First".into(), "!normal-helper".into()),
            ("X-Shared".into(), "base".into()),
        ])))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    let headers = create_live_config_headers(
        &[
            Some(HeaderSource::Live(base)),
            Some(HeaderSource::Config(HeaderConfigRecord::from_pairs(vec![
                ("X-Shared".into(), "source-overlay".into()),
                ("X-Last".into(), "last".into()),
            ]))),
        ],
        HeaderResolutionOptions { auth_header: true, api_key_config: Some("ordinary-key".into()) },
    )
    .unwrap();
    headers.set("X-Shared", "local-overlay");
    headers.set("Authorization", "local-auth");
    let checked = headers.snapshot_checked(&resolver, &context, &|| true).unwrap().unwrap();
    assert_eq!(checked.get("X-First"), Some("helper-value"));
    assert_eq!(checked.get("X-Shared"), Some("local-overlay"));
    assert_eq!(checked.get("Authorization"), Some("Bearer ordinary-key"));
    assert_eq!(checked.keys(), ["X-First", "X-Shared", "X-Last", "Authorization"]);
    assert_eq!(checked.into_pairs(), headers.snapshot(&resolver, &context).unwrap().into_pairs());
}

#[cfg(windows)]
fn failure_command() -> &'static str {
    "echo CONFIG_VALUES_SYNTHETIC_STDERR_SECRET 1>&2 & exit /b 7"
}
#[cfg(not(windows))]
fn failure_command() -> &'static str {
    "printf CONFIG_VALUES_SYNTHETIC_STDERR_SECRET >&2; exit 7"
}

#[test]
fn real_process_values_preserve_shell_cwd_faults_and_output_limits() {
    const CHILD: &str = "ARA_CONFIG_VALUES_STDERR_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let cwd = std::env::current_dir().unwrap();
        let resolver = ConfigValueResolver::isolated();
        let context = ConfigValueContext { project_dir: &cwd, environment: &no_env };
        assert!(
            resolver
                .resolve_config_value(
                    &format!("!{}", failure_command()),
                    &context,
                    ResolveConfigValueOptions::default()
                )
                .is_none()
        );
        assert_eq!(resolver.take_warnings().iter().map(|warning| warning.code).collect::<Vec<_>>(), ["unknown"]);
        return;
    }
    // Capture the actual test process stderr: inspecting only a returned error
    // would fail to detect Node-style direct stderr echoing.
    let captured = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "real_process_values_preserve_shell_cwd_faults_and_output_limits", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(captured.status.success());
    for stream in [&captured.stdout, &captured.stderr] {
        assert!(!String::from_utf8_lossy(stream).contains("CONFIG_VALUES_SYNTHETIC_STDERR_SECRET"));
    }
    let cwd = tempfile::tempdir().unwrap();
    let alternate = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("token.txt"), "\u{FEFF}\u{A0}  process-token\r\n").unwrap();
    std::fs::write(alternate.path().join("token.txt"), "other-cwd-token\n").unwrap();
    let context = ConfigValueContext { project_dir: cwd.path(), environment: &no_env };
    let other = ConfigValueContext { project_dir: alternate.path(), environment: &no_env };
    let unique = cwd.path().file_name().unwrap().to_string_lossy();
    #[cfg(windows)]
    let command = format!("!type token.txt & rem {unique}");
    #[cfg(not(windows))]
    let command = format!("!cat token.txt # {unique}");
    let resolver = ConfigValueResolver::new();
    let another = ConfigValueResolver::new();
    assert_eq!(
        resolver.resolve_config_value(&command, &context, ResolveConfigValueOptions::default()).as_deref(),
        Some("process-token")
    );
    assert_eq!(
        another.resolve_config_value(&command, &other, ResolveConfigValueOptions::default()).as_deref(),
        Some("process-token"),
        "production cache is process-wide and command-only"
    );
    assert_eq!(
        another
            .resolve_config_value(&command, &other, ResolveConfigValueOptions { force_command_refresh: true })
            .as_deref(),
        Some("other-cwd-token")
    );
    resolver.invalidate_command_config(Some(&command));
    let resolver = ConfigValueResolver::isolated();
    assert!(
        resolver
            .resolve_config_value(&format!("!{}", failure_command()), &context, ResolveConfigValueOptions::default())
            .is_none()
    );
    assert_eq!(resolver.take_warnings().iter().map(|warning| warning.code).collect::<Vec<_>>(), ["unknown"]);
    #[cfg(windows)]
    let empty = "!exit /b 0";
    #[cfg(not(windows))]
    let empty = "!:";
    assert!(resolver.resolve_config_value(empty, &context, ResolveConfigValueOptions::default()).is_none());
    assert!(resolver.take_warnings().is_empty());
    let missing = cwd.path().join("does-not-exist");
    let inaccessible = ConfigValueContext { project_dir: &missing, environment: &no_env };
    assert!(
        resolver
            .resolve_config_value("!echo must-not-run", &inaccessible, ResolveConfigValueOptions::default())
            .is_none()
    );
    assert!(resolver.take_warnings().is_empty());

    #[cfg(windows)]
    let overflowing = "!for /l %i in (1,1,60000) do @(echo 0123456789&echo 0123456789 1>&2)";
    #[cfg(not(windows))]
    let overflowing =
        "!i=0; while [ \"$i\" -lt 60000 ]; do printf '0123456789\\n'; printf '0123456789\\n' >&2; i=$((i+1)); done";
    assert!(resolver.resolve_config_value(overflowing, &context, ResolveConfigValueOptions::default()).is_none());
    assert_eq!(resolver.take_warnings().iter().map(|warning| warning.code).collect::<Vec<_>>(), ["ENOBUFS"]);
}

#[test]
fn real_process_timeout_signals_at_ten_seconds_without_automatic_replay() {
    const CHILD: &str = "ARA_CONFIG_VALUES_SLEEP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        // This is a genuine native process with ordinary termination behavior.
        // Windows may leave this descendant alive briefly after killing cmd;
        // the module never claims process-tree containment or undone effects.
        std::thread::sleep(Duration::from_secs(11));
        std::process::exit(0);
    }
    let cwd = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    #[cfg(windows)]
    let command = format!(
        "!set {CHILD}=1&\"{executable}\" --exact real_process_timeout_signals_at_ten_seconds_without_automatic_replay --nocapture"
    );
    #[cfg(not(windows))]
    let command = format!(
        "!{CHILD}=1 exec '{}' --exact real_process_timeout_signals_at_ten_seconds_without_automatic_replay --nocapture",
        executable.replace('\'', "'\\''")
    );
    let resolver = ConfigValueResolver::isolated();
    let context = ConfigValueContext { project_dir: cwd.path(), environment: &no_env };
    let started = Instant::now();
    assert!(resolver.resolve_config_value(&command, &context, ResolveConfigValueOptions::default()).is_none());
    // A bounded ordinary fixture proves timeout dispatch; an adversarial child
    // ignoring SIGTERM is explicitly outside a strict settlement guarantee.
    assert!(started.elapsed() >= COMMAND_TIMEOUT.saturating_sub(Duration::from_millis(100)));
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(resolver.take_warnings().iter().map(|warning| warning.code).collect::<Vec<_>>(), ["ETIMEDOUT"]);
    let cached = Instant::now();
    assert!(resolver.resolve_config_value(&command, &context, ResolveConfigValueOptions::default()).is_none());
    assert!(cached.elapsed() < Duration::from_secs(1));
    assert!(resolver.take_warnings().is_empty());
}
