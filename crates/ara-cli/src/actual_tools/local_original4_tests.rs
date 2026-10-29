//! Full original Local4 bodies, fixed OMP596f2da write-xdev-dispatch.test.ts.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; THIRD_PARTY_NOTICES.md.
//! Called by the existing coarse xdev entrance; no new Cargo test entrance.
use super::{
    BuiltinKind, BuiltinToolSource, NativeDeviceDecorator, NoUiDeviceApproval,
    local_source::NativeLocalSource,
};
use crate::{
    native_factory::{
        NativeCatalogFactoryImports, PreparedNativeSession,
        catalog::{
            NativeCatalogMember, NativeCatalogOwner, NativeCatalogSnapshot, NativeCatalogSources,
            NativeDeviceOptions, NativeToolCatalog,
        },
    },
    native_realm::NativeRealm,
    native_run::NativeRun,
    terminal::{
        graph_renderer::DisplayGraph,
        theme::Theme,
        tool_execution::ToolExecutionComponent,
        types::{EditRenderArgs, EditResult, FrameContext, RenderOptions},
        xdev_renderer::XdevComponent,
    },
};
use ara_agent::{
    ActualExecuteRequest, ActualProviderError, ActualToolBinding, ActualToolSource, AgentMessage,
    SessionValues,
};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, StringifyMode, ValueError, ValueHandle,
        ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use futures::future::BoxFuture;
use std::sync::Arc;
fn get(v: &ValueHandle, k: &str, r: &dyn ValueRuntime) -> Result<ValueHandle, ValueError> {
    v.heap().get(v, &k.into(), r)
}
fn string(v: &ValueHandle) -> WireString {
    match v.heap().snapshot(v).unwrap() {
        ValueSnapshot::String(s) => s,
        _ => panic!("expected original string"),
    }
}
fn own(values: &SessionValues, v: &ValueHandle, k: &str, x: ValueHandle) {
    values
        .heap()
        .define_own_data(v, k.into(), x, values.runtime())
        .unwrap()
}
fn text_result(values: &SessionValues, result: &ValueHandle) -> String {
    let content = get(result, "content", values.runtime()).unwrap();
    let text = get(
        &get(&content, "0", values.runtime()).unwrap(),
        "text",
        values.runtime(),
    )
    .unwrap();
    string(&text).to_utf8().unwrap()
}
// AsyncFunction reflection is not claimed. The original async bodies are lowered
// into genuine captured intrinsic Promise.resolve/reject calls, not ready results.
static PEEK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da write-xdev-dispatch.test.ts:124 async execute() return fresh peeked content",
    name: "execute",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, c, _, _, r| {
        let result =
            heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"peeked"}]}))?;
        heap.call(&c[1], &c[0], &[result], r)
    },
};
static WEATHER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da write-xdev-dispatch.test.ts:423 async execute() return fresh Tokyo text",
    name: "execute",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, c, _, _, r| {
        let result = heap
            .import_serde(&serde_json::json!({"content":[{"type":"text","text":"Tokyo: 22°C"}]}))?;
        heap.call(&c[1], &c[0], &[result], r)
    },
};
static GITHUB: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da write-xdev-dispatch.test.ts:377 async execute() throw actual ToolError gh404",
    name: "execute",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, c, _, _, r| {
        let error = heap.construct(&c[2], &[heap.string("gh: Not Found (HTTP 404)".into())?], r)?;
        heap.call(&c[1], &c[0], &[error], r)
    },
};
static REVISED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da write-xdev-dispatch.test.ts:187 async execute(_id,args) validation/capture/fresh done",
    name: "execute",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 5,
    body: |heap, c, _, args, r| {
        let body = (|| {
            let args = args.get(1).cloned().unwrap_or(heap.undefined()?);
            if !heap.truthy(&args)?
                || !matches!(heap.kind(&args)?, ValueKind::Object | ValueKind::Array)
                || !heap.has(&args, &"q".into(), r)?
                || heap.kind(&get(&args, "q", r)?)? != ValueKind::String
            {
                return Err(ValueError::Thrown(heap.construct(
                    &c[4],
                    &[heap.string("Expected a string query".into())?],
                    r,
                )?));
            }
            heap.define_own_data(&c[3], "executedQuery".into(), get(&args, "q", r)?, r)?;
            heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"done"}]}))
        })();
        match body {
            Ok(value) => heap.call(&c[1], &c[0], &[value], r),
            Err(ValueError::Thrown(error)) => heap.call(&c[2], &c[0], &[error], r),
            Err(error) => Err(error),
        }
    },
};
static READ_TIER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da write-xdev-dispatch.test.ts:123 approval () => read",
    name: "approval",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, _, _| heap.string("read".into()),
};
static REVISED_TIER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da write-xdev-dispatch.test.ts:186 approval args truth/object/q/mutate",
    name: "approval",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, r| {
        let args = args.first().cloned().unwrap_or(heap.undefined()?);
        let write = heap.truthy(&args)?
            && matches!(heap.kind(&args)?, ValueKind::Object | ValueKind::Array)
            && heap.has(&args, &"q".into(), r)?
            && matches!(heap.snapshot(&get(&args,"q",r)?)?,ValueSnapshot::String(s) if s.equals_ascii("mutate"));
        heap.string(if write { "write" } else { "read" }.into())
    },
};
struct RevisedDecorator;
impl NativeDeviceDecorator for RevisedDecorator {
    fn effective_arguments(
        &self,
        binding: ActualToolBinding,
        args: ValueHandle,
        context: ValueHandle,
        runtime: Arc<dyn ValueRuntime>,
    ) -> BoxFuture<'static, Result<ValueHandle, ActualProviderError>> {
        // The original spread/decorated execute preserves canonical callable and
        // all remaining arguments. The Native decorator supplies its actual revised
        // args/context then the owned Local source calls canonical execute once.
        let result = (|| {
            let heap = args.heap();
            let name = get(&binding.source, "name", runtime.as_ref())?;
            if matches!(heap.snapshot(&name)?,ValueSnapshot::String(s) if s.equals_ascii("write")) {
                return Ok(args);
            }
            assert!(string(&name).equals_ascii("peek"));
            let revised = heap.import_serde(&serde_json::json!({"q":"mutate"}))?;
            if !matches!(heap.kind(&context)?, ValueKind::Null | ValueKind::Undefined) {
                let method = get(&context, "xdevTierResolved", runtime.as_ref())?;
                if !matches!(heap.kind(&method)?, ValueKind::Null | ValueKind::Undefined) {
                    heap.call(
                        &method,
                        &context,
                        &[heap.string("write".into())?],
                        runtime.as_ref(),
                    )?;
                }
            }
            Ok(revised)
        })();
        Box::pin(async move { result })
    }
}
fn render_original(
    values: &SessionValues,
    graph: &DisplayGraph<'_>,
    theme: &Arc<Theme>,
    args: &ValueHandle,
    result: &ValueHandle,
    id: &str,
    error: bool,
) -> Vec<String> {
    let write_args = crate::terminal::write_renderer::args_from_graph(graph, args).unwrap();
    let options = RenderOptions {
        expanded: false,
        is_partial: false,
        spinner_frame: None,
    };
    let direct = XdevComponent::build(
        graph,
        &write_args,
        Some(result),
        options,
        true,
        true,
        theme,
        None,
    )
    .unwrap()
    .expect("original WRITE renderResult component")
    .render(graph, 80, theme)
    .unwrap();
    let mut component = ToolExecutionComponent::new(
        id.into(),
        "write".into(),
        EditRenderArgs::default(),
        ara_edit::EditMode::Hashline,
        theme.clone(),
    );
    component.graph_args_root = Some(args.clone());
    component.result_root = Some(result.clone());
    component.args_complete = true;
    component.execution_started = true;
    component.component.options = options;
    component.component.result = Some(EditResult {
        is_error: Some(error),
        ..Default::default()
    });
    assert!(
        component.rebuild_in(graph).is_empty(),
        "original WRITE production renderer construction"
    );
    let rendered = component
        .render_in(
            graph,
            80,
            &FrameContext::default(),
            0,
            usize::MAX,
            std::time::Instant::now(),
        )
        .unwrap();
    assert_eq!(
        rendered, direct,
        "original renderResult and real WRITE component differ"
    );
    assert!(values.heap().kind(result).is_ok());
    rendered
}
pub(crate) fn exercise_original_local4() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for (id, call_id, name, key, input, description) in [
                (
                    "B-7e7425a85c",
                    "write-xdev-read",
                    "peek",
                    "q",
                    "x",
                    "Read-only device",
                ),
                (
                    "B-b6e88a1074",
                    "write-xdev-revised",
                    "peek",
                    "q",
                    "inspect",
                    "Argument-dependent device",
                ),
                (
                    "B-3ab251b6c3",
                    "write-xdev-error",
                    "github",
                    "op",
                    "repo_view",
                    "fixture",
                ),
                (
                    "B-dd59949e02",
                    "write-xdev-default-renderer",
                    "weather",
                    "query",
                    "Tokyo",
                    "Gets the weather",
                ),
            ] {
                eprintln!("[original22] {id} begin");
                let realm = NativeRealm::prepare().unwrap();
                let values = &realm.values;
                let heap = values.heap();
                let display = realm.begin_display_callbacks().unwrap();
                let display_runtime = display.runtime().unwrap();
                let graph = DisplayGraph(values, &display_runtime);
                let imported_type =
                    super::omp_string_object::prepare(values, &realm.globals.errors.error).unwrap();
                let definition = heap.object().unwrap();
                own(
                    values,
                    &definition,
                    key,
                    heap.string("string".into()).unwrap(),
                );
                let parameters = heap
                    .call(
                        &imported_type,
                        &heap.undefined().unwrap(),
                        &[definition],
                        values.runtime(),
                    )
                    .unwrap();
                assert_eq!(heap.kind(&parameters).unwrap(), ValueKind::Function);
                if id == "B-7e7425a85c" {
                    // Supporting bridge observations, not additional original test IDs.
                    let positive = heap
                        .import_serde(&serde_json::json!({"q":"x","extra":true}))
                        .unwrap();
                    let out = heap
                        .call(
                            &parameters,
                            &heap.undefined().unwrap(),
                            &[positive.clone()],
                            values.runtime(),
                        )
                        .unwrap();
                    assert!(out.same_identity(&positive));
                    assert!(
                        heap.truthy(&get(&out, "extra", values.runtime()).unwrap())
                            .unwrap()
                    );
                    let negative = heap.import_serde(&serde_json::json!({"q":1})).unwrap();
                    let errors = heap
                        .call(
                            &parameters,
                            &heap.undefined().unwrap(),
                            &[negative],
                            values.runtime(),
                        )
                        .unwrap();
                    assert!(
                        heap.instance_of(
                            &errors,
                            &get(&imported_type, "errors", values.runtime()).unwrap(),
                            values.runtime()
                        )
                        .unwrap()
                    );
                    assert!(
                        string(
                            &get(
                                &get(&errors, "0", values.runtime()).unwrap(),
                                "message",
                                values.runtime()
                            )
                            .unwrap()
                        )
                        .equals_ascii("q must be a string (was a number)")
                    );
                }
                let schema = heap
                    .call(
                        &get(&parameters, "toJsonSchema", values.runtime()).unwrap(),
                        &parameters,
                        &[],
                        values.runtime(),
                    )
                    .unwrap();
                assert_eq!(
                    heap.kind(&get(&schema, "additionalProperties", values.runtime()).unwrap())
                        .unwrap(),
                    ValueKind::Undefined,
                    "original omptype extras keep"
                );
                let schema_json = heap
                    .stringify(&schema, StringifyMode::Plain, None, values.runtime())
                    .unwrap()
                    .unwrap()
                    .to_utf8()
                    .unwrap();
                let promise = get(&realm.globals.environment, "Promise", values.runtime()).unwrap();
                let resolve = get(&promise, "resolve", values.runtime()).unwrap();
                let reject = get(&promise, "reject", values.runtime()).unwrap();
                let state = heap.object().unwrap();
                own(values, &state, "executedQuery", heap.undefined().unwrap());
                let (spec, captures) = match id {
                    "B-7e7425a85c" => (&PEEK, vec![promise.clone(), resolve.clone()]),
                    "B-b6e88a1074" => (
                        &REVISED,
                        vec![
                            promise.clone(),
                            resolve.clone(),
                            reject.clone(),
                            state.clone(),
                            realm.globals.errors.error.clone(),
                        ],
                    ),
                    "B-3ab251b6c3" => (
                        &GITHUB,
                        vec![
                            promise.clone(),
                            reject.clone(),
                            realm.ast.tool_error.clone(),
                        ],
                    ),
                    _ => (&WEATHER, vec![promise.clone(), resolve.clone()]),
                };
                let device = heap.object().unwrap();
                for (k, v) in [
                    ("name", heap.string(name.into()).unwrap()),
                    (
                        "label",
                        heap.string(
                            if name == "peek" {
                                "Peek"
                            } else if name == "github" {
                                "GitHub"
                            } else {
                                "Weather"
                            }
                            .into(),
                        )
                        .unwrap(),
                    ),
                    ("description", heap.string(description.into()).unwrap()),
                    ("parameters", parameters),
                    (
                        "execute",
                        heap.fresh_compiled_function(spec, &captures).unwrap(),
                    ),
                ] {
                    own(values, &device, k, v)
                }
                if name == "peek" {
                    own(
                        values,
                        &device,
                        "approval",
                        heap.fresh_compiled_function(
                            if id == "B-b6e88a1074" {
                                &REVISED_TIER
                            } else {
                                &READ_TIER
                            },
                            &[],
                        )
                        .unwrap(),
                    )
                }
                if name == "github" {
                    heap.spread_into(&device, &graph.github_renderer().unwrap(), &display_runtime)
                        .unwrap()
                }
                let originals = heap.array(vec![device.clone()]).unwrap();
                let local = NativeLocalSource::new(
                    values,
                    originals.clone(),
                    vec![ara_ai::Tool {
                        name: name.into(),
                        description: description.into(),
                        parameters: serde_json::from_str(&schema_json).unwrap(),
                        ..Default::default()
                    }],
                    vec![name.into()],
                )
                .unwrap();
                assert!(
                    local
                        .current_tools(values)
                        .unwrap()
                        .unwrap()
                        .same_identity(&originals)
                );
                let cwd = std::env::current_dir().unwrap();
                let builtin = BuiltinToolSource::new(
                    values,
                    ara_tools::ToolContext::new(cwd),
                    &[BuiltinKind::Write],
                )
                .unwrap();
                builtin.bind_ast_module(realm.ast.clone(), values).unwrap();
                let builtin_array = builtin.current_tools(values).unwrap().unwrap();
                let write = get(&builtin_array, "0", values.runtime()).unwrap();
                let registry = vec![
                    NativeCatalogMember::bind(
                        NativeCatalogOwner::Builtin(builtin.clone()),
                        &write,
                        values,
                    )
                    .unwrap(),
                    NativeCatalogMember::bind(
                        NativeCatalogOwner::Local(local.clone()),
                        &get(&originals, "0", values.runtime()).unwrap(),
                        values,
                    )
                    .unwrap(),
                ];
                let collection = heap.array(vec![write.clone(), device.clone()]).unwrap();
                // Original Settings.isolated({}) uses fixed approvalMode default yolo.
                let snapshot = NativeCatalogSnapshot::new_with_local_sources(
                    values,
                    collection,
                    registry,
                    NativeCatalogSources {
                        builtin: builtin.clone(),
                        mcp: None,
                        rpc: None,
                    },
                    NativeDeviceOptions {
                        enabled: true,
                        approval_host: Some(Arc::new(NoUiDeviceApproval {
                            mode: "yolo".into(),
                            policies: WireValue::Object(Vec::new()),
                        })),
                        decorator: if id == "B-b6e88a1074" {
                            Some(Arc::new(RevisedDecorator))
                        } else {
                            None
                        },
                        ark_type: Some(imported_type),
                        mounted_local_names: Some(vec![name.into()]),
                        ..Default::default()
                    },
                    vec![local.clone()],
                )
                .unwrap();
                assert_eq!(snapshot.device_names(), &[WireString::from(name)]);
                let catalog = NativeToolCatalog::new(values, snapshot).unwrap();
                display
                    .domain()
                    .bind_device_catalog(Some(&catalog))
                    .unwrap();
                let prepared = PreparedNativeSession::from_realm(realm.clone(), None)
                    .unwrap()
                    .finish_catalog_bindings(NativeCatalogFactoryImports {
                        catalog,
                        string_function: realm.globals.string.clone(),
                        bun: realm.globals.bun.clone(),
                        original_logger: None,
                    })
                    .unwrap();
                let cancel = tokio_util::sync::CancellationToken::new();
                let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
                let content_object = heap.object().unwrap();
                own(
                    values,
                    &content_object,
                    key,
                    heap.string(input.into()).unwrap(),
                );
                let json = get(&realm.globals.environment, "JSON", values.runtime()).unwrap();
                let content = heap
                    .call(
                        &get(&json, "stringify", values.runtime()).unwrap(),
                        &json,
                        &[content_object],
                        values.runtime(),
                    )
                    .unwrap();
                let args = heap.object().unwrap();
                own(
                    values,
                    &args,
                    "path",
                    heap.string(format!("xd://{name}").into()).unwrap(),
                );
                own(values, &args, "content", content);
                let binding = builtin.bind_selected(&write, values).unwrap();
                let call = heap
                    .import_serde(
                        &serde_json::json!({"type":"toolCall","id":call_id,"name":"write"}),
                    )
                    .unwrap();
                own(values, &call, "arguments", args.clone());
                let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty(
                    "fixture", "fixture", "fixture",
                ))
                .unwrap();
                assistant["role"] = "assistant".into();
                assistant["stopReason"] = "toolUse".into();
                let root = heap.import_serde(&assistant).unwrap();
                own(
                    values,
                    &root,
                    "content",
                    heap.array(vec![call.clone()]).unwrap(),
                );
                let owner = AgentMessage::from_graph(root, None).unwrap();
                let context = heap.object().unwrap();
                prepared
                    .require_tool_hooks()
                    .unwrap()
                    .bind_run_context(values, &context)
                    .unwrap();
                let request = ActualExecuteRequest {
                    callable: get(&write, "execute", values.runtime()).unwrap(),
                    receiver: write.clone(),
                    arguments_slot: owner.current_arguments_slot(&call).unwrap(),
                    owner,
                    tool_call: call.clone(),
                    call_id: call_id.into(),
                    args: args.clone(),
                    values: values.clone(),
                    cancel,
                    on_update: Arc::new(|_| Ok(())),
                    tool_context: context,
                };
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    driver.drive(
                        binding
                            .capability
                            .actual_invoker()
                            .unwrap()
                            .invoke_settled(request),
                    ),
                )
                .await
                .unwrap()
                .unwrap()
                .unwrap();
                let xdev = get(
                    &get(&result, "details", values.runtime()).unwrap(),
                    "xdev",
                    values.runtime(),
                )
                .unwrap();
                if id == "B-7e7425a85c" {
                    assert_eq!(
                        heap.kind(&get(&result, "isError", values.runtime()).unwrap())
                            .unwrap(),
                        ValueKind::Undefined
                    );
                    assert!(
                        string(&get(&xdev, "tool", values.runtime()).unwrap()).equals_ascii("peek")
                    );
                    assert!(
                        string(&get(&xdev, "mode", values.runtime()).unwrap())
                            .equals_ascii("execute")
                    );
                    assert!(
                        string(&get(&xdev, "tier", values.runtime()).unwrap()).equals_ascii("read")
                    );
                } else if id == "B-b6e88a1074" {
                    assert!(
                        string(&get(&state, "executedQuery", values.runtime()).unwrap())
                            .equals_ascii("mutate")
                    );
                    assert!(
                        string(&get(&xdev, "tier", values.runtime()).unwrap())
                            .equals_ascii("write")
                    );
                } else {
                    let error = id == "B-3ab251b6c3";
                    if error {
                        assert!(
                            heap.truthy(&get(&result, "isError", values.runtime()).unwrap())
                                .unwrap()
                        );
                        assert!(
                            string(&get(&xdev, "tool", values.runtime()).unwrap())
                                .equals_ascii("github")
                        );
                        assert!(
                            string(&get(&xdev, "mode", values.runtime()).unwrap())
                                .equals_ascii("execute")
                        );
                        assert!(
                            string(
                                &get(
                                    &get(&xdev, "args", values.runtime()).unwrap(),
                                    "op",
                                    values.runtime()
                                )
                                .unwrap()
                            )
                            .equals_ascii("repo_view")
                        );
                    }
                    let theme = Arc::new(Theme::dark());
                    let lines = render_original(values, &graph, &theme, &args, &result, id, error);
                    let rendered = lines
                        .iter()
                        .map(|row| crate::terminal::render_utils::sanitize(&row.as_str().into()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if error {
                        assert!(rendered.contains("GitHub Repo"), "{id}: {rendered}");
                        assert!(
                            rendered.contains("gh: Not Found (HTTP 404)"),
                            "{id}: {rendered}"
                        );
                        assert!(!rendered.contains("Write"), "{id}: {rendered}");
                    } else {
                        assert!(rendered.contains("Weather"), "{id}: {rendered}");
                        assert!(rendered.contains("query=\"Tokyo\""), "{id}: {rendered}");
                        assert!(rendered.contains("Tokyo: 22°C"), "{id}: {rendered}");
                        let theme_root = graph.device_theme(&theme).unwrap();
                        let bg = get(&theme_root, "bg", &display_runtime).unwrap();
                        let probe = heap
                            .call(
                                &bg,
                                &theme_root,
                                &[
                                    heap.string("toolSuccessBg".into()).unwrap(),
                                    heap.string("|".into()).unwrap(),
                                ],
                                &display_runtime,
                            )
                            .unwrap();
                        let probe = string(&probe).to_utf8().unwrap();
                        let prefix = &probe[..probe.find('|').unwrap()];
                        assert!(!prefix.is_empty());
                        assert!(
                            lines.iter().any(|line| line.contains(prefix)),
                            "{id}: background missing"
                        );
                    }
                }
                assert!(
                    get(&call, "arguments", values.runtime())
                        .unwrap()
                        .same_identity(&args)
                );
                println!(
                    "original-write-local:pass:{id}:{}",
                    text_result(values, &result)
                );
            }
        });
}
