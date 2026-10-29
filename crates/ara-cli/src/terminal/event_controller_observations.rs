/// Getter-sensitive admission observations called by the existing 31-observation
/// family. This is a controlled Graph runtime, not physical terminal acceptance.
#[cfg(test)]
pub(super) fn observe_graph_admission() {
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap, ValueRuntime,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Throws {
        reason: ValueHandle,
        reads: AtomicUsize,
    }
    impl ValueRuntime for Throws {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(7801), .. } => {
                    self.reads.fetch_add(1, Ordering::SeqCst);
                    Err(ValueError::Thrown(self.reason.clone()))
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    struct CallbackRuntime {
        base: Arc<Throws>,
        domain: std::sync::OnceLock<Arc<super::graph_renderer::CallbackDomain>>,
    }
    impl ValueRuntime for CallbackRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            self.resolve_effective(request, self)
        }
        fn resolve_effective(
            &self,
            request: RuntimeRequest<'_>,
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.resolve_call(capability, receiver, arguments, effective)
                }
                request => self.base.resolve_effective(request, effective),
            }
        }
        fn resolve_call(
            &self,
            cap: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if let Some(result) =
                self.domain.get().and_then(|domain| domain.resolve_call(cap, receiver, args, effective))
            {
                result
            } else {
                self.base.resolve_call(cap, receiver, args, effective)
            }
        }
    }
    fn trap(graph: &HostGraph<'_>, root: &ValueHandle, key: &str) {
        graph
            .0
            .heap()
            .define_own(
                root,
                key.into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(7801))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                graph.runtime(),
            )
            .unwrap();
    }
    let heap = ValueHeap::new().unwrap();
    let reason = heap.object().unwrap();
    let runtime = Arc::new(Throws { reason: reason.clone(), reads: AtomicUsize::new(0) });
    let callbacks = Arc::new(CallbackRuntime { base: runtime.clone(), domain: std::sync::OnceLock::new() });
    let values = Arc::new(SessionValues::new(heap.clone(), callbacks.clone()).unwrap());
    let domain = super::graph_renderer::CallbackDomain::new(&values);
    assert!(callbacks.domain.set(domain.clone()).is_ok());
    let globals = crate::native_realm::NativeGlobals::prepare(&heap, values.runtime()).unwrap();
    let _ast = crate::native_realm::ast::NativeAstModule::prepare(&heap, &globals, values.runtime()).unwrap();
    domain.bind_environment(&globals.environment).unwrap();
    let graph = HostGraph::base(&values);
    let call = heap.import_serde(&serde_json::json!({ "file_path": "a.ts" })).unwrap();
    for key in ["oldText", "replace_all"] {
        trap(&graph, &call, key);
    }
    let mut controller = EventController::new(Arc::new(Theme::dark()), ara_edit::EditMode::Hashline);
    let generation = controller.begin_run(ara_edit::EditMode::Hashline);
    controller
        .consume(
            generation,
            &AgentEvent::ActualToolExecutionStart {
                tool_call_id: "graph-edit".into(),
                tool_name: "edit".into(),
                args: call.clone(),
                intent: heap.undefined().unwrap(),
            },
            Some(&values),
        )
        .unwrap();
    assert_eq!(
        controller.tools.get(&WireString::from("graph-edit")).unwrap().component.args.file_path,
        Some("a.ts".into())
    );
    assert_eq!(runtime.reads.load(Ordering::SeqCst), 0, "unselected payload getters remain untouched; target cache may capture path");

    let multi_args = heap.import_serde(&serde_json::json!({ "file_path": "first.ts" })).unwrap();
    for key in ["new_string", "newText", "patch", "previewDiff", "diff"] {
        trap(&graph, &multi_args, key);
    }
    let mut multi = EventController::new(Arc::new(Theme::dark()), ara_edit::EditMode::Replace);
    let multi_generation = multi.begin_run(ara_edit::EditMode::Replace);
    multi.upsert("graph-multi".into(), "edit".into(), args_from_json(&serde_json::json!({ "file_path": "first.ts" })));
    multi.preview(
        "graph-multi".into(),
        "edit".into(),
        vec![
            PerFilePreview {
                path: WireText("first.ts".into()),
                error: Some("first error".into()),
                ..Default::default()
            },
            PerFilePreview {
                path: WireText("second.ts".into()),
                diff: Some("+1|second diff".into()),
                ..Default::default()
            },
        ],
        true,
    );
    multi
        .consume(
            multi_generation,
            &AgentEvent::ActualToolExecutionStart {
                tool_call_id: "graph-multi".into(),
                tool_name: "edit".into(),
                args: multi_args,
                intent: heap.undefined().unwrap(),
            },
            Some(&values),
        )
        .unwrap();
    assert_eq!(
        runtime.reads.load(Ordering::SeqCst),
        0,
        "any existing preview diff skips replace fallback; multi preview skips payload getters"
    );

    let success =
        heap.import_serde(&serde_json::json!({ "details": { "path": "a.ts", "diff": "+1|new value" } })).unwrap();
    let success_details = graph.get(&success, "details").unwrap();
    for key in ["errorText", "displayErrorText", "oldText", "newText", "snapshotsPruned"] {
        trap(&graph, &success_details, key);
    }
    for key in ["content", "isError"] {
        trap(&graph, &success, key);
    }
    controller
        .consume(
            generation,
            &AgentEvent::ActualToolExecutionEnd {
                tool_call_id: "graph-edit".into(),
                tool_name: "edit".into(),
                result: success.clone(),
                is_error: false,
            },
            Some(&values),
        )
        .unwrap();
    let rendered = controller
        .tools
        .get_mut(&WireString::from("graph-edit"))
        .unwrap()
        .render_in(&graph, 160, &FrameContext::default(), 0, usize::MAX, std::time::Instant::now())
        .unwrap();
    assert!(rendered.iter().any(|line| line.contains("new value")));
    assert_eq!(
        runtime.reads.load(Ordering::SeqCst),
        0,
        "successful EDIT does not read error fields/content/root.isError"
    );

    let generic =
        heap.import_serde(&serde_json::json!({ "content": [{ "type": "text", "text": "bounded summary" }] })).unwrap();
    for key in ["details", "isError"] {
        trap(&graph, &generic, key);
    }
    controller
        .consume(
            generation,
            &AgentEvent::ActualToolExecutionUpdate {
                tool_call_id: "graph-generic".into(),
                tool_name: "bash".into(),
                args: heap.object().unwrap(),
                partial: generic.clone(),
            },
            Some(&values),
        )
        .unwrap();
    controller
        .consume(
            generation,
            &AgentEvent::ActualToolExecutionEnd {
                tool_call_id: "graph-generic".into(),
                tool_name: "bash".into(),
                result: generic,
                is_error: false,
            },
            Some(&values),
        )
        .unwrap();
    assert_eq!(
        runtime.reads.load(Ordering::SeqCst),
        0,
        "generic summary does not visit EDIT details or overridden root.isError"
    );

    let display_error =
        heap.import_serde(&serde_json::json!({ "details": { "displayErrorText": "chosen display error" } })).unwrap();
    trap(&graph, &graph.get(&display_error, "details").unwrap(), "errorText");
    trap(&graph, &display_error, "content");
    let chosen =
        result_from_graph(&graph, &display_error, &"edit".into(), &EditRenderArgs::default(), Some(true)).unwrap();
    assert_eq!(chosen.details.unwrap().display_error_text, Some("chosen display error".into()));
    assert_eq!(runtime.reads.load(Ordering::SeqCst), 0, "displayErrorText short-circuits errorText and content");

    let fallback_error = heap
        .import_serde(&serde_json::json!({ "details": {}, "content": [
        { "type": "text", "text": "first text only" }, { "type": "text", "text": "must not visit" }
    ] }))
        .unwrap();
    let content = graph.get(&fallback_error, "content").unwrap();
    trap(&graph, &graph.get(&content, "1").unwrap(), "type");
    let fallback =
        result_from_graph(&graph, &fallback_error, &"edit".into(), &EditRenderArgs::default(), Some(true)).unwrap();
    assert_eq!(fallback.content, vec![WireString::from("first text only")]);
    assert_eq!(runtime.reads.load(Ordering::SeqCst), 0, "error content.find stops after its first text block");

    // A getter that the selected branch actually needs still throws its original
    // handle; admission must not swallow it or replace it with a display default.
    let required_path = heap.object().unwrap();
    trap(&graph, &required_path, "path");
    match args_from_graph(&graph, &required_path, ara_edit::EditMode::Hashline, false, false) {
        Err(ValueError::Thrown(thrown)) => assert!(thrown.same_identity(&reason)),
        other => panic!("required path getter must preserve its throw: {other:?}"),
    }
    let required_error = heap.import_serde(&serde_json::json!({ "details": {} })).unwrap();
    trap(&graph, &graph.get(&required_error, "details").unwrap(), "displayErrorText");
    match result_from_graph(&graph, &required_error, &"edit".into(), &EditRenderArgs::default(), Some(true)) {
        Err(ValueError::Thrown(thrown)) => assert!(thrown.same_identity(&reason)),
        other => panic!("required error getter must preserve its throw: {other:?}"),
    }
    assert_eq!(runtime.reads.load(Ordering::SeqCst), 2);

    // Target resolveEditCallFacts snapshots direct cache operands even when
    // file_path wins the path-selection branch. Keep this throw observable.
    let cached_path = heap.import_serde(&serde_json::json!({ "file_path": "cache.ts" })).unwrap();
    trap(&graph, &cached_path, "path");
    match super::graph_renderer::GraphEditRenderer::activity(
        &graph,
        &cached_path,
        RenderOptions::default(),
        &EditRenderContext { edit_mode: Some(ara_edit::EditMode::Hashline), ..Default::default() },
    ) {
        Err(ValueError::Thrown(thrown)) => assert!(thrown.same_identity(&reason)),
        _ => panic!("target direct cache path getter must preserve its throw despite a selected file_path"),
    }
    assert_eq!(runtime.reads.load(Ordering::SeqCst), 3);
}
