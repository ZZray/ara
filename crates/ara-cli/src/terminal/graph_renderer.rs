//! Retained fixed OMP 596f2da EDIT callbacks. Original handles, ordered Gets,
//! and construction facts stay separate from per-callback frame operands.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    edit_renderer::{self, EditComponent},
    theme::Theme,
    types::*,
};
use ara_agent::SessionValues;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyKey, StringifyMode, ValueError, ValueHandle, ValueKind, ValueRuntime,
        ValueSnapshot,
    },
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
#[derive(Clone, Default)]
struct SelectedOperands {
    path: Option<ValueHandle>,
    rename: Option<ValueHandle>,
    apply_error: Option<ValueHandle>,
}
#[derive(Clone)]
struct FactsCache {
    root: ValueHandle,
    partial: bool,
    mode: Option<ara_edit::EditMode>,
    length: usize,
    payload: Option<WireString>,
    direct: [ValueHandle; 5],
    value: (EditRenderArgs, edit_renderer::CallFacts, SelectedOperands),
}
struct ApplyInputSummary {
    entries: Vec<edit_renderer::InputEntry>,
    error: Option<ValueHandle>,
}
static MAP_EDIT_PATH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da edit renderer countEditFiles: edit => filePathFromEditEntry(edit.path)",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let entry = args.first().cloned().unwrap_or(heap.undefined()?);
        let path = heap.get(&entry, &"path".into(), runtime)?;
        if matches!(heap.snapshot(&path)?, ValueSnapshot::String(_)) { Ok(path) } else { heap.undefined() }
    },
};

pub(crate) const DISPLAY_MAPPER: ara_rpc::value_heap::CapabilityId =
    ara_rpc::value_heap::CapabilityId(0x4152415f45444d50);
pub(crate) const DISPLAY_RENDER: ara_rpc::value_heap::CapabilityId =
    ara_rpc::value_heap::CapabilityId(0x4152415f45445244);
pub(crate) const DISPLAY_INVALIDATE: ara_rpc::value_heap::CapabilityId =
    ara_rpc::value_heap::CapabilityId(0x4152415f45444956);
pub(crate) fn callback_claims(cap: ara_rpc::value_heap::CapabilityId) -> bool {
    matches!(cap, DISPLAY_MAPPER | DISPLAY_RENDER | DISPLAY_INVALIDATE | DISPLAY_THEME | DISPLAY_GITHUB)
}
const DISPLAY_GITHUB:ara_rpc::value_heap::CapabilityId=ara_rpc::value_heap::CapabilityId(0x4152415f47485244);
const DISPLAY_THEME:ara_rpc::value_heap::CapabilityId=ara_rpc::value_heap::CapabilityId(0x4152415f58445448);
static NEXT_CALLBACK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static CALLBACK_OWNERS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<u64, std::sync::Weak<CallbackDomain>>>,
> = std::sync::OnceLock::new();
#[derive(Clone)]
pub(crate) struct DisplayRenderState {
    pub options: RenderOptions,
    pub context: EditRenderContext,
    pub frame: FrameContext,
}
type SharedState = Arc<std::sync::Mutex<DisplayRenderState>>;
type ChildRecord = (ValueHandle, u64, GraphEditRenderer);
#[derive(Clone)]
enum Callback {
    GithubMethod {is_result:bool,
    },
    GithubComponent {component:Arc<std::sync::Mutex<super::github_renderer::Component>>,
    },
    Theme { theme:Arc<Theme>,
    },
    Mapper { shared: SharedState, theme: Arc<Theme>, children: Arc<std::sync::Mutex<Vec<ChildRecord>>>,
    },
    Child { shared: SharedState, renderer: Arc<std::sync::Mutex<GraphEditRenderer>>,
    },
}
pub(crate) struct CallbackDomain {
    values: std::sync::Weak<SessionValues>,
    heap_token: u64,
    closed: std::sync::atomic::AtomicBool,
    environment: std::sync::Mutex<Option<ValueHandle>>,
    sanitizer: std::sync::Mutex<Option<ValueHandle>>,
    device_catalog: std::sync::Mutex<Option<std::sync::Weak<crate::native_factory::catalog::NativeToolCatalog>>>,
    theme_values:std::sync::Mutex<std::collections::HashMap<(u64,u64),ValueHandle>>,
    theme_languages:std::sync::Mutex<Option<ValueHandle>>,
    github_renderer:std::sync::Mutex<Option<ValueHandle>>,
    github_imports:std::sync::Mutex<Option<Arc<super::github_renderer::Imports>>>,
    github_expand_hint:std::sync::Mutex<Option<ValueHandle>>,
    github_match_initially_missing:bool,
    theme_spinner_values:std::sync::Mutex<std::collections::HashMap<(u64,u64),ValueHandle>>,
    entries: std::sync::Mutex<std::collections::HashMap<u64, Callback>>,
    facts_cache: std::sync::Mutex<Option<FactsCache>>,
}
impl CallbackDomain {
    pub(crate) fn new(values: &Arc<SessionValues>) -> Arc<Self> {
        // Inspect descriptors, never invoke a programmable match getter. The
        // current pristine Native profile does not provision String.match.
        let github_match_initially_missing=(||->Result<bool>{
            let heap=values.heap();let primitive=heap.string("".into())?;
            let Some(proto)=heap.prototype(&primitive,values.runtime())?else{return Ok(false);
            };
            Ok(heap.own_descriptor(&proto,&"match".into(),values.runtime())?.is_none()
                &&heap.own_descriptor(&heap.object_prototype(),&"match".into(),values.runtime())?.is_none())
        })().unwrap_or(false);
        let owner = Arc::new(Self {
            values: Arc::downgrade(values),
            heap_token: values.heap().heap_token(),
            closed: std::sync::atomic::AtomicBool::new(false),
            environment: std::sync::Mutex::new(None),
            sanitizer: std::sync::Mutex::new(None),
            device_catalog: std::sync::Mutex::new(None),
            theme_values:std::sync::Mutex::new(std::collections::HashMap::new()),
            theme_languages:std::sync::Mutex::new(None),
            github_renderer:std::sync::Mutex::new(None),
            github_imports:std::sync::Mutex::new(None),
            github_expand_hint:std::sync::Mutex::new(None),
            github_match_initially_missing,
            theme_spinner_values:std::sync::Mutex::new(std::collections::HashMap::new()),
            entries: std::sync::Mutex::new(std::collections::HashMap::new()),
            facts_cache: std::sync::Mutex::new(None),
        });
        let mut owners = CALLBACK_OWNERS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        owners.retain(|_, owner| owner.strong_count() > 0);
        owners.insert(owner.heap_token, Arc::downgrade(&owner));
        owner
    }
    /// Presentation-only view of the existing current/active canonical owner.
    /// The callback domain keeps a Weak reference and grants no execution.
    pub(crate) fn bind_device_catalog(&self,catalog:Option<&Arc<crate::native_factory::catalog::NativeToolCatalog>>,
    )->Result<()> {
        let mut current=self.device_catalog.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(catalog)=catalog {
            catalog.check_values(self.values()?.as_ref()).map_err(|error|match error { ara_agent::ActualProviderError::Value(error)=>error,_=>ValueError::UnsupportedOperation("display catalog requires original Session"),
                })?;
            if let Some(prior)=current.as_ref().and_then(std::sync::Weak::upgrade) {
                if !Arc::ptr_eq(&prior,catalog) {return Err(ValueError::UnsupportedOperation("display catalog owner differs"));
                }
            }
            *current=Some(Arc::downgrade(catalog));
        }
        Ok(())
    }
    fn theme_value(&self,theme:&Arc<Theme>,runtime:&dyn ValueRuntime)->Result<ValueHandle>{
        let key=(theme.identity,theme.revision);
        if let Some(value)=self.theme_values.lock().unwrap_or_else(|e|e.into_inner()).get(&key).cloned(){return Ok(value);
        }
        let values=self.values()?;let heap=values.heap();let theme_id=self.register(Callback::Theme{theme:theme.clone()})?;
        let value=heap.object()?;let token=heap.number(theme_id as f64)?;
        for (name,spec) in [("fg",&THEME_FG),("bg",&THEME_BG),("symbol",&THEME_SYMBOL),("styledSymbol",&THEME_STYLED_SYMBOL),("getLangIcon",&THEME_LANG),
        ] {
            heap.define_own_data(&value,name.into(),heap.fresh_compiled_function(spec,&[token.clone()])?,runtime,
            )?;
        }
        heap.define_own_data(&value,"getSpinnerFrames".into(),heap.fresh_compiled_function(&THEME_SPINNER_METHOD,&[token.clone()])?,runtime,
        )?;
        for(name,spec)in [("status",&THEME_STATUS_GET),("sep",&THEME_SEP_GET),("format",&THEME_FORMAT_GET),("boxRound",&THEME_BOX_GET),("spinnerFrames",&THEME_SPINNER_GET),
        ]{
            let getter=heap.fresh_compiled_function(spec,&[token.clone()])?;
            let ValueSnapshot::Function(target)=heap.snapshot(&getter)?else{return Err(ValueError::WrongRuntimeReply("original Theme getter callable"));
            };
            heap.define_own(&value,name.into(),ara_rpc::value_heap::PropertyDescriptor::Accessor{get:Some(target),set:None,enumerable:false,configurable:true,
                },runtime,
            )?;
        }
        let tree=heap.object()?;
        for key in ["last","branch","vertical"]{heap.define_own_data(&tree,key.into(),heap.string(theme.symbol(&format!("tree.{key}")).into())?,runtime,
            )?;}
        heap.define_own_data(&value,"tree".into(),tree,runtime)?;
        self.theme_values.lock().unwrap_or_else(|e|e.into_inner()).insert(key,value.clone());
        Ok(value)
    }
    fn theme_receiver(&self, receiver:&ValueHandle)->Result<Arc<Theme>> {
        let key=self.theme_values.lock().unwrap_or_else(|e|e.into_inner()).iter()
            .find_map(|(key,value)|value.same_identity(receiver).then_some(*key))
            .ok_or_else(|| {
                ValueError::TypeError("Cannot read Theme private member from an unbranded receiver".into())
            })?;
        self.entries.lock().unwrap_or_else(|e|e.into_inner()).values().find_map(|entry|match entry {
            Callback::Theme{theme} if (theme.identity,theme.revision)==key=>Some(theme.clone()),
            _=>None,
        }).ok_or(ValueError::ExpiredCapability(DISPLAY_THEME))
    }
    fn theme_languages(&self, graph:&DisplayGraph<'_>)->Result<ValueHandle> {
        if let Some(value)=self.theme_languages.lock().unwrap_or_else(|e|e.into_inner()).clone(){return Ok(value);
        }
        let heap=graph.0.heap();let value=heap.object()?;
        for (language,key) in THEME_LANGUAGE_KEYS {
            heap.define_own_data(&value,(*language).into(),heap.string((*key).into())?,graph.runtime())?;
        }
        *self.theme_languages.lock().unwrap_or_else(|e|e.into_inner())=Some(value.clone());
        Ok(value)
    }
    pub(crate) fn bind_github_expand_key_hint(&self,callback:&ValueHandle)->Result<()> {
        self.values()?.check_handle(callback)?;
        if self.values()?.heap().kind(callback)?!=ValueKind::Function{return Err(ValueError::TypeError("Host expand-key hint must be callable".into()));
        }
        let mut slot=self.github_expand_hint.lock().unwrap_or_else(|e|e.into_inner());
        if slot.as_ref().is_some_and(|old|!old.same_identity(callback)){return Err(ValueError::UnsupportedOperation("original Host expand-key hint is already bound",
            ));
        }
        *slot=Some(callback.clone());Ok(())
    }
    fn github_imports(&self,graph:&DisplayGraph<'_>)->Result<Arc<super::github_renderer::Imports>>{
        if let Some(imports)=self.github_imports.lock().unwrap_or_else(|e|e.into_inner()).clone(){return Ok(imports);
        }
        let imports=super::github_renderer::Imports::prepare(graph)?;
        *self.github_imports.lock().unwrap_or_else(|e|e.into_inner())=Some(imports.clone());Ok(imports)
    }
    fn github_renderer(&self,graph:&DisplayGraph<'_>)->Result<ValueHandle>{
        if let Some(value)=self.github_renderer.lock().unwrap_or_else(|e|e.into_inner()).clone(){return Ok(value);
        }
        self.github_imports(graph)?;
        let heap=graph.0.heap();let value=heap.object()?;
        for(name,spec,is_result)in [("renderCall",&GITHUB_CALL,false),("renderResult",&GITHUB_RESULT,true),
        ]{
            let id=self.register(Callback::GithubMethod{is_result})?;
            heap.define_own_data(&value,name.into(),heap.fresh_compiled_function(spec,&[heap.number(id as f64)?])?,graph.runtime(),
            )?;
        }
        heap.define_own_data(&value,"mergeCallAndResult".into(),heap.boolean(true)?,graph.runtime(),
        )?;
        *self.github_renderer.lock().unwrap_or_else(|e|e.into_inner())=Some(value.clone());Ok(value)
    }
    pub(crate) fn bind_environment(&self, environment: &ValueHandle) -> Result<()> {
        self.values()?.check_handle(environment)?;
        let mut binding = self.environment.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = binding.as_ref() {
            if !previous.same_identity(environment) {
                return Err(ValueError::UnsupportedOperation("display callbacks already have an environment",
                ));
            }
        } else {
            *binding = Some(environment.clone());
        }
        Ok(())
    }
    fn environment(&self) -> Result<ValueHandle> {
        self.values()?;
        self.environment
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(ValueError::UnsupportedOperation("display callback environment is not connected",
            ))
    }
    fn for_values(values: &SessionValues) -> Result<Arc<Self>> {
        let owner = CALLBACK_OWNERS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&values.heap().heap_token())
            .and_then(std::sync::Weak::upgrade)
            .ok_or(ValueError::ExpiredCapability(DISPLAY_MAPPER))?;
        let original = owner.values()?;
        if !std::ptr::eq(original.as_ref(), values) {
            return Err(ValueError::ExpiredCapability(DISPLAY_MAPPER));
        }
        Ok(owner)
    }
    fn values(&self) -> Result<Arc<SessionValues>> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ValueError::ExpiredCapability(DISPLAY_MAPPER));
        }
        self.values.upgrade().ok_or(ValueError::ExpiredCapability(DISPLAY_MAPPER))
    }
    fn register(&self, callback: Callback) -> Result<u64> {
        self.values()?;
        let id = NEXT_CALLBACK
            .fetch_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |id| (id < 9_007_199_254_740_991).then_some(id + 1),
            )
            .map_err(|_| ValueError::UnsupportedOperation("display callback identities exhausted"))?;
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).insert(id, callback);
        Ok(id)
    }
    fn id(&self, receiver: &ValueHandle) -> Result<u64> {
        let values = self.values()?;
        values.check_handle(receiver)?;
        match values.heap().snapshot(receiver)? {
            ValueSnapshot::Number(n) if n.is_finite() && n >= 1.0 && n.fract() == 0.0 => Ok(n as u64),
            _ => Err(ValueError::ExpiredCapability(DISPLAY_RENDER)),
        }
    }
    fn entry(&self, id: u64) -> Result<Callback> {
        self.values()?;
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
            .ok_or(ValueError::ExpiredCapability(DISPLAY_RENDER))
    }
    fn mapper(
        &self,
        shared: SharedState,
        theme: Arc<Theme>,
    ) -> Result<(ValueHandle, Arc<std::sync::Mutex<Vec<ChildRecord>>>)> {
        let children = Arc::new(std::sync::Mutex::new(Vec::new()));
        let id = self.register(Callback::Mapper { shared, theme, children: children.clone(),
        })?;
        let values = self.values()?;
        let token = values.heap().number(id as f64)?;
        Ok((values.heap().fresh_compiled_function(&MAP_FILE_COMPONENT, &[token])?, children,
        ))
    }
    pub(crate) fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::Release);
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.environment.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.sanitizer.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.device_catalog.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.theme_values.lock().unwrap_or_else(|e|e.into_inner()).clear();
        self.theme_languages.lock().unwrap_or_else(|e|e.into_inner()).take();
        self.github_renderer.lock().unwrap_or_else(|e|e.into_inner()).take();
        self.github_imports.lock().unwrap_or_else(|e|e.into_inner()).take();
        self.github_expand_hint.lock().unwrap_or_else(|e|e.into_inner()).take();
        self.theme_spinner_values.lock().unwrap_or_else(|e|e.into_inner()).clear();
        self.facts_cache.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
    pub(crate) fn resolve_call(
        &self,
        cap: ara_rpc::value_heap::CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Option<Result<ara_rpc::value_heap::RuntimeReply>> {
        if !callback_claims(cap) {
            return None;
        }
        Some((|| {
            let values = self.values()?;
            let id = self.id(receiver)?;
            let entry = self.entry(id)?;
            let result = match (cap, entry) {
                (DISPLAY_GITHUB,Callback::GithubMethod{is_result})=>{
                    let graph=DisplayGraph(values.as_ref(),effective);
                    let built=super::github_renderer::construct(&graph,self.github_imports(&graph)?,is_result,arguments)?;
                    let framed=built.is_framed();
                    let id=self.register(Callback::GithubComponent{component:Arc::new(std::sync::Mutex::new(built)),
                    })?;
                    let component=values.heap().object()?;let token=values.heap().number(id as f64)?;
                    for(name,spec)in [("render",&RENDER_FILE_COMPONENT),("invalidate",&INVALIDATE_FILE_COMPONENT),
                    ]{
                        values.heap().define_own_data(&component,name.into(),values.heap().fresh_compiled_function(spec,&[token.clone()])?,effective,
                        )?;
                    }
                    if framed{
                        let marker=values.heap().module_symbol("OMP596f2da tui/output-block.ts","framedBlockComponent")?;
                        values.heap().define_own_data(&component,marker,values.heap().boolean(true)?,effective)?;
                    }
                    component
                }
                (DISPLAY_RENDER,Callback::GithubComponent{component})=>{
                    let graph=DisplayGraph(values.as_ref(),effective);
                    let mut local=component.lock().unwrap_or_else(|e|e.into_inner()).clone();
                    let rows=local.render(&graph,arguments.first().cloned().unwrap_or(values.heap().undefined()?))?;
                    *component.lock().unwrap_or_else(|e|e.into_inner())=local;rows
                }
                (DISPLAY_INVALIDATE,Callback::GithubComponent{component})=>{
                    component.lock().unwrap_or_else(|e|e.into_inner()).invalidate();values.heap().undefined()?
                }
                (DISPLAY_THEME,Callback::Theme{..})=>{
                    let graph=DisplayGraph(values.as_ref(),effective);
                    let operation=graph.string(&arguments.first().cloned().unwrap_or(values.heap().undefined()?))?;
                    let receiver=arguments.get(1).cloned().unwrap_or(values.heap().undefined()?);
                    let first=arguments.get(2).cloned().unwrap_or(values.heap().undefined()?);
                    let second=arguments.get(3).cloned().unwrap_or(values.heap().undefined()?);
                    let heap=values.heap();
                    let key=|value:&ValueHandle|->Result<PropertyKey>{
                        let primitive=heap.to_primitive(value,true,effective)?;
                        if heap.kind(&primitive)?==ValueKind::Symbol {
                            Ok(PropertyKey::Symbol(heap.symbol_key(&primitive)?))
                        }else{Ok(heap.to_string(&primitive,effective)?.into())}
                    };
                    let string=|value:&ValueHandle|->Result<WireString>{
                        if heap.kind(value)?==ValueKind::Symbol {
                            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into(),
                            ));
                        }
                        heap.to_string(value,effective)
                    };
                    let lookup=|key:&PropertyKey,value:Option<&str>|->Result<ValueHandle>{
                        if let Some(value)=value {heap.string(value.into())}
                        else{heap.get(&heap.object_prototype(),key,effective)}
                    };
                    let own_key=|key:&PropertyKey|match key {
                        PropertyKey::String(value)=>value.to_utf8().ok(),PropertyKey::Symbol(_)=>None,
                    };
                    let symbol=|theme:&Theme,key:&PropertyKey|->Result<ValueHandle>{
                        let name=own_key(key);lookup(key,name.as_deref().and_then(|name|theme.symbol_value(name)))
                    };
                    let result=match operation.to_utf8().map_err(|_|ValueError::WrongRuntimeReply("theme operation"))?.as_str(){
                        operation @ ("fg"|"bg")=>{
                            // Private brand is checked before property-key coercion.
                            let theme=self.theme_receiver(&receiver)?;let property=key(&first)?;
                            let name=own_key(&property);
                            let ansi=lookup(&property,name.as_deref().and_then(|name| {
                                    if operation=="fg"{theme.fg_value(name)}else{theme.bg_value(name)}
                                }),
                            )?;
                            if !graph.truth(&ansi)? {
                                // Source Gets Error before reevaluating the original
                                // color in the error template (a second conversion).
                                let error=graph.get(&self.environment()?,"Error")?;
                                let prefix=if operation=="fg"{"Unknown theme color: "}else{"Unknown theme background color: "};
                                let mut message=WireString::from(prefix).units().to_vec();message.extend_from_slice(string(&first)?.units());
                                return Err(ValueError::Thrown(heap.construct(&error,&[heap.string(WireString::from_units(message))?],effective,
                                )?));
                            }
                            let mut text=string(&ansi)?.units().to_vec();text.extend_from_slice(string(&second)?.units());
                            text.extend_from_slice(WireString::from(if operation=="fg"{"\x1b[39m"}else{"\x1b[49m"}).units(),
                            );
                            heap.string(WireString::from_units(text))?
                        }
                        "symbol"=>{let theme=self.theme_receiver(&receiver)?;symbol(&theme,&key(&first)?)?}
                        "styledSymbol"=>{
                            let fg=graph.get(&receiver,"fg")?;
                            let theme=self.theme_receiver(&receiver)?;
                            let symbol=symbol(&theme,&key(&first)?)?;
                            heap.call(&fg,&receiver,&[second,symbol],effective)?
                        }
                        "getLangIcon"=>{
                            let language_key=if graph.truth(&first)? {
                                let lower=graph.get(&first,"toLowerCase")?;
                                let normalized=heap.call(&lower,&first,&[],effective)?;
                                let resolved=heap.get(&self.theme_languages(&graph)?,&key(&normalized)?,effective)?;
                                if graph.truth(&resolved)? {resolved}else{heap.string("lang.default".into())?}
                            }else{heap.string("lang.default".into())?};
                            let theme=self.theme_receiver(&receiver)?;symbol(&theme,&key(&language_key)?)?
                        }
                        "spinnerFrames"=>{
                            let method=graph.get(&receiver,"getSpinnerFrames")?;
                            heap.call(&method,&receiver,&[],effective)?
                        }
                        "getSpinnerFrames"=>{
                            let theme=self.theme_receiver(&receiver)?;
                            if !matches!(heap.snapshot(&first)?,ValueSnapshot::Undefined)&&!matches!(heap.snapshot(&first)?,ValueSnapshot::String(name)if name.equals_ascii("status")){
                                return Err(ValueError::UnsupportedOperation("Github Theme source uses status spinner only",
                                ));
                            }
                            let key=(theme.identity,theme.revision);
                            let existing=self.theme_spinner_values.lock().unwrap_or_else(|e|e.into_inner()).get(&key).cloned();
                            if let Some(value)=existing{value}else{
                                let value=heap.array(theme.spinner_frames.iter().map(|frame|heap.string(frame.as_str().into())).collect::<Result<Vec<_>>>()?,
                                )?;
                                self.theme_spinner_values.lock().unwrap_or_else(|e|e.into_inner()).insert(key,value.clone());value
                            }
                        }
                        group @ ("status"|"sep"|"format"|"boxRound")=>{
                            let theme=self.theme_receiver(&receiver)?;
                            let object=heap.object()?;
                            let keys:&[&str]=match group{
                                "status"=>&["success","error","warning","info","pending","disabled","enabled","running","shadowed","aborted","done",
                                ],
                                "sep"=>&["powerline","powerlineThin","powerlineLeft","powerlineRight","powerlineThinLeft","powerlineThinRight","powerlineCapLeft","block","space","asciiLeft","asciiRight","dot","slash","pipe",
                                ],
                                "format"=>&["bullet","dash","bracketLeft","bracketRight"],
                                _=>&["topLeft","topRight","bottomLeft","bottomRight","horizontal","vertical","cross","teeDown","teeUp","teeRight","teeLeft",
                                ],
                            };
                            for name in keys{
                                let source_group=if group=="boxRound"&&matches!(*name,"cross"|"teeDown"|"teeUp"|"teeRight"|"teeLeft"){"boxSharp"}else{group};
                                let key:PropertyKey=format!("{source_group}.{name}").as_str().into();
                                let value=symbol(&theme,&key)?;
                                heap.define_own_data(&object,(*name).into(),value,effective)?;
                            }
                            object
                        }
                        _=>return Err(ValueError::WrongRuntimeReply("unowned theme callback operation")),
                    };
                    result
                }
                (DISPLAY_MAPPER, Callback::Mapper { shared, theme, children,
                    },
                ) => {
                    let file = arguments.first().cloned().unwrap_or(values.heap().undefined()?);
                    let graph = DisplayGraph(values.as_ref(), effective);
                    let error = graph.truth(&graph.get(&file, "isError")?)?;
                    let state = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let renderer = GraphEditRenderer::single(
                        &graph,
                        Some(file),
                        None,
                        &EditRenderArgs::default(),
                        error,
                        Vec::new(),
                        state.options,
                        state.context,
                        theme,
                    )?;
                    let child_id = self.register(Callback::Child {
                        shared,
                        renderer: Arc::new(std::sync::Mutex::new(renderer.clone())),
                    })?;
                    let component = values.heap().object()?;
                    let token = values.heap().number(child_id as f64)?;
                    for (key, spec) in [("render", &RENDER_FILE_COMPONENT), ("invalidate", &INVALIDATE_FILE_COMPONENT),
                    ]
                    {
                        let method = values.heap().fresh_compiled_function(spec, &[token.clone()])?;
                        values.heap().define_own_data(&component, key.into(), method, effective)?;
                    }
                    children.lock().unwrap_or_else(|e| e.into_inner()).push((component.clone(), child_id, renderer));
                    component
                }
                (DISPLAY_RENDER, Callback::Child { shared, renderer }) => {
                    let state = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let width = values
                        .heap()
                        .to_number(&arguments.first().cloned().unwrap_or(values.heap().undefined()?), effective,
                    )?;
                    let mut scratch = renderer.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let graph = DisplayGraph(values.as_ref(), effective);
                    let rows =
                        scratch.render(&graph, width.max(0.0) as usize, &state.frame, state.options, &state.context,
                    );
                    *renderer.lock().unwrap_or_else(|e| e.into_inner()) = scratch;
                    rows_to_value(values.heap(), rows?)?
                }
                (DISPLAY_INVALIDATE, Callback::Child { renderer, .. }) => {
                    renderer.lock().unwrap_or_else(|e| e.into_inner()).invalidate();
                    values.heap().undefined()?
                }
                _ => return Err(ValueError::ExpiredCapability(cap)),
            };
            Ok(ara_rpc::value_heap::RuntimeReply::Value(result))
        })())
    }
}
fn rows_to_value(heap: &Arc<ara_rpc::value_heap::ValueHeap>, rows: Vec<String>) -> Result<ValueHandle> {
    let array = heap.array_with_length(u32::try_from(rows.len()).map_err(|_| ValueError::Allocation)?)?;
    for (i, row) in rows.into_iter().enumerate() {
        heap.define_own_data(
            &array,
            WireString::from(i.to_string()).into(),
            heap.string(row.into())?,
            &ara_rpc::value_heap::NoRuntime,
        )?;
    }
    Ok(array)
}
fn callback_reply(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    cap: ara_rpc::value_heap::CapabilityId,
    captures: &[ValueHandle],
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    match runtime.resolve_call(cap, captures.first().ok_or(ValueError::ExpiredCapability(cap))?, args, runtime,
    )? {
        ara_rpc::value_heap::RuntimeReply::Value(value) => {
            heap.kind(&value)?;
            Ok(value)
        }
        _ => Err(ValueError::WrongRuntimeReply("display callback value")),
    }
}
// Fixed theme-class.ts langMap, retained in the original realm.
const THEME_LANGUAGE_KEYS:&[(&str,&str)]=&[
    ("typescript","lang.typescript"),
    ("ts","lang.typescript"),
    ("tsx","lang.typescript"),
    ("javascript","lang.javascript"),
    ("js","lang.javascript"),
    ("jsx","lang.javascript"),
    ("mjs","lang.javascript"),
    ("cjs","lang.javascript"),
    ("python","lang.python"),
    ("py","lang.python"),
    ("rust","lang.rust"),
    ("rs","lang.rust"),
    ("go","lang.go"),
    ("java","lang.java"),
    ("c","lang.c"),
    ("cpp","lang.cpp"),
    ("c++","lang.cpp"),
    ("cc","lang.cpp"),
    ("cxx","lang.cpp"),
    ("csharp","lang.csharp"),
    ("cs","lang.csharp"),
    ("ruby","lang.ruby"),
    ("rb","lang.ruby"),
    ("julia","lang.julia"),
    ("jl","lang.julia"),
    ("php","lang.php"),
    ("swift","lang.swift"),
    ("kotlin","lang.kotlin"),
    ("kt","lang.kotlin"),
    ("bash","lang.shell"),
    ("sh","lang.shell"),
    ("zsh","lang.shell"),
    ("fish","lang.shell"),
    ("powershell","lang.shell"),
    ("just","lang.shell"),
    ("shell","lang.shell"),
    ("html","lang.html"),
    ("htm","lang.html"),
    ("astro","lang.html"),
    ("vue","lang.html"),
    ("svelte","lang.html"),
    ("css","lang.css"),
    ("scss","lang.css"),
    ("sass","lang.css"),
    ("less","lang.css"),
    ("json","lang.json"),
    ("yaml","lang.yaml"),
    ("yml","lang.yaml"),
    ("markdown","lang.markdown"),
    ("md","lang.markdown"),
    ("sql","lang.sql"),
    ("dockerfile","lang.docker"),
    ("docker","lang.docker"),
    ("lua","lang.lua"),
    ("text","lang.text"),
    ("txt","lang.text"),
    ("plain","lang.text"),
    ("log","lang.log"),
    ("env","lang.env"),
    ("dotenv","lang.env"),
    ("toml","lang.toml"),
    ("xml","lang.xml"),
    ("ini","lang.ini"),
    ("conf","lang.conf"),
    ("cfg","lang.conf"),
    ("config","lang.conf"),
    ("properties","lang.conf"),
    ("csv","lang.csv"),
    ("tsv","lang.tsv"),
    ("image","lang.image"),
    ("img","lang.image"),
    ("png","lang.image"),
    ("jpg","lang.image"),
    ("jpeg","lang.image"),
    ("gif","lang.image"),
    ("webp","lang.image"),
    ("svg","lang.image"),
    ("ico","lang.image"),
    ("bmp","lang.image"),
    ("tiff","lang.image"),
    ("pdf","lang.pdf"),
    ("zip","lang.archive"),
    ("tar","lang.archive"),
    ("gz","lang.archive"),
    ("tgz","lang.archive"),
    ("bz2","lang.archive"),
    ("xz","lang.archive"),
    ("7z","lang.archive"),
    ("exe","lang.binary"),
    ("dll","lang.binary"),
    ("so","lang.binary"),
    ("dylib","lang.binary"),
    ("wasm","lang.binary"),
    ("bin","lang.binary"),
];
macro_rules! theme_method {
    ($spec:ident,$name:literal,$length:expr)=>{
        static $spec:CompiledFunctionSpec=CompiledFunctionSpec{
            source:concat!("OMP596f2da original Theme.",$name," consumer via existing display callback owner"),name:$name,length:$length,form:CompiledFunctionForm::Builtin,capture_count:1,
            body:|heap,captures,receiver,args,runtime|{let mut operands=vec![heap.string($name.into())?,receiver.clone()];operands.extend_from_slice(args);callback_reply(heap,DISPLAY_THEME,captures,&operands,runtime)},
        };
    };
}
theme_method!(THEME_STATUS_GET,"status",0);theme_method!(THEME_SEP_GET,"sep",0);theme_method!(THEME_FORMAT_GET,"format",0);theme_method!(THEME_BOX_GET,"boxRound",0);theme_method!(THEME_SPINNER_GET,"spinnerFrames",0);theme_method!(THEME_SPINNER_METHOD,"getSpinnerFrames",1);
static GITHUB_CALL:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da githubToolRenderer.renderCall",name:"renderCall",length:3,form:CompiledFunctionForm::Ordinary,capture_count:1,body:|heap,captures,_,args,rt|callback_reply(heap,DISPLAY_GITHUB,captures,args,rt),
};
static GITHUB_RESULT:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP596f2da githubToolRenderer.renderResult",name:"renderResult",length:4,form:CompiledFunctionForm::Ordinary,capture_count:1,body:|heap,captures,_,args,rt|callback_reply(heap,DISPLAY_GITHUB,captures,args,rt),
};
theme_method!(THEME_FG,"fg",2);theme_method!(THEME_BG,"bg",2);theme_method!(THEME_SYMBOL,"symbol",1);theme_method!(THEME_STYLED_SYMBOL,"styledSymbol",2);theme_method!(THEME_LANG,"getLangIcon",1);
static MAP_FILE_COMPONENT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da renderer1141: fileResult => renderSingleFileResult({ content: [], details: fileResult, isError: fileResult.isError }, options, uiTheme)",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| callback_reply(heap, DISPLAY_MAPPER, captures, args, runtime),
};
static RENDER_FILE_COMPONENT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retained single framed/inline component render(width)",
    name: "render",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| callback_reply(heap, DISPLAY_RENDER, captures, args, runtime),
};
static INVALIDATE_FILE_COMPONENT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retained single component invalidate()",
    name: "invalidate",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| callback_reply(heap, DISPLAY_INVALIDATE, captures, args, runtime),
};

/// All retained child callbacks reached in one frame share lazy transactional
/// cache scratch. No global child cache changes before driver publication.
pub(crate) struct FrameCallbacks<'a> {
    domain: Arc<CallbackDomain>,
    base: &'a dyn ValueRuntime,
    scratch: std::sync::Mutex<std::collections::HashMap<u64, GraphEditRenderer>>,
    github_scratch:std::sync::Mutex<std::collections::HashMap<u64,super::github_renderer::Component>>,
}
impl<'a> FrameCallbacks<'a> {
    pub(crate) fn new(domain: Arc<CallbackDomain>, base: &'a dyn ValueRuntime) -> Self {
        Self { domain, base, scratch: std::sync::Mutex::new(std::collections::HashMap::new()),github_scratch:std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
    pub(crate) fn commit(&self) {
        for(id,local)in self.github_scratch.lock().unwrap_or_else(|e|e.into_inner()).iter(){
            if let Ok(Callback::GithubComponent{component})=self.domain.entry(*id){*component.lock().unwrap_or_else(|e|e.into_inner())=local.clone();}
        }
        let scratch = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
        for (id, graph) in scratch.iter() {
            if let Ok(Callback::Child { renderer, .. }) = self.domain.entry(*id) {
                *renderer.lock().unwrap_or_else(|e| e.into_inner()) = graph.clone();
            }
        }
    }
}
impl ValueRuntime for FrameCallbacks<'_> {
    fn resolve(&self, request: ara_rpc::value_heap::RuntimeRequest<'_>) -> Result<ara_rpc::value_heap::RuntimeReply> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(
        &self,
        request: ara_rpc::value_heap::RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Result<ara_rpc::value_heap::RuntimeReply> {
        match request {
            ara_rpc::value_heap::RuntimeRequest::Call { capability, receiver, arguments,
            } => self.resolve_call(capability, receiver, arguments, effective),
            request => self.base.resolve_effective(request, effective),
        }
    }
    fn resolve_call(
        &self,
        cap: ara_rpc::value_heap::CapabilityId,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ara_rpc::value_heap::RuntimeReply> {
        if cap != DISPLAY_RENDER {
            return self.base.resolve_call(cap, receiver, args, effective);
        }
        let values = self.domain.values()?;
        let id = self.domain.id(receiver)?;
        if let Callback::GithubComponent{component}=self.domain.entry(id)?{
            let mut local={let mut scratch=self.github_scratch.lock().unwrap_or_else(|e|e.into_inner());scratch.entry(id).or_insert_with(||component.lock().unwrap_or_else(|e|e.into_inner()).clone()).clone()};
            let rows=local.render(&DisplayGraph(values.as_ref(),effective),args.first().cloned().unwrap_or(values.heap().undefined()?),
            )?;
            self.github_scratch.lock().unwrap_or_else(|e|e.into_inner()).insert(id,local);
            return Ok(ara_rpc::value_heap::RuntimeReply::Value(rows));
        }
        let Callback::Child { shared, renderer } = self.domain.entry(id)? else {
            return Err(ValueError::ExpiredCapability(cap));
        };
        let state = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let width = values.heap().to_number(&args.first().cloned().unwrap_or(values.heap().undefined()?), effective)?;
        let mut local = {
            let mut scratch = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
            scratch.entry(id).or_insert_with(|| renderer.lock().unwrap_or_else(|e| e.into_inner()).clone()).clone()
        };
        let rows = local.render(
            &DisplayGraph(values.as_ref(), effective),
            width.max(0.0) as usize,
            &state.frame,
            state.options,
            &state.context,
        );
        self.scratch.lock().unwrap_or_else(|e| e.into_inner()).insert(id, local);
        Ok(ara_rpc::value_heap::RuntimeReply::Value(rows_to_value(values.heap(), rows?,
        )?))
    }
    fn resolve_is_constructor(
        &self,
        cap: ara_rpc::value_heap::CapabilityId,
        constructor: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        self.base.resolve_is_constructor(cap, constructor, effective)
    }
    fn resolve_construct(
        &self,
        cap: ara_rpc::value_heap::CapabilityId,
        constructor: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.base.resolve_construct(cap, constructor, args, effective)
    }
    fn resolve_construct_with_new_target(
        &self,
        cap: ara_rpc::value_heap::CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.base.resolve_construct_with_new_target(cap, constructor, new_target, args, effective)
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ara_rpc::value_heap::RuntimeReply> {
        self.base.resolve_local_call(token, receiver, args, effective)
    }
}

pub(crate) fn session_values_for_heap(heap:&Arc<ara_rpc::value_heap::ValueHeap>)->Result<Arc<SessionValues>>{
    let domain=CALLBACK_OWNERS.get_or_init(Default::default).lock().unwrap_or_else(|e|e.into_inner()).get(&heap.heap_token()).and_then(std::sync::Weak::upgrade).ok_or(ValueError::ExpiredCapability(DISPLAY_GITHUB))?;
    let values=domain.values()?;
    if !Arc::ptr_eq(heap,values.heap()){return Err(ValueError::ForeignHeap);
    }Ok(values)
}
pub(crate) struct DisplayGraph<'a>(pub &'a SessionValues, pub &'a dyn ValueRuntime);
impl<'a> DisplayGraph<'a> {
    pub(crate) fn mounted_tool(&self,name:&WireString)->Result<Option<ValueHandle>> {
        let domain=CallbackDomain::for_values(self.0)?;
        let locator=domain.device_catalog.lock().unwrap_or_else(|e|e.into_inner()).clone();
        let Some(locator)=locator else {return Ok(None)};
        let catalog=locator.upgrade().ok_or(ValueError::UnsupportedOperation("display canonical catalog expired"))?;
        let result=(||{let snapshot=catalog.execution_snapshot(self.0)?;snapshot.device_member_in(name,true,self.0,self.1).map(|member|member.map(|member|member.binding.source))})();
        result.map_err(|error|match error {ara_agent::ActualProviderError::Value(error)=>error,_=>ValueError::UnsupportedOperation("display canonical catalog source unavailable"),
        })
    }
    pub(crate) fn github_match_initially_missing(&self)->Result<bool>{Ok(CallbackDomain::for_values(self.0)?.github_match_initially_missing)}
    pub(crate) fn github_renderer(&self)->Result<ValueHandle>{CallbackDomain::for_values(self.0)?.github_renderer(self)}
    pub(crate) fn github_expand_key_hint(&self)->Result<ValueHandle>{
        let domain=CallbackDomain::for_values(self.0)?;
        let callback=domain.github_expand_hint.lock().unwrap_or_else(|e|e.into_inner()).clone();
        match callback{Some(callback)=>self.0.heap().call(&callback,&self.0.heap().undefined()?,&[],self.runtime()),None=>self.0.heap().string("Ctrl+O".into()),
        }
    }
    pub(crate) fn device_theme(&self,theme:&Arc<Theme>)->Result<ValueHandle>{
        CallbackDomain::for_values(self.0)?.theme_value(theme,self.1)
    }
    pub fn base(values: &'a SessionValues) -> Self {
        Self(values, values.runtime())
    }
    pub fn runtime(&self) -> &dyn ValueRuntime {
        self.1
    }
    pub fn get(&self, root: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.0.check_handle(root)?;
        self.0.heap().get(root, &key.into(), self.1)
    }
    pub fn string(&self, v: &ValueHandle) -> Result<WireString> {
        match self.0.heap().snapshot(v)? {
            ValueSnapshot::String(s) => Ok(s),
            _ => Err(ValueError::TypeError("display string required".into())),
        }
    }
    pub fn nullish(&self, v: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.0.heap().snapshot(v)?, ValueSnapshot::Undefined | ValueSnapshot::Null))
    }
    pub fn is(&self, r: &ValueHandle, k: &str, s: &str) -> Result<bool> {
        Ok(matches!(self.0.heap().snapshot(&self.get(r,k)?)?,ValueSnapshot::String(v) if v.equals_ascii(s)))
    }
    pub fn members(&self, v: &ValueHandle) -> Result<Vec<ValueHandle>> {
        let mut i = self.0.heap().get_iterator(v, self.1)?;
        let mut out = Vec::new();
        loop {
            match self.0.heap().iterator_next(&mut i, self.1) {
                Ok(Some(v)) => out.push(v),
                Ok(None) => return Ok(out),
                Err(e) => {
                    let _ = self.0.heap().iterator_close(&mut i, self.1);
                    return Err(e);
                }
            }
        }
    }
    pub(crate) fn truth(&self, v: &ValueHandle) -> Result<bool> {
        self.0.heap().truthy(v)
    }
    pub(crate) fn optional(&self, v: &ValueHandle) -> Result<Option<WireString>> {
        Ok(match self.0.heap().snapshot(v)? {
            ValueSnapshot::String(s) => Some(s),
            _ => None,
        })
    }
    // typeof guard and selected value are distinct source observations.
    fn typed(&self, r: &ValueHandle, k: &str) -> Result<Option<ValueHandle>> {
        if self.optional(&self.get(r, k)?)?.is_some() { Ok(Some(self.get(r, k)?)) } else { Ok(None) }
    }
    fn wire(&self, selected: &Option<ValueHandle>) -> Result<Option<WireString>> {
        selected.as_ref().map(|v| self.optional(v)).transpose().map(Option::flatten)
    }
    fn nullish_selected(&self, selected: &Option<ValueHandle>) -> Result<bool> {
        selected.as_ref().map(|v| self.nullish(v)).transpose().map(|v| v.unwrap_or(true))
    }
    fn truth_selected(&self, selected: &Option<ValueHandle>) -> Result<bool> {
        selected.as_ref().map(|v| self.truth(v)).transpose().map(|v| v.unwrap_or(false))
    }
    fn optional_entry(&self, entry: Option<&ValueHandle>, key: &str) -> Result<Option<WireString>> {
        match entry {
            Some(v) if !self.nullish(v)? => self.entry(v, key),
            _ => Ok(None),
        }
    }
    fn optional_operation(&self, entry: Option<&ValueHandle>) -> Result<Option<Operation>> {
        match entry {
            Some(v) if !self.nullish(v)? => self.operation(&self.get(v, "op")?),
            _ => Ok(None),
        }
    }
    pub(crate) fn present(&self, r: &ValueHandle, k: &str) -> Result<Option<WireString>> {
        if self.0.heap().has(r, &k.into(), self.1)? { self.optional(&self.get(r, k)?) } else { Ok(None) }
    }
    fn present_typed(&self, r: &ValueHandle, k: &str) -> Result<Option<ValueHandle>> {
        if self.0.heap().has(r, &k.into(), self.1)? { self.typed(r, k) } else { Ok(None) }
    }
    fn input(&self, r: &ValueHandle) -> Result<Option<WireString>> {
        let v = self.get(r, "input")?;
        if self.nullish(&v)? { self.optional(&self.get(r, "_input")?) } else { self.optional(&v) }
    }
    pub(crate) fn global(&self, key: &str) -> Result<ValueHandle> {
        let environment = CallbackDomain::for_values(self.0)?.environment()?;
        self.get(&environment, key)
    }
    pub(crate) fn method(&self, receiver: &ValueHandle, key: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let function = self.get(receiver, key)?;
        self.0.heap().call(&function, receiver, args, self.1)
    }
    fn caught_message(&self, error: ValueError) -> Result<ValueHandle> {
        match error {
            ValueError::Thrown(reason) => {
                let constructor = self.global("Error")?;
                if self.0.heap().instance_of(&reason, &constructor, self.1)? {
                    self.get(&reason, "message")
                } else {
                    let string = self.global("String")?;
                    self.0.heap().call(&string, &self.0.heap().undefined()?, &[reason], self.1)
                }
            }
            // Plain stringify exposes these native-origin ECMAScript errors
            // as a Rust carrier; the original Error.message is the same text.
            ValueError::TypeError(message) | ValueError::SyntaxError(message) | ValueError::RangeError(message) => {
                self.0.heap().string(message.into())
            }
            other => Err(other),
        }
    }
    fn input_value(&self, r: &ValueHandle) -> Result<ValueHandle> {
        let value = self.get(r, "input")?;
        if self.nullish(&value)? {
            self.get(r, "_input")
        } else {
            Ok(value)
        }
    }
    fn inspected_entries(
        &self,
        root: &ValueHandle,
        mode: ara_edit::EditMode,
        input: &ValueHandle,
    ) -> Result<Vec<edit_renderer::InputEntry>> {
        let heap = self.0.heap();
        let key = heap.module_symbol("terminal.edit", "edit.inspectedInput")?;
        let cached = heap.get(root, &key, self.1)?;
        let same = |left: &ValueHandle, right: &ValueHandle| -> Result<bool> {
            match (heap.snapshot(left)?, heap.snapshot(right)?) {
                (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => Ok(a == b),
                _ => heap.object_is(left, right),
            }
        };
        let decode = |entries: &ValueHandle| -> Result<Vec<edit_renderer::InputEntry>> {
            let mut out = vec![];
            for entry in self.members(entries)? {
                out.push(edit_renderer::InputEntry {
                    path: self
                        .optional(&self.get(&entry, "path")?)?
                        .map(|s| super::render_utils::sanitize(&s))
                        .unwrap_or_default(),
                    op: self.operation(&self.get(&entry, "op")?)?,
                    rename: self
                        .optional(&self.get(&entry, "rename")?)?
                        .map(|s| super::render_utils::sanitize(&s)),
                    line_edits: false,
                });
            }
            Ok(out)
        };
        if self.truth(&cached)?
            && self.is(&cached, "mode", mode.as_str())?
            && same(&self.get(&cached, "input")?, input)?
        {
            let entries = self.get(&cached, "entries")?;
            if self.truth(&entries)? {
                return decode(&entries);
            }
            return Err(ValueError::Thrown(self.get(&cached, "error")?));
        }
        let inspected = (|| -> Result<ara_edit::Inspection> {
            let wrapper = heap.object()?;
            heap.define_own_data(&wrapper, "input".into(), input.clone(), self.1)?;
            let json = heap
                .stringify(&wrapper, StringifyMode::Plain, None, self.1)?
                .ok_or(ValueError::WrongRuntimeReply("Edit renderer inspect JSON.stringify"))?;
            let json = json
                .to_utf8()
                .map_err(|_| ValueError::WrongRuntimeReply("Edit renderer inspect JSON text"))?;
            Ok(ara_tools::edit::inspect_args_json(mode, &json))
        })();
        let cache = heap.object()?;
        heap.define_own_data(&cache, "mode".into(), heap.string(mode.as_str().into())?, self.1)?;
        heap.define_own_data(&cache, "input".into(), input.clone(), self.1)?;
        match inspected {
            Ok(inspection) => {
                let mut entries = Vec::<edit_renderer::InputEntry>::new();
                for path in inspection.paths {
                    if !entries.iter().any(|e| e.path == path) {
                        entries.push(edit_renderer::InputEntry {
                            path,
                            ..Default::default()
                        });
                    }
                }
                for op in inspection.file_ops {
                    let (path, kind, rename) = match op {
                        ara_edit::FileOpIntent::Delete { path } => (path, Operation::Delete, None),
                        ara_edit::FileOpIntent::Move { from, to } => (from, Operation::Update, Some(to)),
                    };
                    if let Some(entry) = entries.iter_mut().find(|e| e.path == path) {
                        entry.op = Some(kind);
                        entry.rename = rename;
                    } else {
                        entries.push(edit_renderer::InputEntry {
                            path,
                            op: Some(kind),
                            rename,
                            ..Default::default()
                        });
                    }
                }
                let array=heap.import_serde(&serde_json::json!(entries.iter().map(|e|serde_json::json!({"path":e.path,"op":e.op.map(|o|match o{Operation::Create=>"create",Operation::Delete=>"delete",Operation::Update=>"update"}),"rename":e.rename})).collect::<Vec<_>>()))?;
                heap.define_own_data(&cache, "entries".into(), array, self.1)?;
                heap.assign(root, key, cache, self.1)?;
                Ok(entries)
            }
            Err(error) => {
                let reason = match error {
                    ValueError::Thrown(reason) => reason,
                    other => {
                        let message = self.caught_message(other)?;
                        let constructor = self.global("Error")?;
                        heap.construct(&constructor, &[message], self.1)?
                    }
                };
                heap.define_own_data(&cache, "error".into(), reason.clone(), self.1)?;
                heap.assign(root, key, cache, self.1)?;
                Err(ValueError::Thrown(reason))
            }
        }
    }
    fn apply_summary(
        &self,
        root: &ValueHandle,
        options: RenderOptions,
        mode: Option<ara_edit::EditMode>,
    ) -> Result<Option<ApplyInputSummary>> {
        if mode.is_some_and(|mode| mode != ara_edit::EditMode::ApplyPatch)
            || self.optional(&self.get(root, "input")?)?.is_none()
        {
            return Ok(None);
        }
        // The typeof Get is outside the source try; the selected second Get,
        // fresh {input} wrapper and stringify are inside it, in that order.
        let inspected = (|| {
            let selected = self.get(root, "input")?;
            self.inspected_entries(root, ara_edit::EditMode::ApplyPatch, &selected)
        })();
        match inspected {
            Ok(entries) => Ok(Some(ApplyInputSummary { entries, error: None })),
            Err(error) => {
                let message = self.caught_message(error)?;
                let missing_end = matches!(self.0.heap().snapshot(&message)?, ValueSnapshot::String(text)
                    if text.equals_ascii("The last line of the patch must be '*** End Patch'"));
                Ok(Some(ApplyInputSummary {
                    entries: Vec::new(),
                    error: if options.is_partial && missing_end { None } else { Some(message) },
                }))
            }
        }
    }

    pub(crate) fn sanitize_value(&self, value: &ValueHandle) -> Result<ValueHandle> {
        let domain = CallbackDomain::for_values(self.0)?;
        let ready = domain.sanitizer.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let function = if let Some(function) = ready {
            function
        } else {
            let bun = self.global("Bun")?;
            let sanitizer = ara_agent::ActualToolResultSanitizer::new(bun, self.0).map_err(|error| match error {
                ara_agent::ActualProviderError::Value(error) => error,
                _ => ValueError::UnsupportedOperation("EDIT actual sanitizer preparation failed"),
            })?;
            let function = sanitizer.function().clone();
            *domain.sanitizer.lock().unwrap_or_else(|e| e.into_inner()) = Some(function.clone());
            function
        };
        self.0.heap().call(&function, &self.0.heap().undefined()?, &[value.clone()], self.1)
    }
    pub(crate) fn replace_tabs_value(&self, value: &ValueHandle) -> Result<ValueHandle> {
        self.method(value, "replaceAll", &[self.0.heap().string("\t".into())?, self.0.heap().string("   ".into())?],
        )
    }
    pub(crate) fn plain_preview_value(&self, value: &ValueHandle, theme: &Theme) -> Result<String> {
        let sanitized = self.sanitize_value(value)?;
        let lines = self.method(&sanitized, "split", &[self.0.heap().string("\n".into())?])?;
        let visible = self.method(&lines, "slice", &[self.0.heap().number(0.0)?, self.0.heap().number(6.0)?],
        )?;
        let mut preview = "\n\n".to_owned();
        for line in self.members(&visible)? {
            let text = self.string(&self.replace_tabs_value(&line)?)?;
            preview.push_str(
                &theme.fg("toolOutput", &super::render_utils::truncate(&super::render_utils::sanitize(&text), 80),
            ));
            preview.push('\n');
        }
        let length = self.0.heap().to_number(&self.get(&lines, "length")?, self.1)?;
        if length > 6.0 {
            preview.push_str(&theme.fg("dim", &format!("… {} more lines", length - 6.0)));
        }
        Ok(preview.trim_end().to_owned())
    }
    pub(crate) fn streaming_tail_value(
        &self,
        diff: &ValueHandle,
        inner: usize,
        budget: usize,
    ) -> Result<(ValueHandle, bool)> {
        let mut end = self.get(diff, "length")?;
        loop {
            let n = self.0.heap().to_number(&end, self.1)?;
            if !(n > 0.0) {
                break;
            }
            let code = self.method(diff, "charCodeAt", &[self.0.heap().number(n - 1.0)?])?;
            if !matches!(self.0.heap().snapshot(&code)?,ValueSnapshot::Number(code) if code==10.0) {
                break;
            }
            end = self.0.heap().number(n - 1.0)?;
        }
        if matches!(self.0.heap().snapshot(&end)?,ValueSnapshot::Number(end) if end==0.0) {
            return Ok((self.0.heap().string("".into())?, false));
        }
        let mut start = end.clone();
        let mut cursor = end.clone();
        let mut rows = 0;
        loop {
            let n = self.0.heap().to_number(&cursor, self.1)?;
            if !(n >= 0.0) {
                break;
            }
            let newline = if n > 0.0 {
                self.0.heap().to_number(
                    &self.method(
                        diff,
                        "lastIndexOf",
                        &[self.0.heap().string("\n".into())?, self.0.heap().number(n - 1.0)?],
                    )?,
                    self.1,
                )?
            } else {
                -1.0
            };
            let line_start = self.0.heap().number(newline + 1.0)?;
            let line = self.method(diff, "slice", &[line_start.clone(), cursor.clone()])?;
            let line = self.string(&self.replace_tabs_value(&line)?)?;
            let line_rows = super::render_utils::wrap(&super::render_utils::sanitize(&line), inner).len().max(1);
            if rows > 0 && rows + line_rows > budget.max(1) {
                break;
            }
            rows += line_rows;
            start = line_start;
            if newline < 0.0 {
                break;
            }
            cursor = self.0.heap().number(newline)?;
        }
        let hidden = self.0.heap().to_number(&start, self.1)? > 0.0;
        Ok((self.method(diff, "slice", &[start, end])?, hidden))
    }
    pub(crate) fn diff_stats_value(&self, diff: &ValueHandle) -> Result<super::render_utils::DiffStats> {
        let lines = if self.truth(diff)? {
            self.method(diff, "split", &[self.0.heap().string("\n".into())?])?
        } else {
            self.0.heap().array(Vec::new())?
        };
        let mut stats = super::render_utils::DiffStats::default();
        let mut in_hunk = false;
        for line in self.members(&lines)? {
            let add = self.truth(&self.method(&line, "startsWith", &[self.0.heap().string("+".into())?])?)?;
            let remove = self.truth(&self.method(&line, "startsWith", &[self.0.heap().string("-".into())?])?)?;
            stats.added += usize::from(add);
            stats.removed += usize::from(remove);
            if (add || remove) && !in_hunk {
                stats.hunks += 1;
                in_hunk = true;
            } else if !add && !remove {
                in_hunk = false;
            }
        }
        stats.lines = self.0.heap().to_number(&self.get(&lines, "length")?, self.1)?.max(0.0) as usize;
        Ok(stats)
    }
    pub(crate) fn truncate_diff_value(
        &self,
        diff: &ValueHandle,
        max_hunks: usize,
        max_lines: usize,
    ) -> Result<(ValueHandle, usize, usize)> {
        let lines = if self.truth(diff)? {
            self.method(diff, "split", &[self.0.heap().string("\n".into())?])?
        } else {
            self.0.heap().array(Vec::new())?
        };
        let total = self.diff_stats_value(diff)?;
        let line_count = self.0.heap().to_number(&self.get(&lines, "length")?, self.1)?.max(0.0) as usize;
        if line_count <= max_lines && total.hunks <= max_hunks {
            return Ok((diff.clone(), 0, 0));
        }
        struct Segment {
            lines: Vec<ValueHandle>,
            change: bool,
            ellipsis: bool,
        }
        let mut segments = Vec::<Segment>::new();
        for line in self.members(&lines)? {
            let change = self.truth(&self.method(&line, "startsWith", &[self.0.heap().string("+".into())?])?)?
                || self.truth(&self.method(&line, "startsWith", &[self.0.heap().string("-".into())?])?)?;
            let trim_start = self.method(&line, "trimStart", &[])?;
            let ellipsis = self.truth(&self.method(
                &trim_start,
                "startsWith",
                &[self.0.heap().string("...".into())?],
            )?)? || matches!(self.0.heap().snapshot(&self.get(&self.method(&line,"trim",&[])?,"length")?)?,ValueSnapshot::Number(n) if n==0.0);
            if ellipsis || segments.last().is_none_or(|last| last.change != change || last.ellipsis) {
                segments.push(Segment { lines: vec![line], change: change && !ellipsis, ellipsis,
                });
            } else {
                segments.last_mut().unwrap().lines.push(line);
            }
        }
        let changes: usize = segments.iter().filter(|s| s.change).map(|s| s.lines.len()).sum();
        let mut kept = Vec::<ValueHandle>::new();
        let mut hunks = 0;
        let mut source_lines = 0;
        if changes > max_lines {
            for segment in &segments {
                if kept.len() >= max_lines {
                    break;
                }
                if segment.change {
                    if hunks >= max_hunks {
                        break;
                    }
                    hunks += 1;
                }
                let take = segment.lines.len().min(max_lines - kept.len());
                kept.extend_from_slice(&segment.lines[..take]);
            }
            source_lines = kept.len();
        } else {
            let context_budget = max_lines - changes;
            let context_total: usize = segments.iter().filter(|s| !s.change).map(|s| s.lines.len()).sum();
            if context_total <= context_budget {
                for segment in &segments {
                    if segment.change {
                        if hunks >= max_hunks {
                            break;
                        }
                        hunks += 1;
                    }
                    kept.extend_from_slice(&segment.lines);
                    source_lines += segment.lines.len();
                }
            } else {
                let ratio = context_budget as f64 / context_total as f64;
                let mut remaining = context_budget;
                for (index, segment) in segments.iter().enumerate() {
                    if segment.change {
                        if hunks >= max_hunks {
                            break;
                        }
                        hunks += 1;
                        kept.extend_from_slice(&segment.lines);
                        source_lines += segment.lines.len();
                        continue;
                    }
                    if remaining == 0 {
                        continue;
                    }
                    let allowed = remaining.min(((segment.lines.len() as f64 * ratio).floor() as usize).max(1));
                    let before = kept.len();
                    let added;
                    if segment.ellipsis || segment.lines.len() <= allowed {
                        kept.extend_from_slice(&segment.lines[..allowed]);
                        added = allowed;
                    } else {
                        let before_change = segments.get(index + 1).is_some_and(|s| s.change);
                        let after_change = index > 0 && segments[index - 1].change;
                        if before_change && after_change {
                            let budget = if allowed >= 3 { allowed - 1 } else { allowed };
                            let first = budget.div_ceil(2);
                            let last = budget - first;
                            kept.extend_from_slice(&segment.lines[..first]);
                            if allowed >= 3 {
                                kept.push(self.0.heap().string("".into())?);
                            }
                            if last > 0 {
                                kept.extend_from_slice(&segment.lines[segment.lines.len() - last..]);
                            }
                            added = budget;
                        } else if before_change {
                            kept.extend_from_slice(&segment.lines[segment.lines.len() - allowed..]);
                            added = allowed;
                        } else if after_change {
                            kept.extend_from_slice(&segment.lines[..allowed]);
                            added = allowed;
                        } else {
                            let take = allowed.min(2);
                            kept.extend_from_slice(&segment.lines[..take]);
                            added = take;
                        }
                    }
                    remaining = remaining.saturating_sub(kept.len() - before);
                    source_lines += added;
                }
            }
        }
        let kept = self.0.heap().array(kept)?;
        Ok((
            self.method(&kept, "join", &[self.0.heap().string("\n".into())?])?,
            total.hunks.saturating_sub(hunks),
            line_count.saturating_sub(source_lines),
        ))
    }

    fn operation(&self, v: &ValueHandle) -> Result<Option<Operation>> {
        if !self.truth(v)? {
            return Ok(None);
        }
        Ok(Some(
            self.optional(v)?
                .and_then(|v| v.to_utf8().ok())
                .as_deref()
                .and_then(Operation::parse)
                .unwrap_or(Operation::Update),
        ))
    }
    fn entry(&self, r: &ValueHandle, k: &str) -> Result<Option<WireString>> {
        self.optional(&self.get(r, k)?)
    }
    fn count_edits(&self, a: &ValueHandle) -> Result<usize> {
        let environment = CallbackDomain::for_values(self.0)?.environment()?;
        // new Set evaluates the callee before the map/filter argument expression.
        let constructor = self.get(&environment, "Set")?;
        let map = self.get(a, "map")?;
        let mapper = self.0.heap().fresh_compiled_function(&MAP_EDIT_PATH, &[])?;
        let mapped = self.0.heap().call(&map, a, &[mapper], self.1)?;
        let filter = self.get(&mapped, "filter")?;
        let boolean = self.get(&environment, "Boolean")?;
        let filtered = self.0.heap().call(&filter, &mapped, &[boolean], self.1)?;
        // The original Set consumes the selected filtered result's iterator.
        let set = self.0.heap().construct(&constructor, &[filtered], self.1)?;
        match self.0.heap().snapshot(&self.get(&set, "size")?)? {
            ValueSnapshot::Number(n) if n.is_finite() && n >= 0.0 => Ok(n as usize),
            _ => Err(ValueError::TypeError("Set.size did not produce a number".into())),
        }
    }
}

#[cfg(test)]
pub(crate) fn observe_retained_dynamic_callbacks() {
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Reads {
        count: AtomicUsize,
        throw_at: AtomicUsize,
        reason: ValueHandle,
    }
    impl ValueRuntime for Reads {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(7896), receiver, .. } => {
                    let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
                    if n == self.throw_at.load(Ordering::SeqCst) {
                        return Err(ValueError::Thrown(self.reason.clone()));
                    }
                    Ok(RuntimeReply::Value(
                        receiver.heap().string(format!("@@ -1,1 +1,1 @@\n-1|old\n+1|selected-body-{n}").into(),
                    )?))
                }
                RuntimeRequest::Call { capability: CapabilityId(7897), receiver, .. } => {
                    let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
                    Ok(RuntimeReply::Value(if n % 2 == 1 {
                        receiver.heap().string("guard.ts".into())?
                    } else {
                        receiver.heap().number(7.0)?
                    }))
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    struct CallbackRuntime {
        base: Arc<dyn ValueRuntime>,
        domain: Arc<CallbackDomain>,
    }
    impl ValueRuntime for CallbackRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            self.resolve_effective(request, self)
        }
        fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments,
                } => self.resolve_call(capability, receiver, arguments, effective),
                request => self.base.resolve_effective(request, effective),
            }
        }
        fn resolve_call(
            &self,
            cap: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply> {
            if let Some(result) = self.domain.resolve_call(cap, receiver, args, effective) {
                result
            } else {
                self.base.resolve_call(cap, receiver, args, effective)
            }
        }
    }
    let heap = ValueHeap::new().unwrap();
    let reason = heap.object().unwrap();
    let reads =
        Arc::new(Reads { count: AtomicUsize::new(0), throw_at: AtomicUsize::new(usize::MAX), reason: reason.clone(),
    });
    let values = Arc::new(SessionValues::new(heap.clone(), reads.clone()).unwrap());
    let domain = CallbackDomain::new(&values);
    let globals = crate::native_realm::NativeGlobals::prepare(&heap, values.runtime()).unwrap();
    let _ast = crate::native_realm::ast::NativeAstModule::prepare(&heap, &globals, values.runtime()).unwrap();
    domain.bind_environment(&globals.environment).unwrap();
    let callback_runtime = CallbackRuntime { base: reads.clone(), domain: domain.clone(),
    };
    let graph = DisplayGraph(&values, &callback_runtime);
    let details = heap.import_serde(&serde_json::json!({"path":"initial-path.ts"})).unwrap();
    let trap = |details: &ValueHandle| {
        heap.define_own(
            details,
            "diff".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(7896))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            values.runtime(),
        )
        .unwrap()
    };
    trap(&details);
    let root = heap.object().unwrap();
    heap.define_own_data(&root, "details".into(), details.clone(), values.runtime()).unwrap();
    let opts = RenderOptions::default();
    let context = EditRenderContext { edit_mode: Some(ara_edit::EditMode::Hashline), ..Default::default() };
    let theme = Arc::new(Theme::dark());
    let mut single = GraphEditRenderer::result(
        &graph,
        &root,
        None,
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        theme.clone(),
    )
    .unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 1, "single construction selects its inline diff guard once");
    let frame = FrameContext::default();
    let rows = single.render(&graph, 100, &frame, opts, &context).unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 4, "header, body guard, selected body are independent Gets");
    assert!(rows.join("\n").contains("selected-body-4"));
    heap.define_own_data(&details, "path".into(), heap.string("changed-link.ts".into()).unwrap(), values.runtime(),
    )
        .unwrap();
    let rows = single.render(&graph, 100, &frame, opts, &context).unwrap();
    assert!(rows.join("\n").contains("initial-path.ts"), "raw header path was captured at construction");
    assert_eq!(reads.count.load(Ordering::SeqCst), 7, "equal-width framed callback still builds before output cache");
    reads.throw_at.store(10, Ordering::SeqCst);
    match single.render(&graph, 100, &frame, opts, &context) {
        Err(ValueError::Thrown(v)) => assert!(v.same_identity(&reason)),
        _ => panic!("selected body Get must preserve original throw"),
    }
    reads.throw_at.store(usize::MAX, Ordering::SeqCst);
    reads.count.store(0, Ordering::SeqCst);
    let a = heap.import_serde(&serde_json::json!({"path":"a.ts"})).unwrap();
    let b = heap.import_serde(&serde_json::json!({"path":"b.ts"})).unwrap();
    trap(&a);
    trap(&b);
    let array = heap.array_with_length(2).unwrap();
    heap.define_own_data(&array, "0".into(), a, values.runtime()).unwrap();
    heap.define_own_data(&array, "1".into(), b, values.runtime()).unwrap();
    let details = heap.object().unwrap();
    heap.define_own_data(&details, "perFileResults".into(), array.clone(), values.runtime()).unwrap();
    let root = heap.object().unwrap();
    heap.define_own_data(&root, "details".into(), details, values.runtime()).unwrap();
    let mut multi = GraphEditRenderer::result(
        &graph,
        &root,
        None,
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        theme.clone(),
    )
    .unwrap();
    multi.render(&graph, 100, &frame, opts, &context).unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 8);
    multi.render(&graph, 100, &frame, opts, &context).unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 8, "equal multi key skips children");
    heap.assign(&array, "length".into(), heap.number(3.0).unwrap(), values.runtime()).unwrap();
    multi.render(&graph, 100, &frame, opts, &context).unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 14, "new array length invalidates key without remapping children");
    reads.count.store(0, Ordering::SeqCst);
    let args = heap.object().unwrap();
    heap.define_own(
        &args,
        "file_path".into(),
        PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Host(CapabilityId(7896))),
            set: None,
            enumerable: true,
            configurable: true,
        },
        values.runtime(),
    )
    .unwrap();
    let mut tool = super::tool_execution::ToolExecutionComponent::new(
        "dynamic-caller".into(),
        "edit".into(),
        EditRenderArgs::default(),
        ara_edit::EditMode::Hashline,
        Arc::new(Theme::dark()),
    );
    tool.graph_args_root = Some(args.clone());
    assert!(tool.rebuild_in(&graph).is_empty());
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "target selects file_path twice then captures its cache operand");
    tool.set_args_complete();
    assert!(tool.rebuild_in(&graph).is_empty());
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "argsComplete rebuild retains partial facts for the same root and length");
    tool.set_args_complete();
    assert!(tool.rebuild_in(&graph).is_empty());
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "same complete state keeps the display key");
    tool.set_execution_started();
    assert!(tool.rebuild_in(&graph).is_empty());
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "executionStarted alone does not end partial result state");
    tool.render_in(&graph, 100, &frame, 1, usize::MAX, std::time::Instant::now()).unwrap();
    tool.render_in(&graph, 100, &frame, 2, usize::MAX, std::time::Instant::now()).unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "pure spinner frames keep captured header facts");
    tool.toggle_expanded();
    assert!(tool.rebuild_in(&graph).is_empty());
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "expansion does not invalidate target partial facts");
    let compact = tool.render_in(&graph, 100, &frame, 2, 1, std::time::Instant::now()).unwrap();
    assert_eq!(compact.len(), 1);
    assert!(compact[0].contains("Edit"));
    assert_eq!(reads.count.load(Ordering::SeqCst), 3, "activitySummary shares the target partial facts cache");
    GraphEditRenderer::activity(&graph, &args, RenderOptions { is_partial: false, ..opts }, &context).unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 6, "a final frame always recomputes exact facts and captures operands");
    reads.throw_at.store(9, Ordering::SeqCst);
    match GraphEditRenderer::activity(&graph, &args, RenderOptions { is_partial: false, ..opts }, &context) {
        Err(ValueError::Thrown(value)) => assert!(value.same_identity(&reason)),
        _ => panic!("the target cache operand Get preserves its original throw"),
    }
    reads.throw_at.store(usize::MAX, Ordering::SeqCst);
    let root = heap.import_serde(&serde_json::json!({"details":{"path":"detail-fallback.ts","diff":""}})).unwrap();
    let args = heap.import_serde(&serde_json::json!({"edits":[]})).unwrap();
    let mut empty = GraphEditRenderer::result(
        &graph,
        &root,
        Some(&args),
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        Arc::new(Theme::dark()),
    )
    .unwrap();
    assert!(
        empty.render(&graph, 100, &frame, opts, &context).unwrap().join("\n").contains("detail-fallback.ts"),
        "empty first edit takes the original optional-chain detail path fallback"
    );
    let args = heap.import_serde(&serde_json::json!({"edits":[{"path":"x.ts","op":"delete"}]})).unwrap();
    let mut deleted = GraphEditRenderer::result(
        &graph,
        &root,
        Some(&args),
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        Arc::new(Theme::dark()),
    )
    .unwrap();
    assert_eq!(
        deleted.render(&graph, 100, &frame, opts, &context).unwrap().len(),
        1,
        "first edit operation participates in the inline chain"
    );
    let args = heap.import_serde(&serde_json::json!({"input":"[hash-fallback.ts#1a2b]\nPUT 1a2b\ntext"})).unwrap();
    let no_path = heap.import_serde(&serde_json::json!({"details":{"diff":""}})).unwrap();
    let mut hash = GraphEditRenderer::result(
        &graph,
        &no_path,
        Some(&args),
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        Arc::new(Theme::dark()),
    )
    .unwrap();
    assert!(hash.render(&graph, 100, &frame, opts, &context).unwrap().join("\n").contains("hash-fallback.ts"));
    reads.count.store(0, Ordering::SeqCst);
    let args = heap.object().unwrap();
    heap.define_own(
        &args,
        "file_path".into(),
        PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Host(CapabilityId(7897))),
            set: None,
            enumerable: true,
            configurable: true,
        },
        values.runtime(),
    )
    .unwrap();
    let mut selected = GraphEditRenderer::result(
        &graph,
        &root,
        Some(&args),
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        Arc::new(Theme::dark()),
    )
    .unwrap();
    assert_eq!(reads.count.load(Ordering::SeqCst), 2);
    assert!(
        matches!(selected.render(&graph, 100, &frame, opts, &context), Err(ValueError::TypeError(_))),
        "second selected number reaches node:path.basename consumer rather than detail fallback"
    );
    let sparse = heap.array_with_length(2).unwrap();
    heap.define_own_data(
        &sparse,
        "1".into(),
        heap.import_serde(&serde_json::json!({"path":"later.ts","diff":""})).unwrap(),
        values.runtime(),
    )
    .unwrap();
    let sparse_details = heap.object().unwrap();
    heap.define_own_data(&sparse_details, "perFileResults".into(), sparse, values.runtime()).unwrap();
    let sparse_root = heap.object().unwrap();
    heap.define_own_data(&sparse_root, "details".into(), sparse_details, values.runtime()).unwrap();
    let mut sparse = GraphEditRenderer::result(
        &graph,
        &sparse_root,
        None,
        &EditRenderArgs::default(),
        false,
        opts,
        context.clone(),
        Arc::new(Theme::dark()),
    )
    .unwrap();
    assert!(
        matches!(sparse.render(&graph, 100, &frame, opts, &context), Err(ValueError::TypeError(_))),
        "map preserves hole and original index render(undefined) throws"
    );
    let now = std::time::Instant::now();
    let mut elapsed = super::tool_execution::ToolExecutionComponent::new(
        "elapsed".into(),
        "edit".into(),
        EditRenderArgs::default(),
        ara_edit::EditMode::Hashline,
        Arc::new(Theme::dark()),
    );
    elapsed.graph_args_root = Some(heap.import_serde(&serde_json::json!({"path":"elapsed.ts"})).unwrap());
    assert!(elapsed.rebuild_in(&graph).is_empty());
    elapsed.set_execution_started_at(now);
    let compact = elapsed.render_in(&graph, 100, &frame, 2, 1, now + std::time::Duration::from_millis(2300)).unwrap();
    assert!(compact[0].contains("2s"));
    domain.close();

    // Additional discriminating observations stay in the original coarse EDIT
    // renderer entry. They exercise actual compiled consumers and the realm's
    // shared Set/Boolean, rather than synthesizing a display callback marker.
    {
        use std::collections::{HashMap, VecDeque};
        use std::sync::Mutex;
        struct Script {
            replies: Mutex<HashMap<u64, VecDeque<Result<ValueHandle>>>>,
            calls: Mutex<Vec<u64>>,
        }
        impl Script {
            fn put(&self, cap: u64, replies: Vec<Result<ValueHandle>>) {
                self.replies.lock().unwrap().insert(cap, replies.into());
            }
        }
        impl ValueRuntime for Script {
            fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
                let cap = request.capability();
                self.calls.lock().unwrap().push(cap.0);
                self.replies
                    .lock()
                    .unwrap()
                    .get_mut(&cap.0)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or(Err(ValueError::MissingCapability(cap)))
                    .map(RuntimeReply::Value)
            }
        }
        static RETURN_AND_COUNT: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "EDIT original coarse observation: selected consumer effect",
            name: "selectedConsumer",
            length: 0,
            form: CompiledFunctionForm::Ordinary,
            capture_count: 2,
            body: |heap, captures, _, _, runtime| {
                let n = heap.to_number(&heap.get(&captures[1], &"n".into(), runtime)?, runtime)?;
                heap.assign(&captures[1], "n".into(), heap.number(n + 1.0)?, runtime)?;
                Ok(captures[0].clone())
            },
        };
        static OBSERVE_FILTER: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "EDIT original coarse observation: same environment Boolean argument",
            name: "filter",
            length: 1,
            form: CompiledFunctionForm::Ordinary,
            capture_count: 2,
            body: |heap, captures, receiver, arguments, runtime| {
                assert!(heap.object_is(arguments.first().unwrap(), &captures[0])?);
                let n = heap.to_number(&heap.get(&captures[1], &"n".into(), runtime)?, runtime)?;
                heap.assign(&captures[1], "n".into(), heap.number(n + 1.0)?, runtime)?;
                Ok(receiver.clone())
            },
        };
        static THROW_SELECTED: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "EDIT original coarse observation: original toJSON throw",
            name: "toJSON",
            length: 0,
            form: CompiledFunctionForm::Ordinary,
            capture_count: 2,
            body: |heap, captures, _, _, runtime| {
                let n = heap.to_number(&heap.get(&captures[1], &"n".into(), runtime)?, runtime)?;
                heap.assign(&captures[1], "n".into(), heap.number(n + 1.0)?, runtime)?;
                Err(ValueError::Thrown(captures[0].clone()))
            },
        };
        let heap = ValueHeap::new().unwrap();
        let script = Arc::new(Script { replies: Mutex::new(HashMap::new()), calls: Mutex::new(Vec::new()),
        });
        let values = Arc::new(SessionValues::new(heap.clone(), script.clone()).unwrap());
        let domain = CallbackDomain::new(&values);
        let globals = crate::native_realm::NativeGlobals::prepare(&heap, values.runtime()).unwrap();
        let _ast = crate::native_realm::ast::NativeAstModule::prepare(&heap, &globals, values.runtime()).unwrap();
        domain.bind_environment(&globals.environment).unwrap();
        let runtime = CallbackRuntime { base: script.clone(), domain: domain.clone(),
        };
        let g = DisplayGraph(&values, &runtime);
        let getter = |root: &ValueHandle, key: &str, cap: u64| {
            heap.define_own(
                root,
                key.into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(cap))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                &runtime,
            )
            .unwrap();
        };
        let effect = || heap.import_serde(&serde_json::json!({"n":0})).unwrap();
        let count = |root: &ValueHandle| heap.to_number(&g.get(root, "n").unwrap(), &runtime).unwrap();
        let set = g.get(&globals.environment, "Set").unwrap();
        let boolean = g.get(&globals.environment, "Boolean").unwrap();
        let effects = effect();
        let filter = heap.fresh_compiled_function(&OBSERVE_FILTER, &[boolean.clone(), effects.clone()]).unwrap();
        heap.define_own_data(&heap.array_prototype(), "filter".into(), filter, &runtime).unwrap();
        getter(&heap.set_intrinsic_prototype(), "constructor", 7910);
        script.put(7910, vec![Err(ValueError::Thrown(heap.object().unwrap()))]);
        getter(&globals.environment, "Set", 7911);
        getter(&globals.environment, "Boolean", 7912);
        script.put(7911, vec![Ok(set.clone()), Ok(set)]);
        script.put(7912, vec![Ok(boolean.clone()), Ok(boolean)]);
        let edits = heap.import_serde(&serde_json::json!([{"path":"a.ts"},{"path":"a.ts"},{"path":"b.ts"}])).unwrap();
        let map = g.get(&edits, "map").unwrap();
        getter(&edits, "map", 7913);
        script.put(7913, vec![Ok(map.clone()), Ok(map)]);
        assert_eq!(g.count_edits(&edits).unwrap(), 2);
        assert_eq!(g.count_edits(&edits).unwrap(), 2);
        assert_eq!(count(&effects), 2.0);
        assert_eq!(*script.calls.lock().unwrap(), vec![7911, 7913, 7912, 7911, 7913, 7912]);

        let args = heap.object().unwrap();
        let input = heap.object().unwrap();
        let effects = effect();
        let patch = heap.string("*** Begin Patch\n*** Delete File: from-to-json.ts\n*** End Patch".into()).unwrap();
        let to_json = heap.fresh_compiled_function(&RETURN_AND_COUNT, &[patch, effects.clone()]).unwrap();
        heap.define_own_data(&input, "toJSON".into(), to_json, &runtime).unwrap();
        getter(&args, "input", 7920);
        let guard = heap.string("guard".into()).unwrap();
        script.put(7920, vec![Ok(guard.clone()), Ok(guard.clone()), Ok(guard.clone()), Ok(input.clone()),
            ],
        );
        g.input(&args).unwrap();
        g.input(&args).unwrap();
        let summary =
            g.apply_summary(&args, RenderOptions::default(), Some(ara_edit::EditMode::ApplyPatch)).unwrap().unwrap();
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].path, "from-to-json.ts");
        assert_eq!(summary.entries[0].op, Some(Operation::Delete));
        assert_eq!(count(&effects), 1.0);
        let missing = heap.string("The last line of the patch must be '*** End Patch'".into()).unwrap();
        let error = heap.construct(&g.get(&globals.environment, "Error").unwrap(), &[missing], &runtime).unwrap();
        let partial = RenderOptions { is_partial: true, ..Default::default() };
        script.put(7920, vec![Ok(guard.clone()), Err(ValueError::Thrown(error.clone()))]);
        assert!(
            g.apply_summary(&args, partial, Some(ara_edit::EditMode::ApplyPatch)).unwrap().unwrap().error.is_none()
        );
        script.put(7920, vec![Ok(guard.clone()), Err(ValueError::Thrown(error.clone()))]);
        assert!(
            g.apply_summary(&args, RenderOptions::default(), Some(ara_edit::EditMode::ApplyPatch))
                .unwrap()
                .unwrap()
                .error
                .is_some()
        );
        script.put(7920, vec![Err(ValueError::Thrown(error.clone()))]);
        assert!(
            matches!(g.apply_summary(&args,partial,Some(ara_edit::EditMode::ApplyPatch)),Err(ValueError::Thrown(v)) if v.same_identity(&error))
        );
        // Fresh identities avoid the successful inspect cache from the earlier
        // toJSON fixture. The real throw must occur before partial suppression.
        let throwing_args = heap.object().unwrap();
        let throwing_input = heap.object().unwrap();
        let throwing_effects = effect();
        getter(&throwing_args, "input", 7920);
        heap.define_own_data(
            &throwing_input,
            "toJSON".into(),
            heap.fresh_compiled_function(&THROW_SELECTED, &[error.clone(), throwing_effects.clone()]).unwrap(),
            &runtime,
        )
        .unwrap();
        script.put(7920, vec![Ok(guard.clone()), Ok(throwing_input.clone())]);
        assert!(
            g.apply_summary(&throwing_args, partial, Some(ara_edit::EditMode::ApplyPatch)).unwrap().unwrap().error.is_none()
        );
        assert_eq!(count(&throwing_effects), 1.0, "fresh inspection really invokes the throwing toJSON once");
        script.put(7920, vec![Ok(guard.clone()), Ok(throwing_input.clone())]);
        let full = g.apply_summary(&throwing_args, RenderOptions::default(), Some(ara_edit::EditMode::ApplyPatch)).unwrap().unwrap();
        assert!(matches!(heap.snapshot(full.error.as_ref().unwrap()).unwrap(), ValueSnapshot::String(text)
            if text.equals_ascii("The last line of the patch must be '*** End Patch'")));
        assert_eq!(count(&throwing_effects), 1.0, "cached inspection error is reused for the final frame");
        let repaired = heap.string("*** Begin Patch\n*** Delete File: replacement-must-not-run.ts\n*** End Patch".into()).unwrap();
        heap.define_own_data(&throwing_input, "toJSON".into(),
            heap.fresh_compiled_function(&RETURN_AND_COUNT, &[repaired, throwing_effects.clone()]).unwrap(), &runtime).unwrap();
        script.put(7920, vec![Ok(guard.clone()), Ok(throwing_input)]);
        let cached = g.apply_summary(&throwing_args, RenderOptions::default(), Some(ara_edit::EditMode::ApplyPatch)).unwrap().unwrap();
        assert!(matches!(heap.snapshot(cached.error.as_ref().unwrap()).unwrap(), ValueSnapshot::String(text)
            if text.equals_ascii("The last line of the patch must be '*** End Patch'")));
        assert_eq!(count(&throwing_effects), 1.0, "same args/input identity retains cached error without replaying a replacement toJSON");

        let opts = RenderOptions::default();
        let context = EditRenderContext { edit_mode: Some(ara_edit::EditMode::Hashline), ..Default::default() };
        let theme = Arc::new(Theme::dark());
        let details = heap.import_serde(&serde_json::json!({"path":"selected.ts"})).unwrap();
        getter(&details, "diff", 7930);
        let empty = heap.string("".into()).unwrap();
        let text = heap.string("+selected".into()).unwrap();
        script.put(7930, vec![Ok(text.clone()), Ok(text.clone()), Ok(text.clone()), Ok(empty.clone())],
        );
        let root = heap.object().unwrap();
        heap.define_own_data(&root, "details".into(), details.clone(), &runtime).unwrap();
        let mut selected = GraphEditRenderer::result(
            &g,
            &root,
            None,
            &EditRenderArgs::default(),
            false,
            opts,
            context.clone(),
            theme.clone(),
        )
        .unwrap();
        let rows = selected.render(&g, 100, &FrameContext::default(), opts, &context).unwrap();
        assert!(!rows.join("\n").contains("No changes"), "selected empty string retains the source diff branch");
        let object = heap.object().unwrap();
        let effects = effect();
        let lines = heap.array(vec![text.clone()]).unwrap();
        let split = heap.fresh_compiled_function(&RETURN_AND_COUNT, &[lines, effects.clone()]).unwrap();
        let well_formed = heap.fresh_compiled_function(&RETURN_AND_COUNT, &[text.clone(), effects.clone()]).unwrap();
        heap.define_own_data(&object, "split".into(), split, &runtime).unwrap();
        heap.define_own_data(&object, "toWellFormed".into(), well_formed, &runtime).unwrap();
        script.put(7930, vec![Ok(object.clone()); 7]);
        let mut selected = GraphEditRenderer::result(
            &g,
            &root,
            None,
            &EditRenderArgs::default(),
            false,
            opts,
            context.clone(),
            theme.clone(),
        )
        .unwrap();
        let rows = selected.render(&g, 100, &FrameContext::default(), opts, &context).unwrap();
        assert!(rows.join("\n").contains("+1"));
        assert_eq!(
            count(&effects),
            4.0,
            "header split, body split/stats and sanitizer consume original selected object"
        );
        selected.render(&g, 100, &FrameContext::default(), opts, &context).unwrap();
        assert_eq!(count(&effects), 4.0, "equal original selected object hits the source consumer caches");
        let numeric = heap.number(7.0).unwrap();
        assert!(matches!(g.diff_stats_value(&numeric), Err(ValueError::TypeError(_))));
        assert!(matches!(g.plain_preview_value(&numeric, &theme), Err(ValueError::TypeError(_))));
        assert!(matches!(g.streaming_tail_value(&numeric, 80, 6), Err(ValueError::TypeError(_))));
        let args = heap.import_serde(&serde_json::json!({"path":"call.ts"})).unwrap();
        getter(&args, "previewDiff", 7931);
        script.put(7931, vec![Ok(text.clone()), Ok(text.clone()), Ok(empty)]);
        let fallback =
            EditRenderContext { edit_streaming_fallback: Some("FORBIDDEN-FALLBACK".into()), ..context.clone() };
        let mut call = GraphEditRenderer::call(&g, args, opts, fallback.clone(), theme.clone()).unwrap();
        assert!(
            !call
                .render(&g, 100, &FrameContext::default(), opts, &fallback)
                .unwrap()
                .join("\n")
                .contains("FORBIDDEN-FALLBACK")
        );
        let args = heap.object().unwrap();
        getter(&args, "new_string", 7932);
        script.put(7932, vec![Ok(text), Ok(object)]);
        let replace = EditRenderContext { edit_mode: Some(ara_edit::EditMode::Replace), ..Default::default() };
        let call = GraphEditRenderer::call(&g, args, opts, replace, theme).unwrap();
        assert!(call.owned_context().unwrap().edit_streaming_fallback.is_some());

        // Source renderer:801/805. One retained fixture supplies the original
        // same-realm function through the production Host context seam.
        static ORIGINAL_RENDER_DIFF: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "EDIT original coarse observation: renderDiffFn original operand/options/cache",
            name: "renderDiff",
            length: 2,
            form: CompiledFunctionForm::Ordinary,
            capture_count: 3,
            body: |heap, captures, receiver, arguments, runtime| {
                assert!(matches!(heap.snapshot(receiver)?, ValueSnapshot::Undefined));
                let state = &captures[0];
                assert!(heap.object_is(&arguments[0], &heap.get(state, &"expected".into(), runtime)?)?);
                assert!(heap.object_is(&heap.get(&arguments[1], &"filePath".into(), runtime)?, &captures[2])?);
                let previous = heap.get(state, &"options".into(), runtime)?;
                if matches!(heap.snapshot(&previous)?, ValueSnapshot::Object) {
                    assert!(!previous.same_identity(&arguments[1]), "each actual callback Call creates fresh options");
                }
                heap.assign(state, "options".into(), arguments[1].clone(), runtime)?;
                let n = heap.to_number(&heap.get(state, &"n".into(), runtime)?, runtime)?;
                heap.assign(state, "n".into(), heap.number(n + 1.0)?, runtime)?;
                let reason = heap.get(state, &"throw".into(), runtime)?;
                if heap.truthy(&reason)? {
                    return Err(ValueError::Thrown(reason));
                }
                Ok(captures[1].clone())
            },
        };
        let theme = Arc::new(Theme::dark());
        let state = effect();
        let raw_path = heap.string(WireString::from_units(vec![99, 97, 108, 108, 1, 9, 0xd800, 46, 116, 115,
            ])).unwrap();
        let output = heap.string("+1|callback-result".into()).unwrap();
        let callback = heap
            .fresh_compiled_function(&ORIGINAL_RENDER_DIFF, &[state.clone(), output.clone(), raw_path.clone()],
            )
            .unwrap();
        let details = heap.object().unwrap();
        heap.define_own_data(&details, "path".into(), raw_path.clone(), &runtime).unwrap();
        getter(&details, "diff", 7940);
        let root = heap.object().unwrap();
        heap.define_own_data(&root, "details".into(), details, &runtime).unwrap();
        let guard = heap.string("+guard".into()).unwrap();
        let replies = |selected: &ValueHandle, construction: bool, frames: usize| {
            let mut sequence = Vec::new();
            if construction {
                sequence.push(Ok(guard.clone()));
            }
            for _ in 0..frames {
                sequence.extend([Ok(guard.clone()), Ok(guard.clone()), Ok(selected.clone())]);
            }
            script.put(7940, sequence);
            heap.assign(&state, "expected".into(), selected.clone(), &runtime).unwrap();
        };
        let expanded = RenderOptions { expanded: true, ..Default::default() };
        let context = EditRenderContext {
            edit_mode: Some(ara_edit::EditMode::Hashline),
            render_diff_value: Some(callback.clone()),
            render_diff: Some(Arc::new(|_, _| {
                panic!("actual Graph callback must bypass typed string adapter")
            })),
            ..Default::default()
        };
        let original = heap.object().unwrap();
        replies(&original, true, 3);
        let mut retained = GraphEditRenderer::result(
            &g,
            &root,
            None,
            &EditRenderArgs::default(),
            false,
            expanded,
            context.clone(),
            theme.clone(),
        )
        .unwrap();
        assert!(
            retained
                .render(&g, 100, &FrameContext::default(), expanded, &context)
                .unwrap()
                .join("\n")
                .contains("callback-result")
        );
        retained.render(&g, 100, &FrameContext::default(), expanded, &context).unwrap();
        retained.render(&g, 101, &FrameContext::default(), expanded, &context).unwrap();
        assert_eq!(count(&state), 1.0, "same operand/callback keeps raw color result when section width changes");
        let mut replacement = context.clone();
        replacement.render_diff_value =
            Some(heap.fresh_compiled_function(&ORIGINAL_RENDER_DIFF, &[state.clone(), output, raw_path]).unwrap(),
        );
        replies(&original, false, 1);
        retained.render(&g, 101, &FrameContext::default(), expanded, &replacement).unwrap();
        assert_eq!(count(&state), 2.0, "new callback identity invalidates section and color caches");
        for selected in [heap.number(7.0).unwrap(), heap.undefined().unwrap()] {
            replies(&selected, false, 1);
            retained.render(&g, 101, &FrameContext::default(), expanded, &replacement).unwrap();
        }
        assert_eq!(count(&state), 4.0, "number and undefined selected operands reach the original callback");
        let nan = heap.number(f64::NAN).unwrap();
        replies(&nan, false, 2);
        retained.render(&g, 101, &FrameContext::default(), expanded, &replacement).unwrap();
        retained.render(&g, 101, &FrameContext::default(), expanded, &replacement).unwrap();
        assert_eq!(count(&state), 6.0, "source === makes NaN miss each frame");
        replies(&heap.number(0.0).unwrap(), false, 1);
        retained.render(&g, 101, &FrameContext::default(), expanded, &replacement).unwrap();
        replies(&heap.number(-0.0).unwrap(), false, 1);
        retained.render(&g, 101, &FrameContext::default(), expanded, &replacement).unwrap();
        assert_eq!(count(&state), 7.0, "source === makes +0 and -0 share the cached result");
        let text = heap.string("+collapsed".into()).unwrap();
        replies(&text, false, 2);
        retained.render(&g, 100, &FrameContext::default(), opts, &replacement).unwrap();
        retained.render(&g, 101, &FrameContext::default(), opts, &replacement).unwrap();
        assert_eq!(count(&state), 8.0, "collapsed callback shares raw rendered value across section widths");
        let reason = heap.object().unwrap();
        heap.assign(&state, "throw".into(), reason.clone(), &runtime).unwrap();
        retained.invalidate();
        replies(&text, false, 2);
        for _ in 0..2 {
            assert!(
                matches!(retained.render(&g,101,&FrameContext::default(),expanded,&replacement),Err(ValueError::Thrown(v)) if v.same_identity(&reason))
            );
        }
        assert_eq!(
            count(&state),
            10.0,
            "callback throws preserve original identity and never publish successful caches"
        );
        heap.assign(&state, "throw".into(), heap.undefined().unwrap(), &runtime).unwrap();
        for nullish in [heap.undefined().unwrap(), heap.null().unwrap()] {
            let mut fallback = replacement.clone();
            fallback.render_diff_value = Some(nullish);
            replies(&text, false, 1);
            retained.render(&g, 101, &FrameContext::default(), expanded, &fallback).unwrap();
        }
        assert_eq!(count(&state), 10.0, "nullish Graph slot selects native fallback even with a typed adapter present");
        let mut noncallable = replacement.clone();
        noncallable.render_diff_value = Some(heap.boolean(false).unwrap());
        replies(&text, false, 1);
        assert!(matches!(
            retained.render(&g, 101, &FrameContext::default(), expanded, &noncallable),
            Err(ValueError::TypeError(_))
        ));
        let mut foreign = replacement.clone();
        foreign.render_diff_value = Some(ValueHeap::new().unwrap().object().unwrap());
        assert!(retained.render(&g, 101, &FrameContext::default(), expanded, &foreign).is_err());
        domain.close();
    }
}

#[derive(Clone)]
pub(crate) enum GraphEditRenderer {
    Call {
        args: ValueHandle,
        component: EditComponent,
        inline: bool,
    },
    Single {
        details: Option<ValueHandle>,
        component: EditComponent,
        inline: bool,
    },
    Multi {
        array: ValueHandle,
        components: ValueHandle,
        children: Vec<ChildRecord>,
        shared: SharedState,
        remaining: usize,
        cached: Option<((bool, usize, u32, usize), Vec<String>)>,
        theme: Arc<Theme>,
    },
}
impl GraphEditRenderer {
    fn resolve_facts_uncached(
        g: &DisplayGraph<'_>,
        root: &ValueHandle,
        options: RenderOptions,
        context: &EditRenderContext,
    ) -> Result<(EditRenderArgs, edit_renderer::CallFacts, SelectedOperands)> {
        let mode = context.edit_mode;
        let mut args = EditRenderArgs::default();
        let mut selected = SelectedOperands::default();
        let hash = g.input(root)?;
        let sloppy_value = g.input_value(root)?;
        let sloppy = g.optional(&sloppy_value)?;
        let sloppy_entries = if mode == Some(ara_edit::EditMode::Sloppy) && sloppy.is_some() {
            g.inspected_entries(root, ara_edit::EditMode::Sloppy, &sloppy_value)
                .ok()
                .filter(|entries| !entries.is_empty())
        } else {
            None
        };
        let apply = g.apply_summary(root, options, mode)?;
        args.input = match mode {
            Some(ara_edit::EditMode::Hashline) => hash,
            Some(ara_edit::EditMode::Sloppy) => sloppy,
            _ => None,
        };
        let edits = g.get(root, "edits")?;
        let first = if g.0.heap().kind(&edits)? == ValueKind::Array {
            let edits = g.get(root, "edits")?;
            if g.0.heap().array_length(&edits, g.1)? > 0 {
                let edits = g.get(root, "edits")?;
                Some(g.get(&edits, "0")?)
            } else {
                None
            }
        } else {
            None
        };
        let file_path = g.typed(root, "file_path")?;
        let path = if file_path.is_none() { g.typed(root, "path")? } else { None };
        args.file_path = g.wire(&file_path)?;
        args.path = g.wire(&path)?;
        selected.path = file_path.or(path);
        let mut first_entry = EditEntry::default();
        if selected.path.is_none() {
            first_entry.path = g.optional_entry(first.as_ref(), "path")?;
            if first_entry.path.is_none() {
                args.partial_json = g.entry(root, "__partialJson")?;
            }
        }
        let rename = g.typed(root, "rename")?;
        if !g.nullish_selected(&rename)? {
            args.rename = g.wire(&rename)?;
            selected.rename = rename;
        } else {
            first_entry.rename = g.optional_entry(first.as_ref(), "rename")?;
            if first_entry.rename.is_none() {
                first_entry.move_to = g.optional_entry(first.as_ref(), "move")?;
            }
        }
        let op = g.get(root, "op")?;
        args.op = g.operation(&op)?;
        if !g.truth(&op)? {
            first_entry.op = g.optional_operation(first.as_ref())?;
        }
        if first.is_some() {
            args.edits = Some(vec![first_entry]);
        }
        let mut facts = if mode.is_none() || mode == Some(ara_edit::EditMode::ApplyPatch) {
            let (entries, error) = match apply {
                Some(summary) => (Some(summary.entries), summary.error),
                None => (None, None),
            };
            let error_marker =
                error.as_ref().map(|value| g.truth(value)).transpose()?.unwrap_or(false).then(|| String::new());
            selected.apply_error = error;
            edit_renderer::call_facts_with_input_summary(&args, entries, error_marker)
        } else if mode == Some(ara_edit::EditMode::Sloppy) {
            let mut facts = edit_renderer::call_facts_with_input_summary(&args, sloppy_entries, None);
            // Target sloppy summary contributes path/count, not operation/move.
            facts.rename = super::edit_renderer::call_facts_with_input_summary(&args, None, None).rename;
            facts.op = super::edit_renderer::call_facts_with_input_summary(&args, None, None).op;
            facts
        } else {
            edit_renderer::call_facts(&args, options, context)
        };
        let edits = g.get(root, "edits")?;
        if g.0.heap().kind(&edits)? == ValueKind::Array {
            let edits = g.get(root, "edits")?;
            facts.file_count = g.count_edits(&edits)?;
        }
        // A ternary's second selected value is final, including null/number/object.
        // Only the eventual source consumer checks it; no path fallback is allowed.
        if selected.path.is_some() {
            facts.path = g.wire(&selected.path)?.as_ref().map(super::render_utils::sanitize).unwrap_or_default();
        }
        Ok((args, facts, selected))
    }
    fn resolve_facts(
        g: &DisplayGraph<'_>,
        root: &ValueHandle,
        options: RenderOptions,
        context: &EditRenderContext,
    ) -> Result<(EditRenderArgs, edit_renderer::CallFacts, SelectedOperands)> {
        let owner = CallbackDomain::for_values(g.0)?;
        let prior = owner.facts_cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
        fn length(g: &DisplayGraph<'_>, root: &ValueHandle) -> Result<usize> {
            let input = g.input_value(root)?;
            if let ValueSnapshot::String(input) = g.0.heap().snapshot(&input)? {
                return Ok(input.units().len());
            }
            let edits = g.get(root, "edits")?;
            if g.0.heap().kind(&edits)? == ValueKind::Array {
                let edits = g.get(root, "edits")?;
                return Ok(g.0.heap().array_length(&edits, g.1)? as usize);
            }
            Ok(0)
        }
        fn payload(g: &DisplayGraph<'_>, root: &ValueHandle) -> Result<Option<WireString>> {
            let input = g.input_value(root)?;
            if !matches!(g.0.heap().snapshot(&input)?, ValueSnapshot::String(_)) {
                return Ok(None);
            }
            g.optional(&g.get(root, "__partialJson")?)
        }
        fn equal(g: &DisplayGraph<'_>, a: &ValueHandle, b: &ValueHandle) -> Result<bool> {
            match (g.0.heap().snapshot(a)?, g.0.heap().snapshot(b)?) {
                (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => Ok(a == b),
                _ => g.0.heap().object_is(a, b),
            }
        }
        if let Some(prior) = &prior {
            if options.is_partial && prior.partial && context.edit_mode == prior.mode {
                if root.same_identity(&prior.root) {
                    let n = length(g, root)?;
                    if n >= prior.length && n - prior.length < 512 {
                        return Ok(prior.value.clone());
                    }
                } else if let Some(base) = &prior.payload {
                    if let Some(next) = payload(g, root)? {
                        if next.units().len() >= base.units().len() && next.units().len() - base.units().len() < 512 {
                            let mut unchanged = true;
                            for (index, key) in ["file_path", "path", "rename", "op", "edits"].into_iter().enumerate() {
                                if !equal(g, &g.get(root, key)?, &prior.direct[index])? {
                                    unchanged = false;
                                    break;
                                }
                            }
                            if unchanged && next.units().starts_with(base.units()) {
                                return Ok(prior.value.clone());
                            }
                        }
                    }
                }
            }
        }
        let value = Self::resolve_facts_uncached(g, root, options, context)?;
        // Evaluation order follows the target cache object literal.
        let n = length(g, root)?;
        let payload = payload(g, root)?;
        let direct = [
            g.get(root, "file_path")?,
            g.get(root, "path")?,
            g.get(root, "rename")?,
            g.get(root, "op")?,
            g.get(root, "edits")?,
        ];
        *owner.facts_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(FactsCache {
            root: root.clone(),
            partial: options.is_partial,
            mode: context.edit_mode,
            length: n,
            payload,
            direct,
            value: value.clone(),
        });
        Ok(value)
    }
    pub(crate) fn native_call_args(g:&DisplayGraph<'_>,root:&ValueHandle,options:RenderOptions,context:&EditRenderContext)->Result<(EditRenderArgs,edit_renderer::CallFacts)> {
        let(mut args,facts,_)=Self::resolve_facts(g,root,options,context)?;
        let multi=context.per_file_diff_preview.as_ref().is_some_and(|p|p.len()>1&&p.iter().any(|p|p.diff.as_ref().is_some_and(|v|!v.is_empty())||p.error.as_ref().is_some_and(|v|!v.is_empty())));
        if !multi {
            args.preview_diff=g.optional(&g.get(root,"previewDiff")?)?;
            if args.preview_diff.as_ref().is_none_or(WireString::is_empty){
                args.diff=g.optional(&g.get(root,"diff")?)?;
                if args.diff.as_ref().is_none_or(WireString::is_empty){args.new_text=g.optional(&g.get(root,"newText")?)?;if args.new_text.as_ref().is_none_or(WireString::is_empty){args.patch=g.optional(&g.get(root,"patch")?)?;}}
            }
        }
        Ok((args,facts))
    }
    pub(crate) fn activity(
        g: &DisplayGraph<'_>,
        root: &ValueHandle,
        options: RenderOptions,
        context: &EditRenderContext,
    ) -> Result<(String, Option<String>)> {
        let (_, facts, selected) = Self::resolve_facts(g, root, options, context)?;
        let label = match facts.op {
            Some(Operation::Create) => "Create",
            Some(Operation::Delete) => "Delete",
            _ => "Edit",
        }
        .to_owned();
        if selected.path.as_ref().map(|v| g.truth(v)).transpose()?.unwrap_or(!facts.path.is_empty()) == false {
            return Ok((label, None));
        }
        let path = |p: &str| super::render_utils::replace_tabs(&super::render_utils::shorten_path(p));
        let mut detail = path(&facts.path);
        let rename_truth = if selected.rename.is_some() {
            g.truth_selected(&selected.rename)?
        } else {
            facts.rename.as_ref().is_some_and(|r| !r.is_empty())
        };
        if rename_truth {
            detail.push_str(&format!(" → {}", path(facts.rename.as_deref().unwrap_or(""))));
        }
        if facts.file_count > 1 {
            detail.push_str(&format!(" (+{} more)", facts.file_count - 1));
        }
        Ok((label, Some(detail)))
    }
    pub(crate) fn call(
        g: &DisplayGraph<'_>,
        root: ValueHandle,
        options: RenderOptions,
        mut context: EditRenderContext,
        theme: Arc<Theme>,
    ) -> Result<Self> {
        g.0.check_handle(&root)?;
        let mode = context.edit_mode;
        let mut args = EditRenderArgs::default();
        if mode == Some(ara_edit::EditMode::Replace)
            && context
                .per_file_diff_preview
                .as_ref()
                .is_none_or(|p| !p.iter().any(|p| p.diff.as_ref().is_some_and(|d| !d.is_empty())))
        {
            let new_string = g.typed(&root, "new_string")?;
            let new_text = if new_string.is_none() { g.typed(&root, "newText")? } else { None };
            let text = new_string.or(new_text);
            if g.truth_selected(&text)? {
                context.edit_streaming_fallback = Some(g.plain_preview_value(text.as_ref().unwrap(), &theme)?);
            }
            context.edit_streaming_fallback = context.edit_streaming_fallback.filter(|fallback| !fallback.is_empty());
        }
        let (mut owned, facts, selected) = Self::resolve_facts(g, &root, options, &context)?;
        owned.new_string = args.new_string;
        owned.new_text = args.new_text;
        args = owned;
        let multi = context.per_file_diff_preview.as_ref().is_some_and(|p| {
            p.len() > 1
                && p.iter().any(|p| {
                    p.diff.as_ref().is_some_and(|v| !v.is_empty()) || p.error.as_ref().is_some_and(|v| !v.is_empty())
                })
        });
        let mut payload = multi;
        if !payload {
            for key in ["previewDiff", "diff", "newText", "patch"] {
                if g.truth(&g.get(&root, key)?)? {
                    payload = true;
                    break;
                }
            }
        }
        if !payload {
            let e = g.get(&root, "edits")?;
            if g.0.heap().kind(&e)? == ValueKind::Array {
                let e = g.get(&root, "edits")?;
                payload = g.0.heap().array_length(&e, g.1)? > 0;
            }
        }
        payload |= context.edit_streaming_fallback.as_ref().is_some_and(|s| !s.is_empty());
        let mut component = EditComponent::call(args, options, context, theme);
        let inline = facts.file_count <= 1
            && facts.apply_error.is_none()
            && (facts.op == Some(Operation::Delete)
                || ((selected.rename.is_some() || facts.rename.is_some()) && !payload && !facts.hashline_line_edits));
        component.captured_call = Some(facts);
        component.captured_inline = Some(inline);
        component.selected_path = selected.path;
        component.selected_rename = selected.rename;
        component.selected_apply_error = selected.apply_error;
        Ok(Self::Call { args: root, component, inline,
        })
    }

    pub(crate) fn result(
        g: &DisplayGraph<'_>,
        root: &ValueHandle,
        args: Option<&ValueHandle>,
        owned_args: &EditRenderArgs,
        error: bool,
        options: RenderOptions,
        context: EditRenderContext,
        theme: Arc<Theme>,
    ) -> Result<Self> {
        let shared = Arc::new(std::sync::Mutex::new(DisplayRenderState {
            options,
            context: context.clone(),
            frame: FrameContext::default(),
        }));
        Self::result_with_state(g, root, args, owned_args, error, options, context, theme, shared)
    }
    pub(crate) fn result_with_state(
        g: &DisplayGraph<'_>,
        root: &ValueHandle,
        args: Option<&ValueHandle>,
        owned_args: &EditRenderArgs,
        error: bool,
        options: RenderOptions,
        context: EditRenderContext,
        theme: Arc<Theme>,
        shared: SharedState,
    ) -> Result<Self> {
        let edits = if let Some(args) = args {
            let v = g.get(args, "edits")?;
            if g.0.heap().kind(&v)? == ValueKind::Array { Some(g.get(args, "edits")?) } else { None }
        } else {
            None
        };
        let details = g.get(root, "details")?;
        let files = if g.nullish(&details)? { None } else { Some(g.get(&details, "perFileResults")?) };
        let total = edits.as_ref().map(|e| g.count_edits(e)).transpose()?.unwrap_or(0);
        let files = match files {
            Some(v) if g.truth(&v)? => Some(v),
            _ => None,
        };
        if let Some(array) = files {
            let length = g.0.heap().array_length(&array, g.1)?;
            if length > 1 || total > 1 {
                let method = g.get(&array, "map")?;
                let domain = CallbackDomain::for_values(g.0)?;
                let (mapper, records) = domain.mapper(shared.clone(), theme.clone())?;
                let components = g.0.heap().call(&method, &array, &[mapper], g.1)?;
                let children = records.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let remaining = total.saturating_sub(g.0.heap().array_length(&array, g.1)? as usize);
                return Ok(Self::Multi { array, components, children, shared, remaining, cached: None, theme,
                });
            }
        }
        let details = g.get(root, "details")?;
        let details = if g.nullish(&details)? { None } else { Some(details) };
        let mut renderer = Self::single(g, details, args, owned_args, error, Vec::new(), options, context, theme)?;
        if error {
            if let Self::Single { component, .. } = &mut renderer {
                let d = component.result.as_ref().and_then(|r| r.details.as_ref());
                if d.is_none_or(|d| {
                    d.display_error_text.as_ref().is_none_or(WireString::is_empty)
                        && d.error_text.as_ref().is_none_or(WireString::is_empty)
                }) {
                    let content = super::event_controller::content_from_graph(g, root, true)?;
                    if let Some(result) = &mut component.result {
                        result.content = content;
                    }
                }
            }
        }
        Ok(renderer)
    }

    fn single(
        g: &DisplayGraph<'_>,
        details: Option<ValueHandle>,
        args_root: Option<&ValueHandle>,
        owned_args: &EditRenderArgs,
        error: bool,
        content: Vec<WireString>,
        options: RenderOptions,
        context: EditRenderContext,
        theme: Arc<Theme>,
    ) -> Result<Self> {
        let mut args = owned_args.clone();
        let mut first = None;
        let mut selected = SelectedOperands::default();
        if let Some(a) = args_root {
            let edits = g.get(a, "edits")?;
            first = if g.0.heap().kind(&edits)? == ValueKind::Array {
                let edits = g.get(a, "edits")?;
                Some(g.get(&edits, "0")?)
            } else {
                None
            };
            args.input = g.input(a)?;
            args.file_path = None;
            args.path = None;
            args.op = None;
            args.rename = None;
            args.edits = None;
        }
        let mut d = EditDetails { is_error: Some(error), ..Default::default() };
        let source = details.as_ref().map(|d| g.present_typed(d, "sourcePath")).transpose()?.flatten();
        let detail_path = details.as_ref().map(|d| g.present_typed(d, "path")).transpose()?.flatten();
        d.source_path = g.wire(&source)?;
        d.path = g.wire(&detail_path)?;
        let mut first_entry = EditEntry::default();
        if !g.nullish_selected(&source)? {
            selected.path = source;
        } else if let Some(a) = args_root {
            let file_path = g.typed(a, "file_path")?;
            let path = if file_path.is_none() { g.typed(a, "path")? } else { None };
            args.file_path = g.wire(&file_path)?;
            args.path = g.wire(&path)?;
            selected.path = file_path.or(path);
            if selected.path.is_none() {
                first_entry.path = g.optional_entry(first.as_ref(), "path")?;
                if first_entry.path.is_none() && !g.nullish_selected(&detail_path)? {
                    selected.path = detail_path.clone();
                }
            }
        } else if !g.nullish_selected(&detail_path)? {
            selected.path = detail_path.clone();
        }
        if let Some(a) = args_root {
            args.op = g.operation(&g.get(a, "op")?)?;
            if args.op.is_none() {
                first_entry.op = g.optional_operation(first.as_ref())?;
            }
        }
        if args.op.or(first_entry.op).is_none() {
            if let Some(root) = &details {
                d.op = g.operation(&g.get(root, "op")?)?;
            }
        }
        if let Some(a) = args_root {
            let rename = g.typed(a, "rename")?;
            if !g.nullish_selected(&rename)? {
                args.rename = g.wire(&rename)?;
                selected.rename = rename;
            } else {
                first_entry.rename = g.optional_entry(first.as_ref(), "rename")?;
                if first_entry.rename.is_none() {
                    first_entry.move_to = g.optional_entry(first.as_ref(), "move")?;
                }
            }
        }
        if let Some(root) = &details {
            if selected.rename.is_none()
                && args.rename.is_none()
                && first_entry.rename.is_none()
                && first_entry.move_to.is_none()
            {
                let rename = g.present_typed(root, "move")?;
                d.move_to = g.wire(&rename)?;
                selected.rename = rename;
            }
            if error {
                d.display_error_text = g.present(root, "displayErrorText")?;
                if d.display_error_text.as_ref().is_none_or(WireString::is_empty) {
                    d.error_text = g.present(root, "errorText")?;
                }
            }
        }
        let resolved_op = args.op.or(first_entry.op).or(d.op);
        let rename_truth = if selected.rename.is_some() {
            g.truth_selected(&selected.rename)?
        } else {
            args.rename
                .as_ref()
                .or(first_entry.rename.as_ref())
                .or(first_entry.move_to.as_ref())
                .or(d.move_to.as_ref())
                .is_some_and(|v| !v.is_empty())
        };
        if first.is_some() {
            args.edits = Some(vec![first_entry]);
        }
        let mut inline = false;
        if !error {
            let diff = details.as_ref().map(|d| g.get(d, "diff")).transpose()?;
            let hasdiff = diff.as_ref().map(|d| g.truth(d)).transpose()?.unwrap_or(false);
            if !hasdiff {
                let diag = details.as_ref().map(|d| g.get(d, "diagnostics")).transpose()?;
                let hasdiag = diag.as_ref().map(|d| g.truth(d)).transpose()?.unwrap_or(false);
                inline = !hasdiag && (resolved_op == Some(Operation::Delete) || rename_truth);
                if inline {
                    if let Some(root) = &details {
                        d.path = g.present(root, "path")?;
                    }
                }
            }
        }
        let result = EditResult { content, details: details.as_ref().map(|_| d), is_error: Some(error),
        };
        let mut component = EditComponent::result(result, args, options, context, theme);
        component.capture_single();
        component.captured_inline = Some(inline);
        component.selected_path = selected.path;
        component.selected_rename = selected.rename;
        Ok(Self::Single { details, component, inline,
        })
    }
    pub(crate) fn args_root(&self) -> Option<&ValueHandle> {
        match self {
            Self::Call { args, .. } => Some(args),
            _ => None,
        }
    }
    pub(crate) fn owned_context(&self) -> Option<EditRenderContext> {
        match self {
            Self::Call { component, .. } | Self::Single { component, .. } => Some(component.context.clone()),
            Self::Multi { .. } => None,
        }
    }
    pub(crate) fn owned_args(&self) -> EditRenderArgs {
        match self {
            Self::Call { component, .. } | Self::Single { component, .. } => component.args.clone(),
            Self::Multi { .. } => EditRenderArgs::default(),
        }
    }
    pub(crate) fn owned_result(&self) -> EditResult {
        match self {
            Self::Single { component, .. } => component.result.clone().unwrap_or_default(),
            Self::Multi { children, .. } => EditResult {
                details: Some(EditDetails {
                    per_file_results: Some(children.iter().filter_map(|(_, _, c)| c.owned_result().details).collect(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            },
            _ => EditResult::default(),
        }
    }
    pub(crate) fn render(
        &mut self,
        g: &DisplayGraph<'_>,
        width: usize,
        frame: &FrameContext,
        options: RenderOptions,
        context: &EditRenderContext,
    ) -> Result<Vec<String>> {
        match self {
            Self::Call { args, component, inline,
            } => {
                component.options = options;
                component.context = context.clone();
                component.check_selected_header(g)?;
                if !*inline {
                    let multi = context.per_file_diff_preview.as_ref().is_some_and(|p| {
                        p.len() > 1
                            && p.iter().any(|p| p.diff.as_ref().is_some_and(|d| !d.is_empty()) || p.error.is_some())
                    });
                    component.graph_call_preview = None;
                    if !multi {
                        let preview = if g.truth(&g.get(args, "previewDiff")?)? {
                            Some(component.streaming_preview_in(
                                g,
                                &g.get(args, "previewDiff")?,
                                width,
                                frame,
                                "preview",
                            )?)
                        } else if g.truth(&g.get(args, "diff")?)? && g.truth(&g.get(args, "op")?)? {
                            Some(component.streaming_preview_in(g, &g.get(args, "diff")?, width, frame, "streaming")?)
                        } else if g.truth(&g.get(args, "diff")?)? {
                            Some(g.plain_preview_value(&g.get(args, "diff")?, &component.theme)?)
                        } else if g.truth(&g.get(args, "newText")?)? || g.truth(&g.get(args, "patch")?)? {
                            let text = g.get(args, "newText")?;
                            let text = if g.nullish(&text)? {
                                let patch = g.get(args, "patch")?;
                                if g.nullish(&patch)? { g.0.heap().string("".into())? } else { patch }
                            } else {
                                text
                            };
                            Some(g.plain_preview_value(&text, &component.theme)?)
                        } else {
                            None
                        };
                        component.graph_call_preview = preview;
                    }
                    if let Some(error) = component.selected_apply_error.as_ref() {
                        if g.truth(error)? {
                            let text = g.string(&g.replace_tabs_value(error)?)?;
                            if let Some(facts) = &mut component.captured_call {
                                facts.apply_error = Some(super::render_utils::sanitize(&text));
                            }
                        }
                    }
                }
                Ok(component.render(width, frame))
            }
            Self::Single { details, component, inline,
            } => {
                component.options = options;
                component.context = context.clone();
                if *inline {
                    component.check_selected_header(g)?;
                    Ok(component.render(width, frame))
                } else {
                    component.render_single_in(g, details.as_ref(), width, frame)
                }
            }
            Self::Multi { array, components, shared, remaining, cached, theme, .. } => {
                {
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    state.options = options;
                    state.context = context.clone();
                    state.frame = *frame;
                }
                let length = g.0.heap().array_length(array, g.1)?;
                let key = (options.expanded, width, length, *remaining);
                if let Some((old, rows)) = cached {
                    if *old == key {
                        return Ok(rows.clone());
                    }
                }
                let mut rows = Vec::new();
                let mut index = 0;
                while index < g.0.heap().array_length(components, g.1)? {
                    if index > 0 {
                        rows.push(String::new())
                    }
                    let component = g.get(components, &index.to_string())?;
                    let method = g.get(&component, "render")?;
                    let output = g.0.heap().call(&method, &component, &[g.0.heap().number(width as f64)?], g.1)?;
                    for row in g.members(&output)? {
                        rows.push(super::render_utils::sanitize(&g.string(&row)?));
                    }
                    index += 1;
                }
                if *remaining > 0 {
                    if !rows.is_empty() {
                        rows.push(String::new())
                    }
                    let icon = options.spinner_frame.map(|s| theme.status("running", Some(s)));
                    rows.push(super::output_block::status_line(
                        "Edit",
                        &theme.fg(
                            "dim",
                            &format!("{remaining} more file{} pending…", if *remaining > 1 { "s" } else { "" }),
                        ),
                        icon.as_deref(),
                        theme,
                    ));
                }
                *cached = Some((key, rows.clone()));
                Ok(rows)
            }
        }
    }
    pub(crate) fn invalidate_in(&mut self, g: &DisplayGraph<'_>) -> Result<()> {
        match self {
            Self::Multi { components, cached, .. } => {
                *cached = None;
                for component in g.members(components)? {
                    let method = g.get(&component, "invalidate")?;
                    if !g.nullish(&method)? {
                        g.0.heap().call(&method, &component, &[], g.1)?;
                    }
                }
            }
            _ => self.invalidate(),
        }
        Ok(())
    }
    pub(crate) fn invalidate(&mut self) {
        match self {
            Self::Call { component, .. } | Self::Single { component, .. } => component.invalidate(),
            Self::Multi { children, cached, .. } => {
                *cached = None;
                for (_, _, c) in children {
                    c.invalidate()
                }
            }
        }
    }
}
