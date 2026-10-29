//! Production Date.parse calls on the original NativeRun and its scoped parent.
//! This complements parser probes; it does not claim complete Date parity.
use ara_cli::{
    actual_tools::BuiltinToolSource,
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueSnapshot};
use tokio_util::sync::CancellationToken;

static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Date ToString returns the original captured string",
    name: "toString",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Builtin,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Date ToString throws the original captured reason",
    name: "toString",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Builtin,
    body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
};

#[tokio::test]
async fn production_date_parse_original_run_and_coercion() {
    let realm = NativeRealm::prepare().expect("Production SystemDateHost");
    let root = std::env::current_dir().expect("Existing tool root");
    let source = BuiltinToolSource::new(&realm.values, ara_tools::ToolContext::new(root), &[])
        .expect("Original builtin bindings");
    let prepared = PreparedNativeSession::from_complete_bindings(
        realm.clone(),
        NativeFactoryImports {
            source,
            string_function: realm.globals.string.clone(),
            bun: realm.globals.bun.clone(),
            original_logger: None,
        },
    )
    .expect("Original Session");
    let run = NativeRun::begin(prepared, CancellationToken::new()).expect("Original Run");
    let factory = realm.values.factory().clone();
    let calling = run.calling_run(&factory).expect("Original root parent");
    let _native_retry = ara_cli::retry_host::NativeRetryBindings::prepare(
        &factory,
        &realm,
        calling.clone(),
        ara_ai::ActualStreamFailureReceipt::default(),
    )
    .expect("Same original Run retry imports");
    let observation = async {
        calling.with_runtime(&factory, &mut |runtime| {
            let heap = factory.heap();
            let date = &realm.globals.date;
            let parse = heap.get(date, &"parse".into(), runtime)?;
            // The static parser accepts an unrelated receiver, without a Date slot.
            let receiver = heap.object()?;
            for (input, expected) in [
                ("Tue, 14 Nov 2023 22:13:21 GMT", 1_700_000_001_000.0),
                ("2023-11-14T22:13:21Z", 1_700_000_001_000.0),
                ("2023-11-15", 1_700_006_400_000.0),
                ("1970-01-01T00:00:00Z", 0.0),
                ("1970-01-01T08:00:00+0800", 0.0),
                ("+275760-09-13T00:00:00Z", 8_640_000_000_000_000.0),
                ("-271821-04-20T00:00:00Z", -8_640_000_000_000_000.0),
            ] {
                let value = heap.call(&parse, &receiver, &[heap.string(input.into())?], runtime)?;
                let ValueSnapshot::Number(actual) = heap.snapshot(&value)? else {
                    panic!("Date.parse did not return its actual Number: {input}");
                };
                assert_eq!(actual, expected, "{input}");
            }
            for input in ["not a date", "+275760-09-13T00:00:00.001Z", "2026-10-04T24:00:00.001Z"] {
                let value = heap.call(&parse, &receiver, &[heap.string(input.into())?], runtime)?;
                assert!(matches!(heap.snapshot(&value)?, ValueSnapshot::Number(n) if n.is_nan()), "{input}");
            }
            let surrogate = heap.string(ara_rpc::WireString::from_units(vec![0xd800]))?;
            let value = heap.call(&parse, &receiver, &[surrogate], runtime)?;
            assert!(matches!(heap.snapshot(&value)?, ValueSnapshot::Number(n) if n.is_nan()));
            let coercible = heap.object()?;
            let source = heap.string("Tue, 14 Nov 2023 22:13:21 GMT".into())?;
            heap.define_own_data(&coercible, "toString".into(), heap.fresh_compiled_function(&RETURN, &[source])?, runtime)?;
            let value = heap.call(&parse, &receiver, &[coercible], runtime)?;
            assert!(matches!(heap.snapshot(&value)?, ValueSnapshot::Number(n) if n == 1_700_000_001_000.0));
            let throwing = heap.object()?;
            let original_reason = heap.object()?;
            heap.define_own_data(&throwing, "toString".into(), heap.fresh_compiled_function(&THROW, &[original_reason.clone()])?, runtime)?;
            let failure = heap.call(&parse, &receiver, &[throwing], runtime);
            assert!(matches!(failure, Err(ValueError::Thrown(ref reason)) if reason.same_identity(&original_reason)));
            #[cfg(windows)]
            timezone_mutation_matrix(&realm, &factory, runtime)?;
            eprintln!("ARA-RETRY-DATE PASS original-run parse and coercion");
            heap.undefined()
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), run.drive(observation))
        .await
        .expect("Run bound")
        .expect("Original Run settlement")
        .expect("Date observations");
}

#[cfg(windows)]
fn timezone_mutation_matrix(
    realm: &std::sync::Arc<NativeRealm>,
    factory: &std::sync::Arc<ara_ai::graph_values::GraphValueFactory>,
    runtime: &dyn ara_rpc::value_heap::ValueRuntime,
) -> Result<(), ValueError> {
    use ara_cli::native_realm::date::{DateHost, SystemDateHost};
    use ara_rpc::value_heap::{PropertyDescriptor, ValueHandle};
    let snapshot = SystemDateHost::snapshot_timezone_env()?;
    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let heap = factory.heap();
        let process = heap.get(&realm.globals.environment, &"process".into(), runtime)?;
        let env = heap.get(&process, &"env".into(), runtime)?;
        let parse = heap.get(&realm.globals.date, &"parse".into(), runtime)?;
        let undefined = heap.undefined()?;
        let parse_local = |source: &str| -> Result<f64, ValueError> {
            let result = heap.call(&parse, &undefined, &[heap.string(source.into())?], runtime)?;
            heap.to_number(&result, runtime)
        };
        let parse_other = |other: &NativeRealm| -> Result<f64, ValueError> {
            let factory = other.values.factory();
            let heap = factory.heap();
            let runtime = factory.runtime();
            let function = heap.get(&other.globals.date, &"parse".into(), runtime)?;
            let result = heap.call(
                &function,
                &heap.undefined()?,
                &[heap.string("2026-10-04T01:02:03".into())?],
                runtime,
            )?;
            heap.to_number(&result, runtime)
        };
        let set = |text: &str| -> Result<(), ValueError> {
            heap.assign(&env, "TZ".into(), heap.string(text.into())?, runtime)
        };
        let retained = NativeRealm::prepare()?;
        assert!(heap.delete_own(&env, &"TZ".into(), runtime)?);
        let default = parse_local("2026-10-04T01:02:03")?;
        set("ARA_INVALID_TIMEZONE")?;
        assert_eq!(parse_local("2026-10-04T01:02:03")?, default);
        // No Date operation between the valid and rejected mutations.
        set("America/New_York")?;
        set("ARA_INVALID_TIMEZONE")?;
        assert_eq!(parse_local("2026-10-04T01:02:03")?, 1_791_090_123_000.0);
        assert_eq!(
            SystemDateHost.parse_ms(&"2026-10-04T01:02:03".into())?,
            1_791_090_123_000.0
        );
        assert_eq!(parse_other(&retained)?, 1_791_090_123_000.0);
        let fresh = NativeRealm::prepare()?;
        assert_eq!(parse_other(&fresh)?, 1_791_090_123_000.0);
        assert_eq!(parse_local("2026-10-04T01:02:03Z")?, 1_791_075_723_000.0);
        assert!(
            matches!(heap.snapshot(&heap.get(&env, &"TZ".into(), runtime)?)?,
            ValueSnapshot::String(raw) if raw.equals_ascii("ARA_INVALID_TIMEZONE"))
        );
        assert_eq!(
            std::env::var("TZ").ok().as_deref(),
            Some("ARA_INVALID_TIMEZONE")
        );
        set("")?;
        assert_eq!(parse_local("2026-10-04T01:02:03")?, default);
        assert_eq!(parse_other(&retained)?, default);
        set("America/New_York")?;
        assert!(heap.delete_own(&env, &"TZ".into(), runtime)?);
        assert_eq!(parse_local("2026-10-04T01:02:03")?, default);
        assert!(matches!(
            heap.snapshot(&heap.get(&env, &"TZ".into(), runtime)?)?,
            ValueSnapshot::Undefined
        ));
        assert!(std::env::var_os("TZ").is_none());
        // Define uses the same production mutation path, including rejection.
        heap.define_own(
            &env,
            "TZ".into(),
            PropertyDescriptor::Data {
                value: heap.string("America/New_York".into())?,
                writable: true,
                enumerable: true,
                configurable: true,
            },
            runtime,
        )?;
        heap.define_own(
            &env,
            "TZ".into(),
            PropertyDescriptor::Data {
                value: heap.string("ARA_INVALID_TIMEZONE".into())?,
                writable: true,
                enumerable: true,
                configurable: true,
            },
            runtime,
        )?;
        assert_eq!(parse_local("2026-10-04T01:02:03")?, 1_791_090_123_000.0);
        let denied = heap.define_own(
            &env,
            "TZ".into(),
            PropertyDescriptor::Data {
                value: heap.string("UTC".into())?,
                writable: false,
                enumerable: true,
                configurable: true,
            },
            runtime,
        );
        assert!(matches!(denied, Err(ValueError::TypeError(_))));
        let throwing = heap.object()?;
        let reason = heap.object()?;
        heap.define_own_data(
            &throwing,
            "toString".into(),
            heap.fresh_compiled_function(&THROW, &[reason.clone()])?,
            runtime,
        )?;
        let thrown = heap.assign(&env, "TZ".into(), throwing, runtime);
        assert!(
            matches!(thrown, Err(ValueError::Thrown(ref actual)) if actual.same_identity(&reason))
        );
        assert_eq!(parse_local("2026-10-04T01:02:03")?, 1_791_090_123_000.0);
        // Future offsetless Retry-After must use this actual installed zone.
        let future = "2030-10-04T01:02:03";
        let utc = parse_local("2030-10-04T05:02:03Z")?;
        assert_eq!(parse_local(future)?, utc);
        let headers = heap.object()?;
        heap.define_own_data(
            &headers,
            "retry-after".into(),
            heap.string(future.into())?,
            runtime,
        )?;
        let hints = ara_ai::retry::hints::RetryAfterModule::prepare(
            factory,
            realm.globals.environment.clone(),
        )?;
        let before = SystemDateHost.now_ms()?;
        let wait: ValueHandle = hints.invoke(
            factory,
            &hints.get_retry_after_ms_from_headers,
            &[headers],
            runtime,
        )?;
        let after = SystemDateHost.now_ms()?;
        let delay = heap.to_number(&wait, runtime)?;
        assert!((utc - after..=utc - before).contains(&delay));
        eprintln!("ARA-RETRY-DATE PASS TZ mutations, shared realms and offsetless Retry-After");
        Ok::<(), ValueError>(())
    }));
    // Restore both raw env and installed override even when an assertion panics.
    let restored = SystemDateHost::restore_timezone_env(snapshot);
    match observed {
        Ok(result) => {
            restored?;
            result
        }
        Err(payload) => {
            if let Err(error) = restored {
                eprintln!("TZ restore failed: {error}");
            }
            std::panic::resume_unwind(payload)
        }
    }
}
