//! Real selected logger timing Host observations, through the existing CLI
//! coarse entrance. Child processes own their environment and stderr state.
//! The supplied logger shell is explicitly a selected-export fixture; complete
//! Logger and production Session activation remain separate acceptance gates.

use ara_cli::{
    actual_tools::BuiltinToolSource,
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_logger::time_host::initialize_process_performance,
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_rpc::{
    WireString,
    value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueSnapshot},
};
use ara_tools::ToolContext;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const CHILD: &str = "ARA_NATIVE_LOGGER_TIMING_CHILD";
const RECEIPT: &str = "ARA_NATIVE_LOGGER_TIMING_RECEIPT ";
const ENTRANCE: &str = "tests::journal_failure_cancels_the_run";

fn member(realm: &NativeRealm, object: &ValueHandle, key: &str) -> ValueHandle {
    realm.values.heap().get(object, &key.into(), realm.values.runtime()).unwrap()
}
fn number(realm: &NativeRealm, value: &ValueHandle) -> f64 {
    let ValueSnapshot::Number(value) = realm.values.heap().snapshot(value).unwrap() else { panic!("Number expected") };
    value
}
fn now(realm: &NativeRealm, performance: &ValueHandle) -> f64 {
    let function = member(realm, performance, "now");
    let value = realm.values.heap().call(&function, performance, &[], realm.values.runtime()).unwrap();
    number(realm, &value)
}
fn receipt(value: serde_json::Value) {
    println!("{RECEIPT}{value}");
}

#[cfg(windows)]
fn close_child_stderr() {
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};
    // Only the expressly selected child process enters this branch. Keep its
    // stdout for receipts, and make future stderr calls fail at the OS boundary.
    assert_ne!(unsafe { SetStdHandle(STD_ERROR_HANDLE, std::ptr::null_mut()) }, 0);
}
#[cfg(unix)]
fn close_child_stderr() {
    assert_eq!(unsafe { libc::close(libc::STDERR_FILENO) }, 0);
}

static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled retained-Prepared consumer of the original selected time body",
    name: "retainedValue",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};

pub(crate) async fn child_if_selected() -> bool {
    let Some(mode) = std::env::var_os(CHILD) else { return false };
    let mode = mode.to_str().unwrap();
    assert!(["enabled", "disabled", "uninitialized", "closed"].contains(&mode));
    let realm = NativeRealm::prepare().unwrap();
    let imports = realm.logger_time_imports().unwrap();
    if mode == "uninitialized" {
        let function = member(&realm, &imports.performance, "now");
        assert!(matches!(
            realm.values.heap().call(&function, &imports.performance, &[], realm.values.runtime()),
            Err(ValueError::UnsupportedOperation(_))
        ));
        receipt(serde_json::json!({"mode": mode, "uninitialized_is_explicit": true}));
        return true;
    }
    initialize_process_performance();
    let first = now(&realm, &imports.performance);
    tokio::time::sleep(Duration::from_millis(8)).await;
    initialize_process_performance();
    let second_realm = NativeRealm::prepare().unwrap();
    let second_imports = second_realm.logger_time_imports().unwrap();
    let second = now(&second_realm, &second_imports.performance);
    assert!(first.is_finite() && first >= 0.0);
    assert!(second >= first + 5.0, "process performance origin must survive repeated init and new realms");
    drop(second_imports);
    drop(second_realm);
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let undefined = heap.undefined().unwrap();
    let write = member(&realm, &imports.fs, "writeSync");
    if mode == "closed" {
        close_child_stderr();
        let reason = match heap.call(
            &write,
            &imports.fs,
            &[heap.number(2.0).unwrap(), heap.string("closed-stderr-probe\n".into()).unwrap()],
            runtime,
        ) {
            Err(ValueError::Thrown(reason)) => reason,
            other => panic!("closed stderr must yield original realm Error, got {other:?}"),
        };
        assert!(heap.prototype(&reason, runtime).unwrap().unwrap().same_identity(&member(
            &realm,
            &realm.globals.errors.error,
            "prototype"
        )));
        let module = ara_cli::native_logger::time::NativeLoggerTimeModule::prepare(&realm, imports).unwrap();
        let output = heap
            .call(&module.startup_marker, &undefined, &[heap.string("best-effort".into()).unwrap()], runtime)
            .unwrap();
        assert!(matches!(heap.snapshot(&output).unwrap(), ValueSnapshot::Undefined));
        receipt(serde_json::json!({"mode": mode, "closed_stderr_original_error": true, "marker_best_effort": true}));
        return true;
    }
    let mut write_bytes = None;
    if mode == "enabled" {
        let text = heap.string(WireString::from_units(vec![117, 116, 102, 56, 45, 0xd800, 10])).unwrap();
        let result = heap.call(&write, &imports.fs, &[heap.number(2.0).unwrap(), text], runtime).unwrap();
        assert_eq!(number(&realm, &result), 9.0);
        write_bytes = Some(9);
    }
    let source = BuiltinToolSource::new(&realm.values, ToolContext::new(std::env::temp_dir()), &[]).unwrap();
    let logger = heap.object().unwrap();
    let prepared = PreparedNativeSession::from_complete_bindings_with_timing(
        realm.clone(),
        NativeFactoryImports {
            source,
            string_function: realm.globals.string.clone(),
            bun: realm.globals.bun.clone(),
            original_logger: Some(logger.clone()),
        },
    )
    .unwrap();
    let timing = prepared.timing().unwrap();
    for (name, function) in
        [("time", &timing.time), ("startTiming", &timing.start_timing), ("startupMarker", &timing.startup_marker)]
    {
        assert!(member(&realm, &logger, name).same_identity(function));
    }
    heap.call(
        &timing.startup_marker,
        &undefined,
        &[heap.string(WireString::from_units(vec![104, 111, 115, 116, 45, 0xd800])).unwrap()],
        runtime,
    )
    .unwrap();
    heap.call(&timing.start_timing, &undefined, &[], runtime).unwrap();
    let root = timing.root_span(runtime).unwrap();
    let time = timing.time.clone();
    let marker = timing.startup_marker.clone();
    let performance = imports.performance.clone();
    let values = prepared.values().clone();
    let original = heap.object().unwrap();
    let wrapped = heap.fresh_compiled_function(&RETURN, &[original.clone()]).unwrap();
    let weak_prepared = Arc::downgrade(&prepared);
    let weak_realm = Arc::downgrade(&realm);
    let run = NativeRun::begin(prepared.clone(), CancellationToken::new()).unwrap();
    let scoped = run.root_runtime().unwrap();
    let lease = run.lease();
    drop(prepared);
    drop(realm);
    assert!(weak_prepared.upgrade().is_some() && weak_realm.upgrade().is_some());
    let output = run
        .drive(async {
            tokio::time::sleep(Duration::from_millis(8)).await;
            assert!(weak_prepared.upgrade().is_some() && weak_realm.upgrade().is_some());
            let heap = values.heap();
            let output = heap
                .call(
                    &time,
                    &heap.undefined().unwrap(),
                    &[heap.string("host-retained".into()).unwrap(), wrapped],
                    &scoped,
                )
                .unwrap();
            heap.call(&time, &heap.undefined().unwrap(), &[heap.string("host-point".into()).unwrap()], &scoped)
                .unwrap();
            output
        })
        .await
        .unwrap();
    assert!(output.same_identity(&original));
    assert!(weak_prepared.upgrade().is_some() && weak_realm.upgrade().is_some());
    let retained = weak_realm.upgrade().unwrap();
    let children = member(&retained, &root, "children");
    assert_eq!(number(&retained, &member(&retained, &children, "length")), 2.0);
    let span = member(&retained, &children, "0");
    let root_start = number(&retained, &member(&retained, &root, "start"));
    let span_start = number(&retained, &member(&retained, &span, "start"));
    let span_end = number(&retained, &member(&retained, &span, "end"));
    assert!(span_start >= root_start + 5.0 && span_end >= span_start);
    assert!(member(&retained, &span, "parent").same_identity(&root));
    let point = member(&retained, &children, "1");
    assert!(member(&retained, &point, "start").same_identity(&member(&retained, &point, "end")));
    drop(retained);
    lease.settle().await.unwrap();
    drop(lease);
    assert!(weak_prepared.upgrade().is_none() && weak_realm.upgrade().is_none());
    let now_function = values.heap().get(&performance, &"now".into(), values.runtime()).unwrap();
    assert!(matches!(
        values.heap().call(&now_function, &performance, &[], values.runtime()),
        Err(ValueError::ExpiredCapability(_))
    ));
    assert!(matches!(
        values.heap().call(
            &marker,
            &values.heap().undefined().unwrap(),
            &[values.heap().string("after-drop".into()).unwrap()],
            values.runtime()
        ),
        Err(ValueError::ExpiredCapability(_))
    ));
    receipt(serde_json::json!({"mode": mode, "first_ms": first, "second_realm_ms": second,
        "root_start_ms": root_start, "span_start_ms": span_start, "span_end_ms": span_end,
        "write_bytes": write_bytes, "prepared_retained_during_run": true, "prepared_released_after_settlement": true,
        "expired_host_explicit": true}));
    true
}

pub(crate) async fn observe() {
    let directory = std::env::temp_dir().join(format!("ara-native-logger-timing-host-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = std::env::current_exe().unwrap();
    for mode in ["enabled", "disabled", "uninitialized", "closed"] {
        let mut command = tokio::process::Command::new(&executable);
        command
            .args([ENTRANCE, "--exact", "--nocapture"])
            .env(CHILD, mode)
            .env_remove("PI_DEBUG_STARTUP")
            .kill_on_drop(true);
        if mode == "enabled" || mode == "closed" {
            command.env("PI_DEBUG_STARTUP", "1");
        }
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .expect("bounded timing child")
            .unwrap();
        std::fs::write(directory.join(format!("{mode}.stdout")), &output.stdout).unwrap();
        std::fs::write(directory.join(format!("{mode}.stderr")), &output.stderr).unwrap();
        assert!(output.status.success(), "timing child {mode}: {}", String::from_utf8_lossy(&output.stdout));
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        let line = stdout.lines().find_map(|line| line.strip_prefix(RECEIPT)).expect("actual child receipt");
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(record["mode"], mode);
        std::fs::write(directory.join(format!("{mode}.json")), serde_json::to_vec_pretty(&record).unwrap()).unwrap();
        let expected = if mode == "enabled" {
            "utf8-\u{fffd}\n[startup] host-\u{fffd}\n[startup] host-retained:start\n[startup] host-retained:done\n[startup] host-point\n"
        } else {
            ""
        };
        assert_eq!(
            std::str::from_utf8(&output.stderr).unwrap().replace("\r\n", "\n"),
            expected,
            "real child stderr {mode}"
        );
    }
    println!("Native logger timing Host artifacts: {}", directory.display());
}
