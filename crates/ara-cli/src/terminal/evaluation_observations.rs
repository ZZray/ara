#[cfg(test)]
pub(crate) fn observe_display_queue_and_session_callbacks() {
    use crate::native_realm::promise::NativePromiseState;
    use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueSnapshot};
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let port = DisplayPort::new();
    port.run_generation(1);
    let heap = ara_rpc::value_heap::ValueHeap::new().unwrap();
    let mut latest = None;
    runtime.block_on(async {
        for i in 0..20 {
            let message = serde_json::json!({
                "role":"assistant", "content":[], "api":"openai-completions",
                "provider":"fixture", "model":"fixture", "usage":{},
                "stopReason":"stop", "timestamp":0, "fresh":i
            });
            let root = heap.import_serde(&message).unwrap();
            latest = Some(root.clone());
            let event = AgentEvent::ActualMessageUpdate {
                message: ara_agent::AgentMessage::from_raw(message, None).unwrap(),
                event: ara_ai::AssistantMessageEvent::Start { partial: root },
            };
            assert!(matches!(port.admit(1, event).await, Admission::Accepted(_)));
        }
        assert!(matches!(port.admit(1, AgentEvent::AgentEnd { messages: vec![] }).await, Admission::Accepted(_)));
    });
    let cutoff = port.cutoff();
    let update = port.take(cutoff, true).unwrap();
    match &update.work {
        Work::Event { event: AgentEvent::ActualMessageUpdate { event, .. }, .. } => {
            assert!(event.partial().same_identity(latest.as_ref().unwrap()))
        }
        _ => panic!("latest cumulative update must flush first"),
    }
    drop(update);
    assert!(matches!(port.take(cutoff, true).unwrap().work, Work::Event { event: AgentEvent::AgentEnd { .. }, .. }));
    let slot = port.reserve().unwrap();
    port.control_reserved(slot, Control::Replay);
    assert!(port.take(cutoff, true).is_none(), "post-cutoff demand belongs to the prompt pump");
    drop(port.take(port.cutoff(), true));
    let held = (0..32).map(|_| port.reserve().unwrap()).collect::<Vec<_>>();
    assert!(matches!(port.reserve(), Err(Admission::Full)));
    drop(held);
    port.close();
    assert!(matches!(port.reserve(), Err(Admission::Closed)));

    static CURRENT: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "display Session post-Run original storage callback observation",
        name: "current",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 1,
        body: |heap, captures, _, _, effective| {
            let method = heap.get(&captures[0], &"getStore".into(), effective)?;
            heap.call(&method, &captures[0], &[], effective)
        },
    };
    static PROMISE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "display Session post-Run Promise context observation",
        name: "promiseCurrent",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 3,
        body: |heap, captures, _, _, effective| {
            let current = heap.call(&captures[2], &heap.undefined()?, &[], effective)?;
            let resolve = heap.get(&captures[0], &"resolve".into(), effective)?;
            let promise = heap.call(&resolve, &captures[0], &[current], effective)?;
            let then = heap.get(&promise, &"then".into(), effective)?;
            heap.call(&then, &promise, &[captures[2].clone()], effective)
        },
    };
    let prepared = PreparedNativeSession::prepare().unwrap();
    let realm = prepared.realm();
    let heap = prepared.values().heap();
    let run = realm.begin_promise_run().unwrap();
    let expired = run.scoped(run.root_scope(), run.root_parent()).unwrap();
    drop(run);
    let mut callbacks = realm.begin_display_callbacks().unwrap();
    let effective = callbacks.runtime().unwrap();
    let concurrent = realm.begin_promise_run().unwrap();
    assert!(
        matches!(concurrent.admission().capture_scope(&effective), Err(ValueError::ExpiredCapability(_))),
        "Run admission must not capture the coexisting Session callback scope"
    );
    drop(concurrent);
    let storage = realm.context().prepare_storage(heap).unwrap();
    let store = heap.object().unwrap();
    let current = heap.fresh_compiled_function(&CURRENT, &[storage.clone()]).unwrap();
    let method = heap.get(&storage, &"run".into(), &effective).unwrap();
    let undefined = heap.undefined().unwrap();
    assert!(matches!(
        heap.call(&current, &undefined, &[], &effective).and_then(|v| heap.snapshot(&v)),
        Ok(ValueSnapshot::Undefined)
    ));
    let observed = heap.call(&method, &storage, &[store.clone(), current.clone()], &effective).unwrap();
    assert!(observed.same_identity(&store));
    assert!(
        matches!(
            heap.call(&current, &undefined, &[], &effective).and_then(|v| heap.snapshot(&v)),
            Ok(ValueSnapshot::Undefined)
        ),
        "inner storage context restores its display caller"
    );
    assert!(
        matches!(heap.call(&current, &undefined, &[], &expired), Err(ValueError::ExpiredCapability(_))),
        "old Run-origin context remains revoked"
    );
    let constructor = heap.get(&realm.globals.environment, &"Promise".into(), &effective).unwrap();
    let callback = heap.fresh_compiled_function(&PROMISE, &[constructor.clone(), storage.clone(), current]).unwrap();
    let promise = heap.call(&method, &storage, &[store.clone(), callback], &effective).unwrap();
    assert_eq!(realm.promise().snapshot(heap, &promise).unwrap(), NativePromiseState::Pending);
    assert!(matches!(callbacks.drain_one().unwrap(), NativeDrain::RanJob { .. }));
    match realm.promise().snapshot(heap, &promise).unwrap() {
        NativePromiseState::Fulfilled(edge) => assert!(heap.restore_deferred(&edge).unwrap().same_identity(&store)),
        _ => panic!("post-Run reaction preserves original inner context"),
    }

    static RECORD: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "coexisting original Promise callback records FIFO receipt",
        name: "",
        length: 1,
        form: CompiledFunctionForm::Arrow,
        capture_count: 2,
        body: |heap, captures, _, _, effective| {
            let push = heap.get(&captures[0], &"push".into(), effective)?;
            heap.call(&push, &captures[0], &[captures[1].clone()], effective)?;
            heap.undefined()
        },
    };
    let order = heap.array_with_length(0).unwrap();
    let run = realm.begin_promise_run().unwrap();
    let run_runtime = run.scoped(run.root_scope(), run.root_parent()).unwrap();
    let enqueue = |label: &str, runtime: &dyn ValueRuntime| {
        let callback =
            heap.fresh_compiled_function(&RECORD, &[order.clone(), heap.string(label.into()).unwrap()]).unwrap();
        let resolve = heap.get(&constructor, &"resolve".into(), runtime).unwrap();
        let promise = heap.call(&resolve, &constructor, &[undefined.clone()], runtime).unwrap();
        let then = heap.get(&promise, &"then".into(), runtime).unwrap();
        heap.call(&then, &promise, &[callback], runtime).unwrap();
    };
    enqueue("A", &run_runtime);
    enqueue("B", &effective);
    assert!(matches!(callbacks.drain_one().unwrap(), NativeDrain::RanJob { .. }));
    assert!(
        matches!(heap.snapshot(&heap.get(&order,&"0".into(),&effective).unwrap()).unwrap(),ValueSnapshot::String(v) if v.equals_ascii("A")),
        "display pump must drain earlier Run head before ordinary B"
    );
    let mut run = run;
    assert!(matches!(run.drain_one().unwrap(), NativeDrain::RanJob { .. }));
    assert!(
        matches!(heap.snapshot(&heap.get(&order,&"1".into(),&effective).unwrap()).unwrap(),ValueSnapshot::String(v) if v.equals_ascii("B")),
        "Run pump follows the same global head and saved ordinary owner"
    );
    drop(run_runtime);
    drop(run);
    static RETAIN_MAP: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "map override preserves original renderer mapper for a later source call",
        name: "map",
        length: 1,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 1,
        body: |heap, captures, receiver, args, effective| {
            let callback = args.first().cloned().unwrap_or(heap.undefined()?);
            heap.define_own_data(&captures[0], "mapper".into(), callback, effective)?;
            let map = heap.get(&heap.array_prototype(), &"map".into(), effective)?;
            heap.call(&map, receiver, args, effective)
        },
    };
    let cell = heap.object().unwrap();
    let files = heap.import_serde(&serde_json::json!([{"path":"a.ts","diff":""},{"path":"b.ts","diff":""}])).unwrap();
    let override_map = heap.fresh_compiled_function(&RETAIN_MAP, &[cell.clone()]).unwrap();
    heap.define_own_data(&files, "map".into(), override_map, &effective).unwrap();
    let details = heap.object().unwrap();
    heap.define_own_data(&details, "perFileResults".into(), files, &effective).unwrap();
    let root = heap.object().unwrap();
    heap.define_own_data(&root, "details".into(), details, &effective).unwrap();
    let context =
        super::types::EditRenderContext { edit_mode: Some(ara_edit::EditMode::Hashline), ..Default::default() };
    let options = super::types::RenderOptions::default();
    let graph = DisplayGraph(prepared.values(), &effective);
    let _multi = super::graph_renderer::GraphEditRenderer::result(
        &graph,
        &root,
        None,
        &super::types::EditRenderArgs::default(),
        false,
        options,
        context.clone(),
        Arc::new(super::theme::Theme::dark()),
    )
    .unwrap();
    let mapper = heap.get(&cell, &"mapper".into(), &effective).unwrap();
    let file = heap.import_serde(&serde_json::json!({"path":"retained-after-map.ts","diff":""})).unwrap();
    let child = heap.call(&mapper, &undefined, &[file], &effective).unwrap();
    let render = heap.get(&child, &"render".into(), &effective).unwrap();
    let rows = heap.call(&render, &child, &[heap.number(100.0).unwrap()], &effective).unwrap();
    assert!(super::graph_renderer::DisplayGraph(prepared.values(),&effective).members(&rows).unwrap().iter().any(|row|matches!(heap.snapshot(row).unwrap(),ValueSnapshot::String(v) if super::render_utils::sanitize(&v).contains("retained-after-map.ts"))));
    drop(effective);
    drop(callbacks);
    assert!(
        matches!(
            heap.call(&mapper, &undefined, &[heap.object().unwrap()], prepared.values().runtime()),
            Err(ValueError::ExpiredCapability(_))
        ),
        "Session callback owner close revokes retained renderer mapper"
    );
}
// Extend the existing coarse driver group through the actual evaluator path.
// This is an in-memory publication receipt, not a physical terminal claim.
#[cfg(test)]
pub(super) fn observe_summary_capability_transaction(driver: Arc<TerminalDriver>) {
    use super::{
        event_controller::SessionRow, markdown::MarkdownOptions, markdown_capabilities::MarkdownCapabilities,
        session_observation::SessionTranscriptObservation, summary_message::SummaryMessageComponent,
        types::TerminalImageProtocol,
    };
    use ara_session::context_view::{TranscriptView, TranscriptViewMessage};
    use std::sync::atomic::AtomicBool;
    let low = MarkdownCapabilities::default();
    let high = MarkdownCapabilities {
        true_color: true,
        text_sizing: true,
        image_protocol: Some(TerminalImageProtocol::Kitty),
    };
    driver.set_markdown_capabilities(low);
    let mut evaluator = DisplayEvaluator::new(driver.clone(), None, ara_edit::EditMode::Hashline).unwrap();
    let source = ara_rpc::WireValue::parse(r##"{"role":"compactionSummary","summary":"# Heading\n\n```ts\nretained-code\n```","method":"remote","tokensBefore":10,"tokensAfter":5,"extra":{"retained":true}}"##).unwrap();
    evaluator.controller.start_entry_observation();
    evaluator.controller.observe_entry_receipt("summary".into(), Arc::new(source.clone())).unwrap();
    let key = BlockKey::Session("summary".into(), 0);
    assert_eq!(evaluator.controller.transcript.block_keys(), [key.clone()]);
    // Equal restored history must retain the live receipt's same visible row.
    evaluator
        .controller
        .observe_session(
            SessionTranscriptObservation {
                owner: None,
                session_id: "actual-summary-frame".into(),
                revision: 1,
                collapsed: false,
                view: TranscriptView {
                    messages: vec![TranscriptViewMessage { entry_id: "summary".into(), body: source.clone() }],
                    cache_miss_explained_at: vec![false],
                },
                originals: HashMap::new(),
            },
            None,
        )
        .unwrap();
    assert_eq!(evaluator.controller.transcript.block_keys(), [key.clone()]);
    let change_during_render = Arc::new(AtomicBool::new(false));
    let driver_callback = driver.clone();
    let flag = change_during_render.clone();
    let mut component = SummaryMessageComponent::from_wire_with_options(
        &source,
        driver.theme(),
        MarkdownOptions {
            highlighter: Some(Arc::new(move |code, _| {
                if flag.swap(false, Ordering::AcqRel) {
                    driver_callback.set_markdown_capabilities(low);
                }
                code.lines().map(str::to_owned).collect()
            })),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    component.set_expanded(true);
    evaluator.controller.commit_session_row(&key, SessionRow::Summary(component));
    evaluator.render(Demand { seq: 1, origin: Origin::Session });
    let mut retained = evaluator.controller.session_frame_copy(&key).unwrap();
    let first = match &mut retained {
        SessionRow::Summary(c) => c.render(80),
        _ => panic!(),
    };
    assert!(first.join("\n").contains("retained-code"));
    assert!(!first.join("\n").contains("\x1b]66;"));
    assert_eq!(evaluator.controller.transcript.block_keys(), [key.clone()]);
    driver.set_markdown_capabilities(high);
    change_during_render.store(true, Ordering::Release);
    evaluator.render(Demand { seq: 2, origin: Origin::Session });
    assert_eq!(driver.display_snapshot().frame.text_sizing, low.text_sizing);
    assert!(!change_during_render.load(Ordering::Acquire));
    let mut after_rejection = evaluator.controller.session_frame_copy(&key).unwrap();
    let same = match &mut after_rejection {
        SessionRow::Summary(c) => c.render(80),
        _ => panic!(),
    };
    assert!(Arc::ptr_eq(&first, &same), "obsolete actual evaluator frame cannot commit its capability/cache changes");
    driver.set_markdown_capabilities(high);
    evaluator.render(Demand { seq: 3, origin: Origin::Session });
    let mut after_accept = evaluator.controller.session_frame_copy(&key).unwrap();
    let changed = match &mut after_accept {
        SessionRow::Summary(c) => {
            assert!(c.source().deep_equal(&source));
            c.render(80)
        }
        _ => panic!(),
    };
    assert!(!Arc::ptr_eq(&first, &changed));
    assert!(changed.join("\n").contains("\x1b]66;"));
    assert!(evaluator.receipts.is_empty());
}
