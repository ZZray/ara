//! Shared value graph for fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! OMP sources: utils/index.ts (structuredCloneJSON), utils/json.ts,
//! utils/json-parse.ts, and ai/utils/validation.ts's shallow operations (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//!
//! External leases own the arena. All internal edges are NodeId, never Arc,
//! closures, runtime resolvers or foreign handles. This is not a JS interpreter
//! or a collector: unreachable nodes remain until the last arena lease drops.

mod compiled;
mod construct;
mod copy;
mod deep_equals;
mod deferred;
mod encode;
mod guarded;
mod instance;
mod iterate;
mod loop_guard_dependencies;
mod map;
mod module_values;
mod normalization;
mod numeric_methods;
mod property;
mod regexp;
mod string_methods;
mod weak_set;

pub use compiled::{
    CompiledDerivedConstructorBody, CompiledFunctionBody, CompiledFunctionForm, CompiledFunctionKey,
    CompiledFunctionSpec,
};
pub use deferred::DeferredValue;
pub use iterate::{IteratorRecord, LocalValueCallback};

pub use guarded::{
    CaptureOwnControls, CaptureOwnRecords, CapturedOwnControls, CapturedOwnRecord, CapturedOwnScalar, DeferredProperty,
    GuardedFacts, GuardedPropertyError, GuardedPropertyOp, GuardedPropertyStep, GuardedPropertyValue,
    OrderedOwnRecords, OwnScalar, OwnScalarFact,
};

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::json::{Utf16JsonError, WireString, WireValue};

pub type ValueResult<T> = Result<T, ValueError>;

static NEXT_HEAP_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CapabilityId(pub u64);

/// Symbol key identity is non-recycled heap generation plus node identity.
/// It owns no arena lease and is checked at every heap admission point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolKey {
    heap_token: u64,
    node: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    String(WireString),
    Symbol(SymbolKey),
}

impl From<&str> for PropertyKey {
    fn from(value: &str) -> Self {
        Self::String(WireString::from(value))
    }
}
impl From<WireString> for PropertyKey {
    fn from(value: WireString) -> Self {
        Self::String(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    BigInt,
    Object,
    Array,
    Function,
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringifyMode {
    Plain,
    BigIntReplacer,
}
#[derive(Debug, Clone)]
pub enum JsonSpace {
    Number(f64),
    String(WireString),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpaqueKind {
    Object,
    Array,
    Function,
    Symbol,
    BoxedNumber,
    BoxedString,
    BoxedBoolean,
    BoxedBigInt,
}

impl OpaqueKind {
    fn kind(self) -> ValueKind {
        match self {
            Self::Array => ValueKind::Array,
            Self::Function => ValueKind::Function,
            Self::Symbol => ValueKind::Symbol,
            _ => ValueKind::Object,
        }
    }
}

/// Exact canonical signed decimal BigInt, never narrowed to a machine integer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigIntDecimal(String);

impl BigIntDecimal {
    pub fn parse(text: &str) -> ValueResult<Self> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ValueError::TypeError("BigInt decimal contains a non-digit".into()));
        }
        let digits = digits.trim_start_matches('0');
        if digits.is_empty() {
            return Ok(Self("0".into()));
        }
        Ok(Self(if negative { format!("-{digits}") } else { digits.into() }))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intrinsic {
    PrototypeGet,
    PrototypeSet,
    ObjectToString,
    ObjectValueOf,
    FunctionHasInstance,
    FunctionCall,
    ArrayToString,
    ArrayJoin,
    ArrayPush,
    ArraySlice,
    ArraySplice,
    ArrayAt,
    ArrayFind,
    ArraySome,
    ArrayEvery,
    ArrayFilter,
    ArrayMap,
    ArrayFlatMap,
    ArrayShift,
    ArrayPop,
    ArraySort,
    ArrayIncludes,
    ArrayValues,
    ArrayIteratorNext,
    IteratorSelf,
    SetAdd,
    SetHas,
    SetDelete,
    SetClear,
    WeakSetAdd,
    WeakSetHas,
    WeakSetDelete,
    SetSizeGet,
    SetValues,
    SetIteratorNext,
    MapGet,
    MapSet,
    MapHas,
    MapDelete,
    MapClear,
    MapSizeGet,
    MapKeys,
    MapValues,
    MapEntries,
    MapIteratorNext,
    NumberToString,
    BigIntToString,
    StringTrim,
    StringStartsWith,
    StringEndsWith,
    StringSlice,
    StringTrimStart,
    StringTrimEnd,
    StringToLowerCase,
    StringToUpperCase,
    StringIncludes,
    StringIndexOf,
    StringLastIndexOf,
    StringSplit,
    StringReplace,
    StringReplaceAll,
    StringToWellFormed,
    StringCharCodeAt,
    StringPadStart,
    StringRepeat,
    RegExpExec,
    RegExpTest,
    RegExpToString,
    RegExpReplace,
    RegExpSplit,
    RegExpMatch,
    RegExpGlobalGet,
    RegExpIgnoreCaseGet,
    RegExpDotAllGet,
    RegExpStickyGet,
    RegExpUnicodeGet,
    RegExpFalseFlagGet,
    RegExpSourceGet,
    RegExpFlagsGet,
    PrimitiveToString,
    PrimitiveValueOf,
    SpeciesGet,
    ObjectConstructor,
    ArrayConstructor,
    SetConstructor,
    MapConstructor,
    /// Truthy native inherited member; invocation belongs to the Host.
    NativeMember(CapabilityId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableTarget {
    Host(CapabilityId),
    Intrinsic(Intrinsic),
    /// Operation-scoped callback identity; no closure is stored in the heap.
    LocalCallback(u64),
    /// Capture-free persistent callback; uses the invoking runtime for Get.
    ArgumentProperty {
        index: usize,
        key: &'static str,
    },
    /// Trusted persistent body and captures; key construction is heap-private.
    Compiled(CompiledFunctionKey),
}

#[derive(Debug, Clone)]
pub enum ValueSnapshot {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(WireString),
    BigInt(BigIntDecimal),
    Object,
    Array,
    Function(CallableTarget),
    Symbol { key: SymbolKey, description: Option<WireString> },
    Opaque { kind: OpaqueKind, capability: CapabilityId, semantic_identity: u64 },
}

#[derive(Debug, Clone)]
pub enum PropertyPresence {
    Missing,
    Present(ValueHandle),
}

#[derive(Debug, Clone)]
pub enum PropertyDescriptor {
    Data { value: ValueHandle, writable: bool, enumerable: bool, configurable: bool },
    Accessor { get: Option<CallableTarget>, set: Option<CallableTarget>, enumerable: bool, configurable: bool },
}

impl PropertyDescriptor {
    pub fn data(value: ValueHandle) -> Self {
        Self::Data { value, writable: true, enumerable: true, configurable: true }
    }
    pub fn enumerable(&self) -> bool {
        match self {
            Self::Data { enumerable, .. } | Self::Accessor { enumerable, .. } => *enumerable,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ValueError {
    Thrown(ValueHandle),
    /// A prior throw and a non-Native iterator-close failure both remain visible.
    IteratorClose {
        original: Box<ValueError>,
        close: Box<ValueError>,
    },
    TypeError(String),
    SyntaxError(String),
    RangeError(String),
    DataCloneError(String),
    ForeignHeap,
    InvalidNode,
    HeapTokensExhausted,
    NodeIdsExhausted,
    PoisonedHeap,
    SourceIdentityCollision,
    Allocation,
    MissingCapability(CapabilityId),
    ExpiredCapability(CapabilityId),
    WrongRuntimeReply(&'static str),
    UnsupportedOperation(&'static str),
    LegacyBridge(&'static str),
    StrictParse(Utf16JsonError),
}

impl ValueError {
    /// Fixed source catches JavaScript exceptions, never unsupported Host or
    /// carrier boundaries. A failed native clone may enter JSON fallback;
    /// capability/identity/allocation failures must remain visible to callers.
    pub fn is_native_exception(&self) -> bool {
        matches!(
            self,
            Self::Thrown(_) | Self::TypeError(_) | Self::SyntaxError(_) | Self::RangeError(_) | Self::DataCloneError(_)
        )
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thrown(value) => write!(formatter, "Runtime threw {value:?}"),
            Self::TypeError(text) => write!(formatter, "TypeError: {text}"),
            Self::SyntaxError(text) => write!(formatter, "SyntaxError: {text}"),
            Self::RangeError(text) => write!(formatter, "RangeError: {text}"),
            Self::DataCloneError(text) => write!(formatter, "DataCloneError: {text}"),
            Self::MissingCapability(cap) => write!(formatter, "MissingCapability({})", cap.0),
            Self::ExpiredCapability(cap) => write!(formatter, "ExpiredCapability({})", cap.0),
            Self::StrictParse(error) => fmt::Display::fmt(error, formatter),
            other => write!(formatter, "{other:?}"),
        }
    }
}
impl std::error::Error for ValueError {}

pub enum OpaqueOperation<'a> {
    OwnDescriptor(&'a PropertyKey),
    OwnKeys,
    Prototype,
    Get(&'a PropertyKey),
    Has(&'a PropertyKey),
    Set {
        key: &'a PropertyKey,
        value: &'a ValueHandle,
    },
    Define {
        key: &'a PropertyKey,
        descriptor: &'a PropertyDescriptor,
    },
    Delete(&'a PropertyKey),
    SetPrototype(Option<&'a ValueHandle>),
    SetExtensible(bool),
    IsExtensible,
    ArrayLength,
    StructuredClone,
    Unbox,
    ToPrimitive {
        string_hint: bool,
    },
    ToPrimitiveDefault,
    /// Runtime owns opaque callable internal ordinary-instanceof semantics.
    OrdinaryHasInstance(&'a ValueHandle),
}

pub enum RuntimeRequest<'a> {
    Call {
        capability: CapabilityId,
        receiver: &'a ValueHandle,
        arguments: &'a [ValueHandle],
    },
    Opaque {
        capability: CapabilityId,
        target: &'a ValueHandle,
        receiver: &'a ValueHandle,
        operation: OpaqueOperation<'a>,
    },
    ConstructArray {
        capability: CapabilityId,
        constructor: &'a ValueHandle,
        length: u32,
    },
}
impl RuntimeRequest<'_> {
    pub fn capability(&self) -> CapabilityId {
        match self {
            Self::Call { capability, .. }
            | Self::Opaque { capability, .. }
            | Self::ConstructArray { capability, .. } => *capability,
        }
    }
}

pub enum RuntimeReply {
    Value(ValueHandle),
    Descriptor(Option<PropertyDescriptor>),
    Keys(Vec<PropertyKey>),
    Prototype(Option<ValueHandle>),
    Boolean(bool),
    Length(u32),
    Unit,
}

/// Resolver is supplied to an operation, never stored by ValueHeap.
pub trait ValueRuntime: Send + Sync {
    fn resolve(&self, request: RuntimeRequest<'_>) -> ValueResult<RuntimeReply>;

    /// Opaque Hosts invoking callbacks preserve the supplied effective scope.
    /// Existing non-callback Hosts retain their original resolver behavior.
    fn resolve_effective(
        &self,
        request: RuntimeRequest<'_>,
        _effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<RuntimeReply> {
        self.resolve(request)
    }

    /// Hosts invoking a callback supplied to this call use `effective_runtime`.
    /// The default preserves existing Hosts that do not invoke supplied callbacks.
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        _effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<RuntimeReply> {
        self.resolve(RuntimeRequest::Call { capability, receiver, arguments })
    }

    /// Query the selected Host constructor without losing the invoking scope.
    /// An unimplemented Host boundary is not a known nonconstructor.
    fn resolve_is_constructor(
        &self,
        _capability: CapabilityId,
        _constructor: &ValueHandle,
        _effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        Err(ValueError::UnsupportedOperation("Host IsConstructor is not installed"))
    }

    /// Construct with newTarget equal to constructor. Full newTarget semantics
    /// require a separate API and are not silently approximated here.
    fn resolve_construct(
        &self,
        _capability: CapabilityId,
        _constructor: &ValueHandle,
        _arguments: &[ValueHandle],
        _effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        Err(ValueError::UnsupportedOperation("Host Construct is not installed"))
    }

    /// Explicit Host super/newTarget seam. Existing equal-target Hosts retain
    /// their behavior; a distinct-target Host must implement its original
    /// construction instead of silently falling back to equal-target Construct.
    fn resolve_construct_with_new_target(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        if constructor.same_identity(new_target) {
            self.resolve_construct(capability, constructor, arguments, effective_runtime)
        } else {
            Err(ValueError::UnsupportedOperation("Host distinct newTarget Construct is not installed"))
        }
    }

    fn resolve_local_call(
        &self,
        _token: u64,
        _receiver: &ValueHandle,
        _arguments: &[ValueHandle],
        _effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<RuntimeReply> {
        Err(ValueError::UnsupportedOperation("Operation-local callback scope is unavailable or expired"))
    }
}
pub struct NoRuntime;
impl ValueRuntime for NoRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> ValueResult<RuntimeReply> {
        Err(ValueError::MissingCapability(request.capability()))
    }
}

#[derive(Clone)]
pub struct ValueHandle {
    heap: Arc<ValueHeap>,
    node: NodeId,
}
impl ValueHandle {
    pub fn heap(&self) -> &Arc<ValueHeap> {
        &self.heap
    }
    pub fn node_id(&self) -> NodeId {
        self.node
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.heap.token == other.heap.token && self.node == other.node
    }
}
impl fmt::Debug for ValueHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ValueHandle").field("heap_token", &self.heap.token).field("node", &self.node).finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StoredDescriptor {
    Data { value: NodeId, writable: bool, enumerable: bool, configurable: bool },
    Accessor { get: Option<CallableTarget>, set: Option<CallableTarget>, enumerable: bool, configurable: bool },
}
impl StoredDescriptor {
    fn enumerable(&self) -> bool {
        match self {
            Self::Data { enumerable, .. } | Self::Accessor { enumerable, .. } => *enumerable,
        }
    }
    fn configurable(&self) -> bool {
        match self {
            Self::Data { configurable, .. } | Self::Accessor { configurable, .. } => *configurable,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PropertyEntry {
    key: PropertyKey,
    descriptor: StoredDescriptor,
}
#[derive(Debug, Clone)]
pub(super) struct ObjectData {
    prototype: Option<NodeId>,
    properties: Vec<PropertyEntry>,
    extensible: bool,
}
impl ObjectData {
    fn new(prototype: Option<NodeId>) -> Self {
        Self { prototype, properties: Vec::new(), extensible: true }
    }
}

#[derive(Debug, Clone)]
pub(super) enum Node {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(WireString),
    BigInt(BigIntDecimal),
    Object(ObjectData),
    Array {
        object: ObjectData,
        length: u32,
        length_writable: bool,
    },
    ArrayIterator {
        object: ObjectData,
        target: Option<NodeId>,
        next_index: u64,
    },
    Set {
        object: ObjectData,
        entries: Vec<Option<NodeId>>,
    },
    /// Weak membership tokens; never trace as strong edges when GC exists.
    WeakSet {
        object: ObjectData,
        weak_entries: Vec<NodeId>,
    },
    SetIterator {
        object: ObjectData,
        target: Option<NodeId>,
        next_index: usize,
    },
    Map {
        object: ObjectData,
        entries: Vec<Option<(NodeId, NodeId)>>,
    },
    MapIterator {
        object: ObjectData,
        target: Option<NodeId>,
        next_index: usize,
        flavor: map::MapIteration,
    },
    RegExp {
        object: ObjectData,
        data: regexp::RegExpData,
    },
    Function {
        object: ObjectData,
        target: CallableTarget,
    },
    Symbol {
        description: Option<WireString>,
    },
    Opaque {
        kind: OpaqueKind,
        capability: CapabilityId,
        semantic_identity: u64,
    },
}
impl Node {
    fn kind(&self) -> ValueKind {
        match self {
            Self::Undefined => ValueKind::Undefined,
            Self::Null => ValueKind::Null,
            Self::Boolean(_) => ValueKind::Boolean,
            Self::Number(_) => ValueKind::Number,
            Self::String(_) => ValueKind::String,
            Self::BigInt(_) => ValueKind::BigInt,
            Self::Object(_)
            | Self::ArrayIterator { .. }
            | Self::Set { .. }
            | Self::WeakSet { .. }
            | Self::SetIterator { .. }
            | Self::Map { .. }
            | Self::MapIterator { .. }
            | Self::RegExp { .. } => ValueKind::Object,
            Self::Array { .. } => ValueKind::Array,
            Self::Function { .. } => ValueKind::Function,
            Self::Symbol { .. } => ValueKind::Symbol,
            Self::Opaque { kind, .. } => kind.kind(),
        }
    }
    fn object(&self) -> Option<&ObjectData> {
        match self {
            Self::Object(object)
            | Self::Array { object, .. }
            | Self::ArrayIterator { object, .. }
            | Self::Set { object, .. }
            | Self::WeakSet { object, .. }
            | Self::SetIterator { object, .. }
            | Self::Map { object, .. }
            | Self::MapIterator { object, .. }
            | Self::RegExp { object, .. }
            | Self::Function { object, .. } => Some(object),
            _ => None,
        }
    }
    fn object_mut(&mut self) -> Option<&mut ObjectData> {
        match self {
            Self::Object(object)
            | Self::Array { object, .. }
            | Self::ArrayIterator { object, .. }
            | Self::Set { object, .. }
            | Self::WeakSet { object, .. }
            | Self::SetIterator { object, .. }
            | Self::Map { object, .. }
            | Self::MapIterator { object, .. }
            | Self::RegExp { object, .. }
            | Self::Function { object, .. } => Some(object),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StaticModuleValue {
    RegExp { pattern: &'static str, flags: &'static str },
    StringSet(&'static [&'static str]),
    StringArray(&'static [&'static str]),
    OpenSubschemaWidening,
}

#[derive(Debug)]
pub(super) struct Arena {
    nodes: Vec<Node>,
    // Neutral module-owned symbol IDs only: no domain records or arena leases.
    symbol_registry: HashMap<WireString, NodeId>,
    module_symbols: HashMap<(&'static str, &'static str), NodeId>,
    module_values: HashMap<(&'static str, &'static str), (NodeId, StaticModuleValue)>,
    module_set_initializing: HashMap<&'static str, (std::thread::ThreadId, module_values::StringSetDefinitions)>,
    module_set_failures:
        HashMap<&'static str, (module_values::StringSetDefinitions, module_values::StoredModuleFailure)>,
    module_set_ready: HashMap<&'static str, module_values::StringSetDefinitions>,
    compiled_functions: HashMap<NodeId, compiled::StoredCompiledFunction>,
    module_compiled_functions: HashMap<(&'static str, &'static str), NodeId>,
}
impl Arena {
    fn allocate(&mut self, node: Node) -> ValueResult<NodeId> {
        let id = NodeId(self.nodes.len());
        if id.0 == usize::MAX {
            return Err(ValueError::NodeIdsExhausted);
        }
        self.nodes.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        self.nodes.push(node);
        Ok(id)
    }
    fn get(&self, id: NodeId) -> ValueResult<&Node> {
        self.nodes.get(id.0).ok_or(ValueError::InvalidNode)
    }
    fn get_mut(&mut self, id: NodeId) -> ValueResult<&mut Node> {
        self.nodes.get_mut(id.0).ok_or(ValueError::InvalidNode)
    }
}

#[derive(Debug)]
pub(super) struct Intrinsics {
    object_proto: NodeId,
    function_proto: NodeId,
    array_proto: NodeId,
    string_proto: NodeId,
    number_proto: NodeId,
    boolean_proto: NodeId,
    bigint_proto: NodeId,
    undefined: NodeId,
    null: NodeId,
    species: NodeId,
    to_primitive: NodeId,
    has_instance: NodeId,
    array_constructor: NodeId,
    iterator: NodeId,
    array_iterator_proto: NodeId,
    set_proto: NodeId,
    weak_set_proto: NodeId,
    regexp_proto: NodeId,
    symbol_match: NodeId,
    symbol_replace: NodeId,
    symbol_split: NodeId,
    set_iterator_proto: NodeId,
    map_proto: NodeId,
    map_constructor: NodeId,
    map_iterator_proto: NodeId,
}

#[derive(Debug)]
pub struct ValueHeap {
    arena: Mutex<Arena>,
    token: u64,
    intrinsics: Intrinsics,
    module_initialized: Condvar,
}

impl ValueHeap {
    pub fn new() -> ValueResult<Arc<Self>> {
        let token = NEXT_HEAP_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
            .map_err(|_| ValueError::HeapTokensExhausted)?;
        let mut arena = Arena {
            nodes: Vec::new(),
            symbol_registry: HashMap::new(),
            module_symbols: HashMap::new(),
            module_values: HashMap::new(),
            module_set_initializing: HashMap::new(),
            module_set_failures: HashMap::new(),
            module_set_ready: HashMap::new(),
            compiled_functions: HashMap::new(),
            module_compiled_functions: HashMap::new(),
        };
        let object_proto = arena.allocate(Node::Object(ObjectData::new(None)))?;
        let function_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let array_proto = arena.allocate(Node::Array {
            object: ObjectData::new(Some(object_proto)),
            length: 0,
            length_writable: true,
        })?;
        let string_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let number_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let boolean_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let bigint_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let undefined = arena.allocate(Node::Undefined)?;
        let null = arena.allocate(Node::Null)?;
        let species = arena.allocate(Node::Symbol { description: Some("Symbol.species".into()) })?;
        let to_primitive = arena.allocate(Node::Symbol { description: Some("Symbol.toPrimitive".into()) })?;
        let has_instance = arena.allocate(Node::Symbol { description: Some("Symbol.hasInstance".into()) })?;
        let iterator = arena.allocate(Node::Symbol { description: Some("Symbol.iterator".into()) })?;
        let iterator_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let array_iterator_proto = arena.allocate(Node::Object(ObjectData::new(Some(iterator_proto))))?;
        let set_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let weak_set_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let symbol_split = arena.allocate(Node::Symbol { description: Some("Symbol.split".into()) })?;
        let (regexp_proto, symbol_match, symbol_replace) =
            regexp::bootstrap_regexp(&mut arena, token, object_proto, function_proto, symbol_split)?;
        let set_iterator_proto = arena.allocate(Node::Object(ObjectData::new(Some(iterator_proto))))?;
        let map_proto = arena.allocate(Node::Object(ObjectData::new(Some(object_proto))))?;
        let map_iterator_proto = arena.allocate(Node::Object(ObjectData::new(Some(iterator_proto))))?;
        let object_constructor = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::ObjectConstructor),
        })?;
        let array_constructor = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::ArrayConstructor),
        })?;
        let set_constructor = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::SetConstructor),
        })?;
        let map_constructor = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::MapConstructor),
        })?;
        fn add(arena: &mut Arena, object: NodeId, key: PropertyKey, descriptor: StoredDescriptor) -> ValueResult<()> {
            let object = arena.get_mut(object)?.object_mut().ok_or(ValueError::InvalidNode)?;
            object.properties.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            object.properties.push(PropertyEntry { key, descriptor });
            Ok(())
        }
        fn data(value: NodeId) -> StoredDescriptor {
            StoredDescriptor::Data { value, writable: true, enumerable: false, configurable: true }
        }
        let instance_method = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::FunctionHasInstance),
        })?;
        let instance_name = arena.allocate(Node::String("[Symbol.hasInstance]".into()))?;
        let instance_length = arena.allocate(Node::Number(1.0))?;
        for (key, value) in [("name", instance_name), ("length", instance_length)] {
            add(
                &mut arena,
                instance_method,
                key.into(),
                StoredDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
            )?;
        }
        add(
            &mut arena,
            function_proto,
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: has_instance }),
            StoredDescriptor::Data { value: instance_method, writable: false, enumerable: false, configurable: false },
        )?;
        add(&mut arena, object_proto, "constructor".into(), data(object_constructor))?;
        add(&mut arena, array_proto, "constructor".into(), data(array_constructor))?;
        add(&mut arena, set_proto, "constructor".into(), data(set_constructor))?;
        add(&mut arena, map_proto, "constructor".into(), data(map_constructor))?;
        add(
            &mut arena,
            map_proto,
            "size".into(),
            StoredDescriptor::Accessor {
                get: Some(CallableTarget::Intrinsic(Intrinsic::MapSizeGet)),
                set: None,
                enumerable: false,
                configurable: true,
            },
        )?;
        add(
            &mut arena,
            map_constructor,
            "prototype".into(),
            StoredDescriptor::Data { value: map_proto, writable: false, enumerable: false, configurable: false },
        )?;
        add(
            &mut arena,
            set_proto,
            "size".into(),
            StoredDescriptor::Accessor {
                get: Some(CallableTarget::Intrinsic(Intrinsic::SetSizeGet)),
                set: None,
                enumerable: false,
                configurable: true,
            },
        )?;
        add(
            &mut arena,
            set_constructor,
            "prototype".into(),
            StoredDescriptor::Data { value: set_proto, writable: false, enumerable: false, configurable: false },
        )?;
        add(
            &mut arena,
            object_constructor,
            "prototype".into(),
            StoredDescriptor::Data { value: object_proto, writable: false, enumerable: false, configurable: false },
        )?;
        add(
            &mut arena,
            array_constructor,
            "prototype".into(),
            StoredDescriptor::Data { value: array_proto, writable: false, enumerable: false, configurable: false },
        )?;
        add(
            &mut arena,
            object_proto,
            "__proto__".into(),
            StoredDescriptor::Accessor {
                get: Some(CallableTarget::Intrinsic(Intrinsic::PrototypeGet)),
                set: Some(CallableTarget::Intrinsic(Intrinsic::PrototypeSet)),
                enumerable: false,
                configurable: true,
            },
        )?;
        add(
            &mut arena,
            array_constructor,
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: species }),
            StoredDescriptor::Accessor {
                get: Some(CallableTarget::Intrinsic(Intrinsic::SpeciesGet)),
                set: None,
                enumerable: false,
                configurable: true,
            },
        )?;
        let methods = [
            (function_proto, "call", Intrinsic::FunctionCall),
            (array_proto, "pop", Intrinsic::ArrayPop),
            (array_proto, "sort", Intrinsic::ArraySort),
            (object_proto, "toString", Intrinsic::ObjectToString),
            (object_proto, "valueOf", Intrinsic::ObjectValueOf),
            (object_proto, "toLocaleString", Intrinsic::NativeMember(CapabilityId(0xffff_0001))),
            (object_proto, "hasOwnProperty", Intrinsic::NativeMember(CapabilityId(0xffff_0002))),
            (object_proto, "isPrototypeOf", Intrinsic::NativeMember(CapabilityId(0xffff_0003))),
            (object_proto, "propertyIsEnumerable", Intrinsic::NativeMember(CapabilityId(0xffff_0004))),
            (object_proto, "__defineGetter__", Intrinsic::NativeMember(CapabilityId(0xffff_0005))),
            (object_proto, "__defineSetter__", Intrinsic::NativeMember(CapabilityId(0xffff_0006))),
            (object_proto, "__lookupGetter__", Intrinsic::NativeMember(CapabilityId(0xffff_0007))),
            (object_proto, "__lookupSetter__", Intrinsic::NativeMember(CapabilityId(0xffff_0008))),
            (array_proto, "toString", Intrinsic::ArrayToString),
            (array_proto, "join", Intrinsic::ArrayJoin),
            (array_proto, "push", Intrinsic::ArrayPush),
            (array_proto, "slice", Intrinsic::ArraySlice),
            (array_proto, "splice", Intrinsic::ArraySplice),
            (array_proto, "at", Intrinsic::ArrayAt),
            (array_proto, "find", Intrinsic::ArrayFind),
            (array_proto, "some", Intrinsic::ArraySome),
            (array_proto, "every", Intrinsic::ArrayEvery),
            (array_proto, "filter", Intrinsic::ArrayFilter),
            (array_proto, "map", Intrinsic::ArrayMap),
            (array_proto, "flatMap", Intrinsic::ArrayFlatMap),
            (array_proto, "shift", Intrinsic::ArrayShift),
            (array_proto, "includes", Intrinsic::ArrayIncludes),
            (array_iterator_proto, "next", Intrinsic::ArrayIteratorNext),
            (string_proto, "toString", Intrinsic::PrimitiveToString),
            (string_proto, "valueOf", Intrinsic::PrimitiveValueOf),
            (string_proto, "trim", Intrinsic::StringTrim),
            (string_proto, "startsWith", Intrinsic::StringStartsWith),
            (string_proto, "endsWith", Intrinsic::StringEndsWith),
            (string_proto, "slice", Intrinsic::StringSlice),
            (string_proto, "trimStart", Intrinsic::StringTrimStart),
            (string_proto, "trimEnd", Intrinsic::StringTrimEnd),
            (string_proto, "toLowerCase", Intrinsic::StringToLowerCase),
            (string_proto, "toUpperCase", Intrinsic::StringToUpperCase),
            (string_proto, "includes", Intrinsic::StringIncludes),
            (string_proto, "indexOf", Intrinsic::StringIndexOf),
            (string_proto, "lastIndexOf", Intrinsic::StringLastIndexOf),
            (string_proto, "split", Intrinsic::StringSplit),
            (string_proto, "replace", Intrinsic::StringReplace),
            (string_proto, "replaceAll", Intrinsic::StringReplaceAll),
            (string_proto, "toWellFormed", Intrinsic::StringToWellFormed),
            (string_proto, "charCodeAt", Intrinsic::StringCharCodeAt),
            (string_proto, "padStart", Intrinsic::StringPadStart),
            (string_proto, "repeat", Intrinsic::StringRepeat),
            (set_proto, "add", Intrinsic::SetAdd),
            (set_proto, "has", Intrinsic::SetHas),
            (set_proto, "delete", Intrinsic::SetDelete),
            (set_proto, "clear", Intrinsic::SetClear),
            (weak_set_proto, "add", Intrinsic::WeakSetAdd),
            (weak_set_proto, "has", Intrinsic::WeakSetHas),
            (weak_set_proto, "delete", Intrinsic::WeakSetDelete),
            (set_iterator_proto, "next", Intrinsic::SetIteratorNext),
            (map_proto, "get", Intrinsic::MapGet),
            (map_proto, "set", Intrinsic::MapSet),
            (map_proto, "has", Intrinsic::MapHas),
            (map_proto, "delete", Intrinsic::MapDelete),
            (map_proto, "clear", Intrinsic::MapClear),
            (map_proto, "keys", Intrinsic::MapKeys),
            (map_proto, "values", Intrinsic::MapValues),
            (map_iterator_proto, "next", Intrinsic::MapIteratorNext),
            (number_proto, "toString", Intrinsic::NumberToString),
            (number_proto, "valueOf", Intrinsic::PrimitiveValueOf),
            (boolean_proto, "toString", Intrinsic::PrimitiveToString),
            (boolean_proto, "valueOf", Intrinsic::PrimitiveValueOf),
            (bigint_proto, "toString", Intrinsic::BigIntToString),
            (bigint_proto, "valueOf", Intrinsic::PrimitiveValueOf),
        ];
        for (owner, name, intrinsic) in methods {
            let function = arena.allocate(Node::Function {
                object: ObjectData::new(Some(function_proto)),
                target: CallableTarget::Intrinsic(intrinsic),
            })?;
            if matches!(
                intrinsic,
                Intrinsic::FunctionCall
                    | Intrinsic::ArrayPop
                    | Intrinsic::ArraySort
                    | Intrinsic::ArrayAt
                    | Intrinsic::ArrayFind
                    | Intrinsic::SetAdd
                    | Intrinsic::SetHas
                    | Intrinsic::SetDelete
                    | Intrinsic::SetClear
                    | Intrinsic::WeakSetAdd
                    | Intrinsic::WeakSetHas
                    | Intrinsic::WeakSetDelete
                    | Intrinsic::StringTrim
                    | Intrinsic::StringStartsWith
                    | Intrinsic::StringEndsWith
                    | Intrinsic::StringSlice
                    | Intrinsic::StringTrimStart
                    | Intrinsic::StringTrimEnd
                    | Intrinsic::StringToLowerCase
                    | Intrinsic::StringToUpperCase
                    | Intrinsic::StringIncludes
                    | Intrinsic::StringIndexOf
                    | Intrinsic::StringLastIndexOf
                    | Intrinsic::StringSplit
                    | Intrinsic::StringReplace
                    | Intrinsic::StringReplaceAll
                    | Intrinsic::StringToWellFormed
                    | Intrinsic::StringCharCodeAt
                    | Intrinsic::StringPadStart
                    | Intrinsic::StringRepeat
                    | Intrinsic::ArrayMap
                    | Intrinsic::ArrayFlatMap
                    | Intrinsic::ArrayShift
                    | Intrinsic::MapGet
                    | Intrinsic::MapSet
                    | Intrinsic::MapHas
                    | Intrinsic::MapDelete
                    | Intrinsic::MapClear
                    | Intrinsic::MapKeys
                    | Intrinsic::MapValues
                    | Intrinsic::MapIteratorNext
                    | Intrinsic::NumberToString
                    | Intrinsic::BigIntToString
            ) {
                let length = arena.allocate(Node::Number(match intrinsic {
                    Intrinsic::ArrayPop
                    | Intrinsic::StringTrim
                    | Intrinsic::StringTrimStart
                    | Intrinsic::StringTrimEnd
                    | Intrinsic::StringToLowerCase
                    | Intrinsic::StringToUpperCase
                    | Intrinsic::StringToWellFormed
                    | Intrinsic::BigIntToString
                    | Intrinsic::ArrayShift
                    | Intrinsic::SetClear
                    | Intrinsic::MapClear
                    | Intrinsic::MapKeys
                    | Intrinsic::MapValues
                    | Intrinsic::MapIteratorNext => 0.0,
                    Intrinsic::StringSlice
                    | Intrinsic::StringSplit
                    | Intrinsic::StringReplace
                    | Intrinsic::StringReplaceAll
                    | Intrinsic::MapSet => 2.0,
                    _ => 1.0,
                }))?;
                let function_name = arena.allocate(Node::String(name.into()))?;
                for (key, value) in [("length", length), ("name", function_name)] {
                    add(
                        &mut arena,
                        function,
                        key.into(),
                        StoredDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
                    )?;
                }
            }
            add(&mut arena, owner, name.into(), data(function))?;
        }
        let values = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::ArrayValues),
        })?;
        add(&mut arena, array_proto, "values".into(), data(values))?;
        add(
            &mut arena,
            array_proto,
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: iterator }),
            data(values),
        )?;
        let set_values = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::SetValues),
        })?;
        for key in [
            PropertyKey::from("values"),
            PropertyKey::from("keys"),
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: iterator }),
        ] {
            add(&mut arena, set_proto, key, data(set_values))?;
        }
        let map_entries = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::MapEntries),
        })?;
        let entries_name = arena.allocate(Node::String("entries".into()))?;
        let entries_length = arena.allocate(Node::Number(0.0))?;
        for (key, value) in [("name", entries_name), ("length", entries_length)] {
            add(
                &mut arena,
                map_entries,
                key.into(),
                StoredDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
            )?;
        }
        add(&mut arena, map_proto, "entries".into(), data(map_entries))?;
        add(
            &mut arena,
            map_proto,
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: iterator }),
            data(map_entries),
        )?;
        let iterator_self = arena.allocate(Node::Function {
            object: ObjectData::new(Some(function_proto)),
            target: CallableTarget::Intrinsic(Intrinsic::IteratorSelf),
        })?;
        add(
            &mut arena,
            iterator_proto,
            PropertyKey::Symbol(SymbolKey { heap_token: token, node: iterator }),
            data(iterator_self),
        )?;
        Ok(Arc::new(Self {
            arena: Mutex::new(arena),
            module_initialized: Condvar::new(),
            token,
            intrinsics: Intrinsics {
                object_proto,
                function_proto,
                array_proto,
                string_proto,
                number_proto,
                boolean_proto,
                bigint_proto,
                undefined,
                null,
                species,
                to_primitive,
                has_instance,
                array_constructor,
                iterator,
                array_iterator_proto,
                set_proto,
                weak_set_proto,
                regexp_proto,
                symbol_match,
                symbol_replace,
                symbol_split,
                set_iterator_proto,
                map_proto,
                map_constructor,
                map_iterator_proto,
            },
        }))
    }
    pub fn heap_token(&self) -> u64 {
        self.token
    }
    fn lock(&self) -> ValueResult<MutexGuard<'_, Arena>> {
        self.arena.lock().map_err(|_| ValueError::PoisonedHeap)
    }
    fn handle(self: &Arc<Self>, node: NodeId) -> ValueHandle {
        ValueHandle { heap: Arc::clone(self), node }
    }
    fn admit(&self, value: &ValueHandle) -> ValueResult<NodeId> {
        if self.token != value.heap.token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(value.node)
    }
    fn admit_key(&self, key: &PropertyKey) -> ValueResult<()> {
        if matches!(key, PropertyKey::String(_)) {
            return Ok(());
        }
        let arena = self.lock()?;
        self.admit_key_locked(&arena, key)
    }
    fn admit_key_locked(&self, arena: &Arena, key: &PropertyKey) -> ValueResult<()> {
        if let PropertyKey::Symbol(symbol) = key {
            if symbol.heap_token != self.token {
                return Err(ValueError::ForeignHeap);
            }
            if !matches!(arena.get(symbol.node)?, Node::Symbol { .. } | Node::Opaque { kind: OpaqueKind::Symbol, .. }) {
                return Err(ValueError::InvalidNode);
            }
        }
        Ok(())
    }
    fn allocate(self: &Arc<Self>, node: Node) -> ValueResult<ValueHandle> {
        let id = self.lock()?.allocate(node)?;
        Ok(self.handle(id))
    }
    pub fn adopt(self: &Arc<Self>, value: &ValueHandle) -> ValueResult<ValueHandle> {
        let id = self.admit(value)?;
        self.lock()?.get(id)?;
        Ok(self.handle(id))
    }
    pub fn undefined(self: &Arc<Self>) -> ValueResult<ValueHandle> {
        Ok(self.handle(self.intrinsics.undefined))
    }
    pub fn null(self: &Arc<Self>) -> ValueResult<ValueHandle> {
        Ok(self.handle(self.intrinsics.null))
    }
    pub fn boolean(self: &Arc<Self>, value: bool) -> ValueResult<ValueHandle> {
        self.allocate(Node::Boolean(value))
    }
    pub fn number(self: &Arc<Self>, value: f64) -> ValueResult<ValueHandle> {
        self.allocate(Node::Number(value))
    }
    pub fn string(self: &Arc<Self>, value: WireString) -> ValueResult<ValueHandle> {
        self.allocate(Node::String(value))
    }
    pub fn bigint_decimal(self: &Arc<Self>, value: &str) -> ValueResult<ValueHandle> {
        self.allocate(Node::BigInt(BigIntDecimal::parse(value)?))
    }
    pub fn object(self: &Arc<Self>) -> ValueResult<ValueHandle> {
        self.allocate(Node::Object(ObjectData::new(Some(self.intrinsics.object_proto))))
    }
    pub fn object_with_prototype(self: &Arc<Self>, prototype: Option<&ValueHandle>) -> ValueResult<ValueHandle> {
        let prototype = match prototype {
            Some(value) => Some(self.object_id(value)?),
            None => None,
        };
        self.allocate(Node::Object(ObjectData::new(prototype)))
    }
    pub fn array_with_length(self: &Arc<Self>, length: u32) -> ValueResult<ValueHandle> {
        self.allocate(Node::Array {
            object: ObjectData::new(Some(self.intrinsics.array_proto)),
            length,
            length_writable: true,
        })
    }
    pub fn array(self: &Arc<Self>, values: Vec<ValueHandle>) -> ValueResult<ValueHandle> {
        let length = u32::try_from(values.len()).map_err(|_| ValueError::RangeError("Invalid array length".into()))?;
        for value in &values {
            self.admit(value)?;
        }
        let mut object = ObjectData::new(Some(self.intrinsics.array_proto));
        object.properties.try_reserve(values.len()).map_err(|_| ValueError::Allocation)?;
        for (index, value) in values.into_iter().enumerate() {
            object.properties.push(PropertyEntry {
                key: index_key(index as u32),
                descriptor: StoredDescriptor::Data {
                    value: value.node,
                    writable: true,
                    enumerable: true,
                    configurable: true,
                },
            });
        }
        self.allocate(Node::Array { object, length, length_writable: true })
    }
    pub fn function(self: &Arc<Self>, capability: CapabilityId) -> ValueResult<ValueHandle> {
        self.allocate(Node::Function {
            object: ObjectData::new(Some(self.intrinsics.function_proto)),
            target: CallableTarget::Host(capability),
        })
    }
    pub fn symbol(self: &Arc<Self>, description: Option<WireString>) -> ValueResult<ValueHandle> {
        self.allocate(Node::Symbol { description })
    }
    pub fn symbol_key(self: &Arc<Self>, value: &ValueHandle) -> ValueResult<SymbolKey> {
        let node = self.admit(value)?;
        if self.kind(value)? != ValueKind::Symbol {
            return Err(ValueError::TypeError("Value is not a symbol".into()));
        }
        Ok(SymbolKey { heap_token: self.token, node })
    }
    /// Actual realm-global Symbol.for registry; distinct from module/name keys.
    pub fn symbol_for(self: &Arc<Self>, name: WireString) -> ValueResult<ValueHandle> {
        let mut arena = self.lock()?;
        if let Some(node) = arena.symbol_registry.get(&name).copied() {
            return Ok(self.handle(node));
        }
        arena.symbol_registry.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let node = arena.allocate(Node::Symbol { description: Some(name.clone()) })?;
        arena.symbol_registry.insert(name, node);
        Ok(self.handle(node))
    }
    /// Stable symbol identity scoped to this heap and a module/name pair.
    /// Creation and publication share one guard; no Host is called under it.
    pub fn module_symbol(self: &Arc<Self>, module: &'static str, name: &'static str) -> ValueResult<PropertyKey> {
        let mut arena = self.lock()?;
        let key = (module, name);
        if let Some(node) = arena.module_symbols.get(&key).copied() {
            return Ok(PropertyKey::Symbol(SymbolKey { heap_token: self.token, node }));
        }
        arena.module_symbols.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let node = arena.allocate(Node::Symbol { description: Some(name.into()) })?;
        arena.module_symbols.insert(key, node);
        Ok(PropertyKey::Symbol(SymbolKey { heap_token: self.token, node }))
    }
    pub fn species_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.species })
    }
    pub fn to_primitive_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.to_primitive })
    }
    pub fn opaque(
        self: &Arc<Self>,
        kind: OpaqueKind,
        capability: CapabilityId,
        semantic_identity: u64,
    ) -> ValueResult<ValueHandle> {
        let mut arena = self.lock()?;
        if arena.nodes.iter().any(|node| matches!(node, Node::Opaque { kind: existing_kind, capability: existing_capability, semantic_identity: existing_identity } if *existing_kind == kind && *existing_capability == capability && *existing_identity == semantic_identity)) {
            return Err(ValueError::SourceIdentityCollision);
        }
        let node = arena.allocate(Node::Opaque { kind, capability, semantic_identity })?;
        Ok(self.handle(node))
    }
    pub fn object_prototype(self: &Arc<Self>) -> ValueHandle {
        self.handle(self.intrinsics.object_proto)
    }
    pub fn array_prototype(self: &Arc<Self>) -> ValueHandle {
        self.handle(self.intrinsics.array_proto)
    }
    pub fn bigint_prototype(self: &Arc<Self>) -> ValueHandle {
        self.handle(self.intrinsics.bigint_proto)
    }
    /// The realm's original intrinsic, independent of a writable prototype.constructor.
    pub fn map_constructor(self: &Arc<Self>) -> ValueHandle {
        self.handle(self.intrinsics.map_constructor)
    }
    pub fn map_intrinsic_prototype(self: &Arc<Self>) -> ValueHandle {
        self.handle(self.intrinsics.map_proto)
    }
    pub fn set_intrinsic_prototype(self: &Arc<Self>) -> ValueHandle {
        self.handle(self.intrinsics.set_proto)
    }
    pub fn kind(&self, value: &ValueHandle) -> ValueResult<ValueKind> {
        Ok(self.lock()?.get(self.admit(value)?)?.kind())
    }
    pub fn snapshot(&self, value: &ValueHandle) -> ValueResult<ValueSnapshot> {
        let node = self.admit(value)?;
        Ok(match self.lock()?.get(node)? {
            Node::Undefined => ValueSnapshot::Undefined,
            Node::Null => ValueSnapshot::Null,
            Node::Boolean(value) => ValueSnapshot::Boolean(*value),
            Node::Number(value) => ValueSnapshot::Number(*value),
            Node::String(value) => ValueSnapshot::String(value.clone()),
            Node::BigInt(value) => ValueSnapshot::BigInt(value.clone()),
            Node::Object(_)
            | Node::ArrayIterator { .. }
            | Node::Set { .. }
            | Node::WeakSet { .. }
            | Node::SetIterator { .. }
            | Node::Map { .. }
            | Node::MapIterator { .. }
            | Node::RegExp { .. } => ValueSnapshot::Object,
            Node::Array { .. } => ValueSnapshot::Array,
            Node::Function { target, .. } => ValueSnapshot::Function(*target),
            Node::Symbol { description } => ValueSnapshot::Symbol {
                key: SymbolKey { heap_token: self.token, node },
                description: description.clone(),
            },
            Node::Opaque { kind, capability, semantic_identity } => {
                ValueSnapshot::Opaque { kind: *kind, capability: *capability, semantic_identity: *semantic_identity }
            }
        })
    }
    fn object_id(&self, value: &ValueHandle) -> ValueResult<NodeId> {
        self.admit(value)?;
        let arena = self.lock()?;
        self.object_id_locked(&arena, value)
    }
    fn object_id_locked(&self, arena: &Arena, value: &ValueHandle) -> ValueResult<NodeId> {
        let node = self.admit(value)?;
        if !matches!(arena.get(node)?.kind(), ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Err(ValueError::TypeError("Value is not an object".into()));
        }
        Ok(node)
    }
    pub fn object_is(&self, left: &ValueHandle, right: &ValueHandle) -> ValueResult<bool> {
        self.admit(left)?;
        self.admit(right)?;
        if left.node == right.node {
            return Ok(true);
        }
        Ok(match (self.snapshot(left)?, self.snapshot(right)?) {
            (ValueSnapshot::Undefined, ValueSnapshot::Undefined) | (ValueSnapshot::Null, ValueSnapshot::Null) => true,
            (ValueSnapshot::Boolean(a), ValueSnapshot::Boolean(b)) => a == b,
            (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => {
                a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
            }
            (ValueSnapshot::String(a), ValueSnapshot::String(b)) => a == b,
            (ValueSnapshot::BigInt(a), ValueSnapshot::BigInt(b)) => a == b,
            _ => false,
        })
    }
    pub fn truthy(&self, value: &ValueHandle) -> ValueResult<bool> {
        Ok(match self.snapshot(value)? {
            ValueSnapshot::Undefined | ValueSnapshot::Null => false,
            ValueSnapshot::Boolean(value) => value,
            ValueSnapshot::Number(value) => value != 0.0 && !value.is_nan(),
            ValueSnapshot::String(value) => !value.is_empty(),
            ValueSnapshot::BigInt(value) => value.as_str() != "0",
            _ => true,
        })
    }
    /// Every resolver exit enters this one boundary before error classification.
    /// A thrown value is still a runtime handle: foreign throws are a carrier
    /// admission failure, not a native exception eligible for successful catch.
    fn runtime_reply(&self, result: ValueResult<RuntimeReply>) -> ValueResult<RuntimeReply> {
        match result {
            Err(error) => Err(self.admit_runtime_error(error)?),
            other => other,
        }
    }
    fn admit_runtime_error(&self, error: ValueError) -> ValueResult<ValueError> {
        match error {
            ValueError::Thrown(value) => {
                let node = self.admit(&value)?;
                self.lock()?.get(node)?;
                Ok(ValueError::Thrown(value))
            }
            ValueError::IteratorClose { original, close } => Ok(ValueError::IteratorClose {
                original: Box::new(self.admit_runtime_error(*original)?),
                close: Box::new(self.admit_runtime_error(*close)?),
            }),
            other => Ok(other),
        }
    }
    fn runtime_value(self: &Arc<Self>, result: ValueResult<RuntimeReply>) -> ValueResult<ValueHandle> {
        match self.runtime_reply(result)? {
            RuntimeReply::Value(value) => self.adopt(&value),
            _ => Err(ValueError::WrongRuntimeReply("Value")),
        }
    }
    fn runtime_unit(&self, result: ValueResult<RuntimeReply>) -> ValueResult<()> {
        match self.runtime_reply(result)? {
            RuntimeReply::Unit => Ok(()),
            _ => Err(ValueError::WrongRuntimeReply("Unit")),
        }
    }
    fn runtime_bool(&self, result: ValueResult<RuntimeReply>) -> ValueResult<bool> {
        match self.runtime_reply(result)? {
            RuntimeReply::Boolean(value) => Ok(value),
            _ => Err(ValueError::WrongRuntimeReply("Boolean")),
        }
    }
}

/// Same JS canonical array-index predicate used by the existing wire encoder.
fn array_index(key: &PropertyKey) -> Option<u32> {
    let PropertyKey::String(key) = key else {
        return None;
    };
    let units = key.units();
    if units.is_empty() || units.len() > 10 || (units.len() > 1 && units[0] == 0x30) {
        return None;
    }
    let mut result = 0u32;
    for &unit in units {
        if !(0x30..=0x39).contains(&unit) {
            return None;
        }
        result = result.checked_mul(10)?.checked_add(u32::from(unit - 0x30))?;
    }
    (result != u32::MAX).then_some(result)
}
fn index_key(index: u32) -> PropertyKey {
    PropertyKey::String(WireString::from(index.to_string()))
}
fn is_length(key: &PropertyKey) -> bool {
    matches!(key, PropertyKey::String(key) if key.equals_ascii("length"))
}
fn is_primitive_kind(kind: ValueKind) -> bool {
    !matches!(kind, ValueKind::Object | ValueKind::Array | ValueKind::Function)
}
