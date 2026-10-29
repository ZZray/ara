//! Actual process/file observations in the existing CLI coarse entrance.
//! Host IO is real; generated artifacts are retained and independently read.
use ara_cli::{
    actual_tools::BuiltinToolSource,
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_logger::{logger_host::NativeLoggerHostOptions, time_host::initialize_process_performance},
    native_realm::{NativeRealm, date::SystemDateHost, logger::NativeLoggerBindings},
    native_run::NativeRun,
};
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueHandle, ValueSnapshot};
use ara_tools::ToolContext;
use std::{path::PathBuf, sync::Arc};

const CHILD: &str = "ARA_NATIVE_LOGGER_BODY_CHILD";
const DIRECTORY: &str = "ARA_NATIVE_LOGGER_BODY_DIRECTORY";
const ENTRANCE: &str = "tests::journal_failure_cancels_the_run";
#[path = "logger.rs"]
mod sync_body;
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "selected real Host consumer of the installed original time export",
    name: "retainedValue",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, captures, _, _, runtime| {
        heap.call(&captures[1], &heap.undefined()?, &[heap.string("shared-root-point".into())?], runtime)?;
        Ok(captures[0].clone())
    },
};

fn field(realm: &NativeRealm, object: &ValueHandle, name: &str, value: ValueHandle) {
    realm.values.heap().define_own_data(object, name.into(), value, realm.values.runtime()).unwrap();
}
fn call(realm: &NativeRealm, function: &ValueHandle, args: &[ValueHandle]) -> ValueHandle {
    realm.values.heap().call(function, &realm.values.heap().undefined().unwrap(), args, realm.values.runtime()).unwrap()
}
fn export(realm: &NativeRealm, bindings: &NativeLoggerBindings, name: &str) -> ValueHandle {
    realm.values.heap().get(&bindings.sync_module, &name.into(), realm.values.runtime()).unwrap()
}

#[cfg(windows)]
fn close_stdout() {
    use windows_sys::Win32::System::Console::{STD_OUTPUT_HANDLE, SetStdHandle};
    assert_ne!(unsafe { SetStdHandle(STD_OUTPUT_HANDLE, std::ptr::null_mut()) }, 0);
}
#[cfg(unix)]
fn close_stdout() {
    assert_eq!(unsafe { libc::close(1) }, 0);
}

pub(crate) async fn child_if_selected() -> bool {
    let Some(mode) = std::env::var_os(CHILD) else { return false };
    let mode = mode.to_str().unwrap();
    assert!(["file", "console", "both", "closed", "failed_file"].contains(&mode));
    let directory = PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
    let logs = directory.join("logs");
    std::fs::create_dir_all(&directory).unwrap();
    initialize_process_performance();
    let realm = NativeRealm::prepare().unwrap();
    let source = BuiltinToolSource::new(&realm.values, ToolContext::new(directory.clone()), &[]).unwrap();
    let prepared = PreparedNativeSession::from_complete_bindings_with_logger(
        realm.clone(),
        NativeFactoryImports {
            source,
            string_function: realm.globals.string.clone(),
            bun: realm.globals.bun.clone(),
            original_logger: None,
        },
        NativeLoggerHostOptions {
            default_logs_dir: logs.clone(),
            filename_prefix: "omp".into(),
            date: Arc::new(SystemDateHost),
        },
    )
    .unwrap();
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let module = prepared.logger_bindings().unwrap();
    let timing = prepared.timing().unwrap();
    assert_eq!(heap.own_keys(&module.sync_module, runtime).unwrap().len(), 15);
    assert!(!logs.exists(), "Logger preparation must preserve lazy file initialization");
    let options = heap.object().unwrap();
    field(&realm, &options, "file", heap.boolean(matches!(mode, "file" | "both" | "failed_file")).unwrap());
    field(&realm, &options, "console", heap.boolean(matches!(mode, "console" | "both" | "closed")).unwrap());
    call(&realm, &export(&realm, &module, "setTransports"), &[options]);
    if mode == "failed_file" {
        std::fs::write(&logs, b"a file is not a directory").unwrap();
    }
    if mode == "closed" {
        close_stdout();
    }
    let context = heap.object().unwrap();
    field(&realm, &context, "requestId", heap.string("same-native-context".into()).unwrap());
    let reason = call(&realm, &realm.globals.errors.error, &[heap.string("original cause".into()).unwrap()]);
    field(&realm, &reason, "code", heap.string("HOST_ORIGINAL".into()).unwrap());
    field(&realm, &reason, "cause", heap.string("retained-cause".into()).unwrap());
    field(&realm, &context, "err", reason);
    call(&realm, &export(&realm, &module, "info"), &[heap.string(format!("host-{mode}").into()).unwrap(), context]);

    if mode == "file" {
        // Switching active transports closes the prior file, then initializes
        // the explicit new path. Subsequent calls must use the new sink.
        let alternate = directory.join("alternate");
        let options = heap.object().unwrap();
        field(&realm, &options, "file", heap.string(alternate.to_str().unwrap().into()).unwrap());
        call(&realm, &export(&realm, &module, "setTransports"), &[options]);
        call(&realm, &export(&realm, &module, "warn"), &[heap.string("after-switch".into()).unwrap()]);
        let run = NativeRun::begin(prepared.clone(), tokio_util::sync::CancellationToken::new()).unwrap();
        call(&realm, &export(&realm, &module, "startTiming"), &[]);
        let value = heap.number(17.0).unwrap();
        let action = heap.fresh_compiled_function(&RETURN, &[value.clone(), export(&realm, &module, "time")]).unwrap();
        let scoped = run.root_runtime().unwrap();
        let result = heap
            .call(
                &export(&realm, &module, "time"),
                &heap.undefined().unwrap(),
                &[heap.string("shared-root-host".into()).unwrap(), action],
                &scoped,
            )
            .unwrap();
        assert!(result.same_identity(&value));
        let root = timing.root_span(&scoped).unwrap();
        let children = heap.get(&root, &"children".into(), &scoped).unwrap();
        assert_eq!(heap.array_length(&children, &scoped).unwrap(), 1);
        drop(scoped);
        run.drive(async { Ok::<(), ara_agent::ActualProviderError>(()) }).await.unwrap().unwrap();
        call(&realm, &export(&realm, &module, "printTimings"), &[]);
        call(&realm, &export(&realm, &module, "endTiming"), &[]);
        assert!(matches!(heap.snapshot(&timing.root_span(runtime).unwrap()).unwrap(), ValueSnapshot::Undefined));
    }
    let receipt = serde_json::json!({"mode":mode,"original_error":true,"pid":std::process::id(),"completed_call":true});
    std::fs::write(directory.join("receipt.json"), serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    true
}

fn read_logs(directory: &std::path::Path) -> Vec<serde_json::Value> {
    if !directory.is_dir() {
        return Vec::new();
    }
    let mut values = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|suffix| suffix == "log") {
            let text = std::fs::read_to_string(path).unwrap();
            for line in text.lines().filter(|line| !line.is_empty()) {
                values.push(serde_json::from_str(line).unwrap());
            }
        }
    }
    values
}

pub(crate) async fn observe() {
    let realm = NativeRealm::prepare().unwrap();
    let heap = realm.values.heap();
    let storage = realm.context().prepare_storage(heap).unwrap();
    let run = realm.begin_promise_run().unwrap();
    let scoped = run.scoped(run.root_scope(), run.root_parent()).unwrap();
    let observation = sync_body::observe_logger_sync(heap, &realm.globals, &storage, &scoped).unwrap();
    assert_eq!(observation.exports_exercised, 14);
    assert_eq!(observation.console_only_json_calls, 2);
    assert!(!observation.timing_output.is_empty());
    assert!(heap.get(&realm.globals.environment, &"Map".into(), &scoped).unwrap().same_identity(&realm.globals.map));
    drop(scoped);
    drop(run);
    drop(storage);
    drop(realm);
    let temp = tempfile::Builder::new().prefix("ara-native-logger-body-").tempdir().unwrap();
    for mode in ["file", "console", "both", "closed", "failed_file"] {
        let directory = temp.path().join(mode);
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child.args([ENTRANCE, "--exact", "--nocapture"]).env(CHILD, mode).env(DIRECTORY, &directory);
        child.env_remove("ARA_NATIVE_LOGGER_TIMING_CHILD");
        child.env_remove("PI_DEBUG_STARTUP").env_remove("PI_TIMING");
        child.kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(25), child.output()).await.unwrap().unwrap();
        std::fs::write(temp.path().join(format!("{mode}-stdout.bin")), &output.stdout).unwrap();
        std::fs::write(temp.path().join(format!("{mode}-stderr.bin")), &output.stderr).unwrap();
        assert!(output.status.success(), "Native logger child {mode}: {}", String::from_utf8_lossy(&output.stderr));
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["mode"], mode);
        let logs = read_logs(&directory.join("logs"));
        assert_eq!(logs.len(), usize::from(matches!(mode, "file" | "both")));
        if let Some(entry) = logs.first() {
            assert_eq!(entry["message"], format!("host-{mode}"));
            assert_eq!(entry["level"], "info");
            assert_eq!(entry["pid"], receipt["pid"]);
            assert_eq!(entry["requestId"], "same-native-context");
            assert_eq!(entry["err"]["name"], "Error");
            assert_eq!(entry["err"]["message"], "original cause");
            assert_eq!(entry["err"]["code"], "HOST_ORIGINAL");
            assert_eq!(entry["err"]["cause"], "retained-cause");
            assert!(entry["timestamp"].as_str().is_some_and(|stamp| stamp.contains('T') && stamp.len() >= 29));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected_console = matches!(mode, "console" | "both");
        assert_eq!(stdout.matches(&format!("\"message\":\"host-{mode}\"")).count(), usize::from(expected_console));
        if mode == "file" {
            let alternate = read_logs(&directory.join("alternate"));
            assert_eq!(alternate.len(), 1);
            assert_eq!(alternate[0]["message"], "after-switch");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("shared-root-host") && stderr.contains("Total:"));
        }
    }
    let artifacts = temp.keep();
    eprintln!("Logger Body Host retained artifacts: {}", artifacts.display());
    eprintln!(
        "Logger Body real process: lazy file/console/both/switch; original Error fields; closed stdout/failed directory best-effort"
    );
}
