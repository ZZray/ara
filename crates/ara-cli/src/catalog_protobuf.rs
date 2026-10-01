//! Native codecs for the complete fixed OMP protobuf API (596f2da).
//! Source: packages/catalog/src/discovery/protobuf.ts. Ordered open records,
//! BigInt, UTF-16, absent fields and shared byte views remain native values.
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use ara_rpc::{WireString, WireValue};
use base64::Engine;
use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive, Zero};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoError {
    pub name: &'static str,
    pub message: String,
}
impl ProtoError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { name: "Error", message: message.into() }
    }
    fn type_error(message: impl Into<String>) -> Self {
        Self { name: "TypeError", message: message.into() }
    }
}
impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProtoError {}
pub type ProtoResult<T> = Result<T, ProtoError>;

/// Uint8Array/subarray identity. Decoded bytes and unknown payloads share storage.
#[derive(Clone, Debug)]
pub struct ByteView {
    storage: Arc<Mutex<Vec<u8>>>,
    offset: usize,
    length: usize,
}
impl ByteView {
    pub fn new(bytes: Vec<u8>) -> Self {
        let length = bytes.len();
        Self { storage: Arc::new(Mutex::new(bytes)), offset: 0, length }
    }
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.storage.lock().expect("byte view poisoned")[self.offset..self.offset + self.length].to_vec()
    }
    pub fn set(&self, index: usize, value: u8) {
        if index < self.length {
            self.storage.lock().expect("byte view poisoned")[self.offset + index] = value;
        }
    }
    pub fn get(&self, index: usize) -> Option<u8> {
        (index < self.length).then(|| self.storage.lock().expect("byte view poisoned")[self.offset + index])
    }
    pub fn slice(&self, start: usize, end: usize) -> Self {
        let start = start.min(self.length);
        let end = end.min(self.length).max(start);
        Self { storage: self.storage.clone(), offset: self.offset + start, length: end - start }
    }
    pub fn shares_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.storage, &other.storage)
    }
}

#[derive(Clone, Debug)]
pub enum ProtoValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    BigInt(BigInt),
    String(WireString),
    Bytes(ByteView),
    Array(Arc<Mutex<Vec<ProtoValue>>>),
    Object(ProtoMessage),
    /// Object.prototype's non-enumerable functions are lookup values, not fields.
    OpaqueFunction,
}
impl ProtoValue {
    pub fn string(value: impl Into<WireString>) -> Self {
        Self::String(value.into())
    }
    pub fn array(values: Vec<Self>) -> Self {
        Self::Array(Arc::new(Mutex::new(values)))
    }
    pub fn object(values: &[(&str, Self)]) -> Self {
        Self::Object(ProtoMessage::from_entries(values))
    }
    pub fn as_string(&self) -> Option<WireString> {
        if let Self::String(v) = self { Some(v.clone()) } else { None }
    }
    pub fn as_number(&self) -> Option<f64> {
        if let Self::Number(v) = self { Some(*v) } else { None }
    }
    pub fn as_object(&self) -> Option<ProtoMessage> {
        if let Self::Object(v) = self { Some(v.clone()) } else { None }
    }
    pub fn items(&self) -> Vec<Self> {
        if let Self::Array(v) = self { v.lock().expect("proto array poisoned").clone() } else { vec![] }
    }
    pub fn truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Bool(v) => *v,
            Self::Number(v) => *v != 0.0 && !v.is_nan(),
            Self::BigInt(v) => !v.is_zero(),
            Self::String(v) => !v.is_empty(),
            _ => true,
        }
    }
    fn type_of(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::BigInt(_) => "bigint",
            Self::String(_) => "string",
            Self::OpaqueFunction => "function",
            _ => "object",
        }
    }
    pub fn from_wire(value: &WireValue) -> Self {
        match value {
            WireValue::Null => Self::Null,
            WireValue::Bool(v) => Self::Bool(*v),
            WireValue::Number(v) => Self::Number(*v),
            WireValue::String(v) => Self::String(v.clone()),
            WireValue::Array(v) => Self::array(v.iter().map(Self::from_wire).collect()),
            WireValue::Object(v) => {
                let out = ProtoMessage::new(false);
                for (k, v) in v {
                    out.set_own(k.clone(), Self::from_wire(v));
                }
                Self::Object(out)
            }
        }
    }
    /// A JSON boundary is explicit; native messages never travel through this.
    pub fn to_wire(&self) -> ProtoResult<WireValue> {
        Ok(match self {
            Self::Null => WireValue::Null,
            Self::Bool(v) => WireValue::Bool(*v),
            Self::Number(v) => WireValue::Number(*v),
            Self::String(v) => WireValue::String(v.clone()),
            Self::Array(v) => WireValue::Array(
                v.lock()
                    .expect("proto array poisoned")
                    .iter()
                    .map(|v| if matches!(v, Self::Undefined) { Ok(WireValue::Null) } else { v.to_wire() })
                    .collect::<ProtoResult<_>>()?,
            ),
            Self::Object(v) => WireValue::Object(
                v.own_entries()
                    .into_iter()
                    .filter(|(_, v)| !matches!(v, Self::Undefined))
                    .map(|(k, v)| Ok((k, v.to_wire()?)))
                    .collect::<ProtoResult<_>>()?,
            ),
            Self::Undefined => return Err(ProtoError::new("undefined has no JSON value")),
            Self::BigInt(_) => return Err(ProtoError::type_error("Do not know how to serialize a BigInt")),
            Self::Bytes(_) | Self::OpaqueFunction => {
                return Err(ProtoError::new("native value requires explicit protobuf JSON conversion"));
            }
        })
    }
}

#[derive(Debug, Clone)]
struct Record {
    entries: Vec<(WireString, ProtoValue)>,
    prototype: Option<ProtoMessage>,
    ordinary: bool,
    object_prototype: bool,
}
#[derive(Clone, Debug)]
pub struct ProtoMessage(Arc<Mutex<Record>>);
/// Forward-compatible payload view excludes the tag and shares decoded storage.
#[derive(Clone, Debug)]
pub struct ProtoUnknownField {
    pub no: u32,
    pub wire_type: WireType,
    pub data: ByteView,
}
impl ProtoUnknownField {
    pub fn into_value(self) -> ProtoValue {
        ProtoValue::object(&[
            ("no", ProtoValue::Number(self.no.into())),
            ("wireType", ProtoValue::Number(self.wire_type.into())),
            ("data", ProtoValue::Bytes(self.data)),
        ])
    }
}
fn index_key(key: &WireString) -> Option<u32> {
    let s = key.to_utf8().ok()?;
    let n = s.parse::<u32>().ok()?;
    (n != u32::MAX && n.to_string() == s).then_some(n)
}
impl ProtoMessage {
    pub fn new(ordinary: bool) -> Self {
        Self(Arc::new(Mutex::new(Record { entries: vec![], prototype: None, ordinary, object_prototype: false })))
    }
    pub fn from_entries(values: &[(&str, ProtoValue)]) -> Self {
        let out = Self::new(true);
        for (k, v) in values {
            out.set_own((*k).into(), v.clone());
        }
        out
    }
    pub fn get(&self, name: &str) -> ProtoValue {
        self.get_key(&name.into())
    }
    pub fn get_key(&self, name: &WireString) -> ProtoValue {
        let rec = self.0.lock().expect("proto record poisoned");
        if let Some((_, v)) = rec.entries.iter().find(|(k, _)| k == name) {
            return v.clone();
        }
        let proto = rec.prototype.clone();
        let ordinary = rec.ordinary;
        let object_prototype = rec.object_prototype;
        drop(rec);
        if name.equals_ascii("__proto__") {
            if object_prototype {
                return ProtoValue::Null;
            }
            let inherits_getter = proto.as_ref().map_or(ordinary, Self::inherits_object_prototype);
            if inherits_getter {
                return ProtoValue::Object(proto.unwrap_or_else(Self::object_prototype));
            }
        }
        if let Some(p) = proto {
            return p.get_key(name);
        }
        if (ordinary || object_prototype)
            && [
                "constructor",
                "toString",
                "toLocaleString",
                "valueOf",
                "hasOwnProperty",
                "isPrototypeOf",
                "propertyIsEnumerable",
                "__defineGetter__",
                "__defineSetter__",
                "__lookupGetter__",
                "__lookupSetter__",
            ]
            .iter()
            .any(|s| name.equals_ascii(s))
        {
            ProtoValue::OpaqueFunction
        } else {
            ProtoValue::Undefined
        }
    }
    fn object_prototype() -> Self {
        static PROTOTYPE: OnceLock<ProtoMessage> = OnceLock::new();
        PROTOTYPE
            .get_or_init(|| {
                Self(Arc::new(Mutex::new(Record {
                    entries: vec![],
                    prototype: None,
                    ordinary: false,
                    object_prototype: true,
                })))
            })
            .clone()
    }
    fn inherits_object_prototype(&self) -> bool {
        let record = self.0.lock().expect("proto record poisoned");
        let prototype = record.prototype.clone();
        let ordinary = record.ordinary;
        let builtin = record.object_prototype;
        drop(record);
        builtin || prototype.as_ref().map_or(ordinary, Self::inherits_object_prototype)
    }
    pub fn has_own(&self, name: &str) -> bool {
        self.0.lock().expect("proto record poisoned").entries.iter().any(|(k, _)| k.equals_ascii(name))
    }
    pub fn set(&self, name: &str, value: ProtoValue) {
        self.reflect_set(name.into(), value);
    }
    pub fn set_own(&self, key: WireString, value: ProtoValue) {
        let mut rec = self.0.lock().expect("proto record poisoned");
        if let Some((_, v)) = rec.entries.iter_mut().find(|(k, _)| *k == key) {
            *v = value;
        } else {
            rec.entries.push((key, value));
        }
    }
    pub fn set_prototype(&self, prototype: Option<Self>) {
        self.0.lock().expect("proto record poisoned").prototype = prototype;
    }
    fn reflect_set(&self, key: WireString, value: ProtoValue) {
        if key.equals_ascii("__proto__") {
            let inherits_getter = self.inherits_object_prototype();
            let mut rec = self.0.lock().expect("proto record poisoned");
            if inherits_getter && !rec.entries.iter().any(|(k, _)| k == &key) {
                match value {
                    ProtoValue::Object(p) => rec.prototype = Some(p),
                    ProtoValue::Null => {
                        rec.prototype = None;
                        rec.ordinary = false;
                    }
                    _ => {}
                }
                return;
            }
        }
        self.set_own(key, value);
    }
    pub fn own_entries(&self) -> Vec<(WireString, ProtoValue)> {
        let mut rows = self.0.lock().expect("proto record poisoned").entries.clone();
        rows.sort_by(|(a, _), (b, _)| match (index_key(a), index_key(b)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
        rows
    }
    pub fn enumerable_entries(&self) -> Vec<(WireString, ProtoValue)> {
        let mut out = self.own_entries();
        let held: HashSet<_> = out.iter().map(|(k, _)| k.clone()).collect();
        let proto = self.0.lock().expect("proto record poisoned").prototype.clone();
        if let Some(p) = proto {
            out.extend(p.enumerable_entries().into_iter().filter(|(k, _)| !held.contains(k)));
        }
        out
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn is_ordinary(&self) -> bool {
        self.0.lock().expect("proto record poisoned").ordinary
    }
    pub fn prototype(&self) -> Option<Self> {
        self.0.lock().expect("proto record poisoned").prototype.clone()
    }
    pub fn to_wire(&self) -> ProtoResult<WireValue> {
        ProtoValue::Object(self.clone()).to_wire()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Bool,
    Bytes,
    Double,
    Enum,
    Float,
    Int32,
    Int64,
    String,
    Uint32,
    Uint64,
}
impl ScalarKind {
    fn parse(s: &str) -> Self {
        match s {
            "bool" => Self::Bool,
            "bytes" => Self::Bytes,
            "double" => Self::Double,
            "enum" => Self::Enum,
            "float" => Self::Float,
            "int32" => Self::Int32,
            "int64" => Self::Int64,
            "string" => Self::String,
            "uint32" => Self::Uint32,
            "uint64" => Self::Uint64,
            _ => panic!("unknown generated scalar"),
        }
    }
    fn wire(self) -> u8 {
        match self {
            Self::Double => 1,
            Self::String | Self::Bytes => 2,
            Self::Float => 5,
            _ => 0,
        }
    }
}
pub type JsonValue = ProtoValue;
pub type InferMessage = ProtoMessage;
pub type WireType = u8;
/// Descriptor references may supply arbitrary native callbacks, as in OMP.
pub trait ReferenceCodec: Send + Sync {
    fn encode(&self, value: &ProtoValue) -> ProtoResult<ByteView>;
    fn decode(&self, value: &ByteView) -> ProtoResult<ProtoMessage>;
    fn to_json(&self, value: &ProtoValue) -> ProtoResult<JsonValue>;
}
impl ReferenceCodec for MessageCodec {
    fn encode(&self, value: &ProtoValue) -> ProtoResult<ByteView> {
        MessageCodec::encode(
            self,
            &value
                .as_object()
                .ok_or_else(|| ProtoError::type_error("Reflect.get requires the first argument be an object"))?,
        )
    }
    fn decode(&self, value: &ByteView) -> ProtoResult<ProtoMessage> {
        MessageCodec::decode(self, value)
    }
    fn to_json(&self, value: &ProtoValue) -> ProtoResult<JsonValue> {
        MessageCodec::to_json(
            self,
            &value
                .as_object()
                .ok_or_else(|| ProtoError::type_error("Reflect.get requires the first argument be an object"))?,
        )
    }
}
#[derive(Clone)]
pub struct MessageReference {
    factory: Arc<dyn Fn() -> Arc<dyn ReferenceCodec> + Send + Sync>,
    cached: Arc<OnceLock<Arc<dyn ReferenceCodec>>>,
}
impl fmt::Debug for MessageReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MessageReference")
    }
}
impl MessageReference {
    pub fn new(factory: impl Fn() -> MessageCodec + Send + Sync + 'static) -> Self {
        Self::custom(move || Arc::new(factory()))
    }
    pub fn custom(factory: impl Fn() -> Arc<dyn ReferenceCodec> + Send + Sync + 'static) -> Self {
        Self { factory: Arc::new(factory), cached: Arc::new(OnceLock::new()) }
    }
    fn get(&self) -> Arc<dyn ReferenceCodec> {
        self.cached.get_or_init(|| (self.factory)()).clone()
    }
    fn for_compiled_value(&self) -> Self {
        Self { factory: self.factory.clone(), cached: Arc::new(OnceLock::new()) }
    }
    pub fn encode(&self, value: &ProtoValue) -> ProtoResult<ByteView> {
        self.get().encode(value)
    }
    pub fn decode(&self, value: &ByteView) -> ProtoResult<ProtoMessage> {
        self.get().decode(value)
    }
    pub fn to_json(&self, value: &ProtoValue) -> ProtoResult<JsonValue> {
        self.get().to_json(value)
    }
}
#[derive(Clone, Debug)]
pub enum ValueKind {
    Scalar(ScalarKind),
    Message(MessageReference),
}
#[derive(Clone, Debug)]
pub struct VariantDesc {
    pub no: u32,
    pub name: WireString,
    pub kind: ValueKind,
}
#[derive(Clone, Debug)]
pub enum FieldDesc {
    Scalar { no: u32, name: WireString, kind: ScalarKind, optional: bool, repeat: bool },
    Message { no: u32, name: WireString, reference: MessageReference, repeat: bool },
    Map { no: u32, name: WireString, value: ValueKind },
    Oneof { name: WireString, variants: Vec<VariantDesc> },
}
pub type ScalarFieldDesc = FieldDesc;
pub type MessageFieldDesc = FieldDesc;
pub type EnumFieldDesc = FieldDesc;
pub type MapFieldDesc = FieldDesc;
pub type OneofFieldDesc = FieldDesc;
impl FieldDesc {
    fn compiled_clone(&self) -> Self {
        let fresh = |value: &ValueKind| match value {
            ValueKind::Scalar(kind) => ValueKind::Scalar(*kind),
            ValueKind::Message(reference) => ValueKind::Message(reference.for_compiled_value()),
        };
        match self {
            Self::Message { no, name, reference, repeat } => Self::Message {
                no: *no,
                name: name.clone(),
                reference: reference.for_compiled_value(),
                repeat: *repeat,
            },
            Self::Map { no, name, value } => Self::Map { no: *no, name: name.clone(), value: fresh(value) },
            Self::Oneof { name, variants } => Self::Oneof {
                name: name.clone(),
                variants: variants
                    .iter()
                    .map(|variant| VariantDesc {
                        no: variant.no,
                        name: variant.name.clone(),
                        kind: fresh(&variant.kind),
                    })
                    .collect(),
            },
            field => field.clone(),
        }
    }
    pub fn scalar(no: u32, name: impl Into<WireString>, kind: ScalarKind) -> Self {
        Self::Scalar { no, name: name.into(), kind, optional: false, repeat: false }
    }
    fn numbers(&self) -> Vec<u32> {
        match self {
            Self::Oneof { variants, .. } => variants.iter().map(|v| v.no).collect(),
            Self::Scalar { no, .. } | Self::Message { no, .. } | Self::Map { no, .. } => vec![*no],
        }
    }
}
#[derive(Debug)]
struct Compiled {
    fields: Vec<FieldDesc>,
    defaults: Vec<ProtoValue>,
    by_number: HashMap<u32, usize>,
}
#[derive(Debug)]
struct CodecInner {
    type_name: WireString,
    descriptors: Arc<Mutex<Vec<FieldDesc>>>,
    compiled: OnceLock<Compiled>,
}
#[derive(Clone, Debug)]
pub struct MessageCodec(Arc<CodecInner>);
pub enum CodecArgument {
    Message(ProtoMessage),
    Bytes(ByteView),
}
pub enum CodecOutput {
    Message(ProtoMessage),
    Bytes(ByteView),
}
pub fn pb(type_name: impl Into<WireString>, fields: Vec<FieldDesc>) -> MessageCodec {
    pb_shared(type_name, Arc::new(Mutex::new(fields)))
}
pub fn pb_shared(type_name: impl Into<WireString>, fields: Arc<Mutex<Vec<FieldDesc>>>) -> MessageCodec {
    MessageCodec(Arc::new(CodecInner { type_name: type_name.into(), descriptors: fields, compiled: OnceLock::new() }))
}
pub trait CodecSource {
    fn message_codec(&self) -> MessageCodec;
}
impl CodecSource for MessageCodec {
    fn message_codec(&self) -> MessageCodec {
        self.clone()
    }
}
impl CodecSource for SchemaHandle {
    fn message_codec(&self) -> MessageCodec {
        self.codec()
    }
}
pub fn create(codec: &impl CodecSource, value: Option<&ProtoMessage>) -> ProtoMessage {
    codec.message_codec().create(value)
}
pub fn to_binary(codec: &impl CodecSource, value: &ProtoMessage) -> ProtoResult<ByteView> {
    codec.message_codec().encode(value)
}
pub fn from_binary(codec: &impl CodecSource, value: &ByteView) -> ProtoResult<ProtoMessage> {
    codec.message_codec().decode(value)
}
pub fn to_json(codec: &impl CodecSource, value: &ProtoMessage) -> ProtoResult<ProtoValue> {
    codec.message_codec().to_json(value)
}
impl MessageCodec {
    fn compiled(&self) -> &Compiled {
        self.0.compiled.get_or_init(|| {
            let fields: Vec<_> = self
                .0
                .descriptors
                .lock()
                .expect("proto descriptors poisoned")
                .iter()
                .map(FieldDesc::compiled_clone)
                .collect();
            let defaults = fields
                .iter()
                .map(|f| match f {
                    FieldDesc::Scalar { kind, .. } => default_value(&ValueKind::Scalar(*kind)),
                    FieldDesc::Map { value, .. } => default_value(value),
                    _ => ProtoValue::Undefined,
                })
                .collect();
            let mut by_number = HashMap::new();
            for (i, f) in fields.iter().enumerate() {
                for no in f.numbers() {
                    by_number.insert(no, i);
                }
            }
            Compiled { fields, defaults, by_number }
        })
    }
    pub fn type_name(&self) -> &WireString {
        &self.0.type_name
    }
    pub fn descriptors(&self) -> &Arc<Mutex<Vec<FieldDesc>>> {
        &self.0.descriptors
    }
    pub fn invoke(&self, arg: CodecArgument) -> ProtoResult<CodecOutput> {
        match arg {
            CodecArgument::Message(v) => self.encode(&v).map(CodecOutput::Bytes),
            CodecArgument::Bytes(v) => self.decode(&v).map(CodecOutput::Message),
        }
    }
    pub fn create(&self, input: Option<&ProtoMessage>) -> ProtoMessage {
        let value = ProtoMessage::new(true);
        if !self.0.type_name.is_empty() {
            value.set("$typeName", ProtoValue::String(self.0.type_name.clone()));
        }
        for (f, default) in self.compiled().fields.iter().zip(&self.compiled().defaults) {
            init_field(f, default, &value);
        }
        if let Some(input) = input {
            for (k, v) in input.enumerable_entries() {
                if !matches!(v, ProtoValue::Undefined) {
                    value.reflect_set(k, v);
                }
            }
        }
        value
    }
    pub fn encode(&self, value: &ProtoMessage) -> ProtoResult<ByteView> {
        let mut writer = Writer::default();
        for f in &self.compiled().fields {
            encode_field(f, value, &mut writer)?;
        }
        for unknown in value.get("$unknown").items() {
            if let Some(m) = unknown.as_object()
                && let (ProtoValue::Number(no), ProtoValue::Number(wt), ProtoValue::Bytes(data)) =
                    (m.get("no"), m.get("wireType"), m.get("data"))
                && matches!(wt, 0.0 | 1.0 | 2.0 | 5.0)
            {
                writer.tag(to_u32(no), wt as u8);
                writer.raw(&data.bytes());
            }
        }
        Ok(ByteView::new(writer.0))
    }
    pub fn decode(&self, input: &ByteView) -> ProtoResult<ProtoMessage> {
        let value = self.create(None);
        let mut reader = Reader::new(input.clone());
        while reader.pos < reader.len() {
            let tag = reader.uint32()?;
            let no = tag >> 3;
            let wt = (tag & 7) as u8;
            if !valid_wire(wt) {
                return Err(ProtoError::new(format!("Unsupported protobuf wire type {wt} at byte {}", reader.pos)));
            }
            if let Some(i) = self.compiled().by_number.get(&no) {
                decode_field(&self.compiled().fields[*i], &self.compiled().defaults[*i], &value, &mut reader, wt, no)?;
            } else {
                let start = reader.pos;
                reader.skip(wt)?;
                let unknown = ProtoValue::object(&[
                    ("no", ProtoValue::Number(no.into())),
                    ("wireType", ProtoValue::Number(wt.into())),
                    ("data", ProtoValue::Bytes(input.slice(start, reader.pos))),
                ]);
                let bag = value.get("$unknown");
                if let ProtoValue::Array(v) = bag
                    && v.lock().expect("proto array poisoned").iter().all(is_unknown)
                {
                    v.lock().expect("proto array poisoned").push(unknown);
                    continue;
                }
                value.set("$unknown", ProtoValue::array(vec![unknown]));
            }
        }
        Ok(value)
    }
    pub fn to_json(&self, value: &ProtoMessage) -> ProtoResult<ProtoValue> {
        let out = ProtoMessage::new(true);
        for f in &self.compiled().fields {
            json_field(f, value, &out)?;
        }
        Ok(ProtoValue::Object(out))
    }
}
fn is_unknown(value: &ProtoValue) -> bool {
    value.as_object().is_some_and(|m| {
        matches!(
            (m.get("no"), m.get("wireType"), m.get("data")),
            (ProtoValue::Number(_), ProtoValue::Number(0.0 | 1.0 | 2.0 | 5.0), ProtoValue::Bytes(_))
        )
    })
}
fn valid_wire(wt: u8) -> bool {
    matches!(wt, 0 | 1 | 2 | 5)
}
fn assert_wire(actual: u8, expected: u8) -> ProtoResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(ProtoError::new(format!("Unexpected protobuf wire type {actual}; expected {expected}")))
    }
}
fn default_value(kind: &ValueKind) -> ProtoValue {
    match kind {
        ValueKind::Message(_) => ProtoValue::Undefined,
        ValueKind::Scalar(k) => match k {
            ScalarKind::Bool => ProtoValue::Bool(false),
            ScalarKind::Bytes => ProtoValue::Bytes(ByteView::new(vec![])),
            ScalarKind::String => ProtoValue::string(""),
            ScalarKind::Int64 | ScalarKind::Uint64 => ProtoValue::BigInt(BigInt::zero()),
            _ => ProtoValue::Number(0.0),
        },
    }
}
fn kind_wire(kind: &ValueKind) -> u8 {
    match kind {
        ValueKind::Message(_) => 2,
        ValueKind::Scalar(k) => k.wire(),
    }
}
fn init_field(field: &FieldDesc, default: &ProtoValue, out: &ProtoMessage) {
    match field {
        FieldDesc::Scalar { name, optional, repeat, .. } => {
            if *repeat {
                out.reflect_set(name.clone(), ProtoValue::array(vec![]));
            } else if !optional {
                out.reflect_set(name.clone(), default.clone());
            }
        }
        FieldDesc::Message { name, repeat, .. } => {
            if *repeat {
                out.reflect_set(name.clone(), ProtoValue::array(vec![]));
            }
        }
        FieldDesc::Map { name, .. } => out.reflect_set(name.clone(), ProtoValue::Object(ProtoMessage::new(false))),
        FieldDesc::Oneof { name, .. } => {
            out.reflect_set(name.clone(), ProtoValue::object(&[("case", ProtoValue::Undefined)]))
        }
    }
}
fn encode_field(field: &FieldDesc, input: &ProtoMessage, w: &mut Writer) -> ProtoResult<()> {
    match field {
        FieldDesc::Scalar { no, name, kind, optional, repeat } => {
            encode_normal(*no, name, &ValueKind::Scalar(*kind), *optional, *repeat, input, w)
        }
        FieldDesc::Message { no, name, reference, repeat } => {
            encode_normal(*no, name, &ValueKind::Message(reference.clone()), false, *repeat, input, w)
        }
        FieldDesc::Map { no, name, value } => {
            if let Some(map) = input.get_key(name).as_object() {
                for (k, v) in map.enumerable_entries() {
                    let mut entry = Writer::default();
                    let key = ProtoValue::String(k);
                    if !is_default(&ValueKind::Scalar(ScalarKind::String), &key)? {
                        entry.tag(1, 2);
                        write_value(&ValueKind::Scalar(ScalarKind::String), &key, &mut entry)?;
                    }
                    if !is_default(value, &v)? {
                        entry.tag(2, kind_wire(value));
                        write_value(value, &v, &mut entry)?;
                    }
                    w.tag(*no, 2);
                    w.delimited(&entry.0);
                }
            }
            Ok(())
        }
        FieldDesc::Oneof { name, variants } => {
            if let Some(one) = input.get_key(name).as_object()
                && let Some(case) = one.get("case").as_string()
                && let Some(v) = variants.iter().rev().find(|v| v.name == case)
            {
                let input = one.get("value");
                if !matches!(input, ProtoValue::Undefined) {
                    w.tag(v.no, kind_wire(&v.kind));
                    write_value(&v.kind, &input, w)?;
                }
            }
            Ok(())
        }
    }
}
fn encode_normal(
    no: u32,
    name: &WireString,
    kind: &ValueKind,
    optional: bool,
    repeat: bool,
    m: &ProtoMessage,
    w: &mut Writer,
) -> ProtoResult<()> {
    let value = m.get_key(name);
    if repeat {
        if let ProtoValue::Array(items) = value {
            if items.lock().expect("proto array poisoned").is_empty() {
                return Ok(());
            }
            let wt = kind_wire(kind);
            if wt != 2 {
                let mut packed = Writer::default();
                let mut index = 0;
                loop {
                    let value = items.lock().expect("proto array poisoned").get(index).cloned();
                    let Some(value) = value else { break };
                    write_value(kind, &value, &mut packed)?;
                    index += 1;
                }
                w.tag(no, 2);
                w.delimited(&packed.0);
            } else {
                let mut index = 0;
                loop {
                    // A source for-of reads the current element and current
                    // length. Release the array before invoking a public codec.
                    let value = items.lock().expect("proto array poisoned").get(index).cloned();
                    let Some(value) = value else { break };
                    w.tag(no, wt);
                    write_value(kind, &value, w)?;
                    index += 1;
                }
            }
        }
        return Ok(());
    }
    if matches!(value, ProtoValue::Undefined) || (!optional && is_default(kind, &value)?) {
        return Ok(());
    }
    w.tag(no, kind_wire(kind));
    write_value(kind, &value, w)
}
fn decode_field(
    f: &FieldDesc,
    default: &ProtoValue,
    m: &ProtoMessage,
    r: &mut Reader,
    wt: u8,
    no: u32,
) -> ProtoResult<()> {
    match f {
        FieldDesc::Scalar { name, kind, repeat, .. } => {
            decode_normal(name, &ValueKind::Scalar(*kind), *repeat, m, r, wt)
        }
        FieldDesc::Message { name, reference, repeat, .. } => {
            decode_normal(name, &ValueKind::Message(reference.clone()), *repeat, m, r, wt)
        }
        FieldDesc::Map { name, value, .. } => {
            assert_wire(wt, 2)?;
            let target = if let Some(v) = m.get_key(name).as_object() {
                v
            } else {
                let v = ProtoMessage::new(false);
                m.reflect_set(name.clone(), ProtoValue::Object(v.clone()));
                v
            };
            let limit = r.uint32()? as usize;
            let end = r.pos.checked_add(limit).ok_or_else(|| ProtoError::new("protobuf length overflow"))?;
            let mut key = WireString::from("");
            let mut val = default.clone();
            while r.pos < end {
                let tag = r.uint32()?;
                let entry_no = tag >> 3;
                let ew = (tag & 7) as u8;
                if !valid_wire(ew) {
                    return Err(ProtoError::new(format!("Unsupported wire type {ew} in map entry")));
                }
                if entry_no == 1 {
                    assert_wire(ew, 2)?;
                    key = r.string()?;
                } else if entry_no == 2 {
                    assert_wire(ew, kind_wire(value))?;
                    val = read_value(value, r)?;
                } else {
                    r.skip(ew)?;
                }
            }
            target.reflect_set(key, val);
            Ok(())
        }
        FieldDesc::Oneof { name, variants } => {
            let v = variants
                .iter()
                .rev()
                .find(|v| v.no == no)
                .ok_or_else(|| ProtoError::new(format!("Unknown oneof field {no}")))?;
            assert_wire(wt, kind_wire(&v.kind))?;
            m.reflect_set(
                name.clone(),
                ProtoValue::object(&[("case", ProtoValue::String(v.name.clone())), ("value", read_value(&v.kind, r)?)]),
            );
            Ok(())
        }
    }
}
fn decode_normal(
    name: &WireString,
    kind: &ValueKind,
    repeat: bool,
    m: &ProtoMessage,
    r: &mut Reader,
    wt: u8,
) -> ProtoResult<()> {
    if repeat {
        let array = match m.get_key(name) {
            ProtoValue::Array(v) => v,
            _ => {
                let v = Arc::new(Mutex::new(vec![]));
                m.reflect_set(name.clone(), ProtoValue::Array(v.clone()));
                v
            }
        };
        if wt == 2 && kind_wire(kind) != 2 {
            let limit = r.uint32()? as usize;
            let end = r.pos.checked_add(limit).ok_or_else(|| ProtoError::new("protobuf length overflow"))?;
            while r.pos < end {
                let value = read_value(kind, r)?;
                array.lock().expect("proto array poisoned").push(value);
            }
            return Ok(());
        }
        assert_wire(wt, kind_wire(kind))?;
        let value = read_value(kind, r)?;
        array.lock().expect("proto array poisoned").push(value);
    } else {
        assert_wire(wt, kind_wire(kind))?;
        m.reflect_set(name.clone(), read_value(kind, r)?);
    }
    Ok(())
}
fn json_field(f: &FieldDesc, m: &ProtoMessage, out: &ProtoMessage) -> ProtoResult<()> {
    match f {
        FieldDesc::Scalar { name, kind, optional, repeat, .. } => {
            json_normal(name, &ValueKind::Scalar(*kind), *optional, *repeat, m, out)
        }
        FieldDesc::Message { name, reference, repeat, .. } => {
            json_normal(name, &ValueKind::Message(reference.clone()), false, *repeat, m, out)
        }
        FieldDesc::Map { name, value, .. } => {
            if let Some(map) = m.get_key(name).as_object() {
                let target = ProtoMessage::new(false);
                for (k, v) in map.enumerable_entries() {
                    target.reflect_set(k, json_value(value, &v)?);
                }
                out.reflect_set(name.clone(), ProtoValue::Object(target));
            }
            Ok(())
        }
        FieldDesc::Oneof { name, variants } => {
            if let Some(one) = m.get_key(name).as_object()
                && let Some(case) = one.get("case").as_string()
                && let Some(v) = variants.iter().rev().find(|v| v.name == case)
            {
                let value = one.get("value");
                if !matches!(value, ProtoValue::Undefined) {
                    out.reflect_set(v.name.clone(), json_value(&v.kind, &value)?);
                }
            }
            Ok(())
        }
    }
}
fn json_normal(
    name: &WireString,
    kind: &ValueKind,
    optional: bool,
    repeat: bool,
    m: &ProtoMessage,
    out: &ProtoMessage,
) -> ProtoResult<()> {
    let value = m.get_key(name);
    if repeat {
        if let ProtoValue::Array(v) = value {
            let length = v.lock().expect("proto array poisoned").len();
            if length != 0 {
                // Array.map fixes the initial length, but observes replacement
                // of later elements. Missing entries remain holes at the wire
                // boundary rather than invoking a callback with undefined.
                let mut items = Vec::with_capacity(length);
                for index in 0..length {
                    let value = v.lock().expect("proto array poisoned").get(index).cloned();
                    items.push(value.map_or(Ok(ProtoValue::Undefined), |value| json_value(kind, &value))?);
                }
                out.reflect_set(name.clone(), ProtoValue::array(items));
            }
        }
    } else if !matches!(value, ProtoValue::Undefined) && (optional || !is_default(kind, &value)?) {
        out.reflect_set(name.clone(), json_value(kind, &value)?);
    }
    Ok(())
}

fn to_u32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 { 0 } else { n.trunc().rem_euclid(4294967296.0) as u32 }
}
fn js_whitespace(c: u16) -> bool {
    matches!(c,0x0009..=0x000d|0x0020|0x00a0|0x1680|0x2000..=0x200a|0x2028..=0x2029|0x202f|0x205f|0x3000|0xfeff)
}
fn parse_bigint(s: &WireString) -> ProtoResult<BigInt> {
    let units = s.units();
    let start = units.iter().position(|&c| !js_whitespace(c)).unwrap_or(units.len());
    let end = units.iter().rposition(|&c| !js_whitespace(c)).map_or(start, |i| i + 1);
    let parse_error = || ProtoError { name: "SyntaxError", message: "Failed to parse String to BigInt".into() };
    let s = String::from_utf16(&units[start..end]).map_err(|_| parse_error())?;
    if s.is_empty() {
        return Ok(BigInt::zero());
    }
    let (radix, digits) = if s.starts_with("0x") || s.starts_with("0X") {
        (16, &s[2..])
    } else if s.starts_with("0b") || s.starts_with("0B") {
        (2, &s[2..])
    } else if s.starts_with("0o") || s.starts_with("0O") {
        (8, &s[2..])
    } else {
        (10, s.as_str())
    };
    let digits = if radix == 10 { digits.strip_prefix('+').unwrap_or(digits) } else { digits };
    let body = if radix == 10 { digits.strip_prefix('-').unwrap_or(digits) } else { digits };
    if body.is_empty() || !body.chars().all(|c| c.is_ascii() && c.is_digit(radix)) {
        return Err(parse_error());
    }
    BigInt::parse_bytes(digits.as_bytes(), radix).ok_or_else(parse_error)
}
fn bigint(v: &ProtoValue) -> ProtoResult<BigInt> {
    match v {
        ProtoValue::BigInt(v) => Ok(v.clone()),
        ProtoValue::Number(v) if v.is_finite() && v.fract() == 0.0 => {
            BigInt::from_f64(*v).ok_or_else(|| ProtoError::new("Expected bigint, got number"))
        }
        ProtoValue::String(v) => parse_bigint(v),
        _ => Err(ProtoError::new(format!("Expected bigint, got {}", v.type_of()))),
    }
}
fn validate(kind: ScalarKind, v: &ProtoValue) -> ProtoResult<ProtoValue> {
    Ok(match kind {
        ScalarKind::Bool => {
            if let ProtoValue::Bool(v) = v {
                ProtoValue::Bool(*v)
            } else {
                return Err(ProtoError::new(format!("Expected boolean, got {}", v.type_of())));
            }
        }
        ScalarKind::Bytes => {
            if let ProtoValue::Bytes(v) = v {
                ProtoValue::Bytes(v.clone())
            } else {
                return Err(ProtoError::new("Expected Uint8Array"));
            }
        }
        ScalarKind::String => {
            if let ProtoValue::String(v) = v {
                ProtoValue::String(v.clone())
            } else {
                return Err(ProtoError::new(format!("Expected string, got {}", v.type_of())));
            }
        }
        ScalarKind::Float | ScalarKind::Double => {
            if let ProtoValue::Number(v) = v {
                if !v.is_finite() {
                    return Err(ProtoError::new("Expected number, got number"));
                }
                ProtoValue::Number(*v)
            } else {
                return Err(ProtoError::new(format!("Expected number, got {}", v.type_of())));
            }
        }
        ScalarKind::Int32 | ScalarKind::Enum => {
            if let ProtoValue::Number(v) = v {
                if !v.is_finite() || v.fract() != 0.0 {
                    return Err(ProtoError::new("Expected int32, got number"));
                }
                ProtoValue::Number((to_u32(*v) as i32).into())
            } else {
                return Err(ProtoError::new(format!("Expected int32, got {}", v.type_of())));
            }
        }
        ScalarKind::Uint32 => {
            if let ProtoValue::Number(v) = v {
                if !v.is_finite() || v.fract() != 0.0 || *v < 0.0 {
                    return Err(ProtoError::new("Expected uint32, got number"));
                }
                ProtoValue::Number(to_u32(*v).into())
            } else {
                return Err(ProtoError::new(format!("Expected uint32, got {}", v.type_of())));
            }
        }
        ScalarKind::Int64 | ScalarKind::Uint64 => {
            let b = bigint(v)?;
            if kind == ScalarKind::Uint64 && b < BigInt::zero() {
                return Err(ProtoError::new("Expected unsigned bigint"));
            }
            ProtoValue::BigInt(b)
        }
    })
}
fn is_default(kind: &ValueKind, value: &ProtoValue) -> ProtoResult<bool> {
    if matches!(kind, ValueKind::Message(_)) {
        return Ok(matches!(value, ProtoValue::Undefined));
    }
    let ValueKind::Scalar(kind) = kind else { unreachable!() };
    Ok(match validate(*kind, value)? {
        ProtoValue::Bool(v) => !v,
        ProtoValue::Number(v) => v == 0.0,
        ProtoValue::BigInt(v) => v.is_zero(),
        ProtoValue::String(v) => v.is_empty(),
        ProtoValue::Bytes(v) => v.is_empty(),
        _ => false,
    })
}
fn write_value(kind: &ValueKind, v: &ProtoValue, w: &mut Writer) -> ProtoResult<()> {
    if let ValueKind::Message(reference) = kind {
        w.delimited(&reference.get().encode(v)?.bytes());
        return Ok(());
    }
    let ValueKind::Scalar(kind) = kind else { unreachable!() };
    match validate(*kind, v)? {
        ProtoValue::Bool(v) => w.0.push(u8::from(v)),
        ProtoValue::String(v) => w.delimited(String::from_utf16_lossy(v.units()).as_bytes()),
        ProtoValue::Bytes(v) => w.delimited(&v.bytes()),
        ProtoValue::BigInt(v) => {
            let modulus: BigInt = BigInt::from(1u8) << 64_usize;
            let unsigned: BigInt = ((v % &modulus) + &modulus) % &modulus;
            w.uint64(unsigned.to_u64().expect("mod64 fits"));
        }
        ProtoValue::Number(v) => match kind {
            ScalarKind::Float => w.raw(&(v as f32).to_le_bytes()),
            ScalarKind::Double => w.raw(&v.to_le_bytes()),
            ScalarKind::Int32 | ScalarKind::Enum => {
                let n = v as i32;
                if n < 0 {
                    w.uint64(n as i64 as u64);
                } else {
                    w.uint32(n as u32);
                }
            }
            ScalarKind::Uint32 => w.uint32(v as u32),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
    Ok(())
}
fn read_value(kind: &ValueKind, r: &mut Reader) -> ProtoResult<ProtoValue> {
    Ok(match kind {
        ValueKind::Message(reference) => ProtoValue::Object(reference.get().decode(&r.bytes()?)?),
        ValueKind::Scalar(kind) => match kind {
            ScalarKind::Bool => ProtoValue::Bool(r.uint32()? != 0),
            ScalarKind::Bytes => ProtoValue::Bytes(r.bytes()?),
            ScalarKind::String => ProtoValue::String(r.string()?),
            ScalarKind::Float => ProtoValue::Number(f32::from_le_bytes(r.fixed::<4>("float")?) as f64),
            ScalarKind::Double => ProtoValue::Number(f64::from_le_bytes(r.fixed::<8>("double")?)),
            ScalarKind::Int32 | ScalarKind::Enum => ProtoValue::Number((r.uint64()? as u32 as i32).into()),
            ScalarKind::Uint32 => ProtoValue::Number(r.uint32()?.into()),
            ScalarKind::Int64 => ProtoValue::BigInt(BigInt::from(r.uint64()? as i64)),
            ScalarKind::Uint64 => ProtoValue::BigInt(BigInt::from(r.uint64()?)),
        },
    })
}
fn json_value(kind: &ValueKind, v: &ProtoValue) -> ProtoResult<ProtoValue> {
    match kind {
        ValueKind::Message(reference) => reference.get().to_json(v),
        ValueKind::Scalar(kind) => Ok(match validate(*kind, v)? {
            ProtoValue::BigInt(v) => ProtoValue::string(v.to_string()),
            ProtoValue::Bytes(v) => ProtoValue::string(base64::engine::general_purpose::STANDARD.encode(v.bytes())),
            v => v,
        }),
    }
}

#[derive(Default)]
struct Writer(Vec<u8>);
impl Writer {
    fn raw(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn tag(&mut self, no: u32, wt: u8) {
        self.uint32(no.wrapping_shl(3) | u32::from(wt));
    }
    fn uint32(&mut self, n: u32) {
        self.uint64(n.into());
    }
    fn uint64(&mut self, mut n: u64) {
        while n > 127 {
            self.0.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        self.0.push(n as u8);
    }
    fn delimited(&mut self, b: &[u8]) {
        self.uint32(b.len() as u32);
        self.raw(b);
    }
}
struct Reader {
    input: ByteView,
    pos: usize,
}
impl Reader {
    fn new(input: ByteView) -> Self {
        Self { input, pos: 0 }
    }
    fn len(&self) -> usize {
        self.input.len()
    }
    fn uint32(&mut self) -> ProtoResult<u32> {
        let mut result = 0u32;
        let mut shift = 0;
        while let Some(byte) = self.input.get(self.pos) {
            self.pos += 1;
            result |= u32::from(byte & 127).wrapping_shl(shift);
            if byte & 128 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 32 {
                return Err(ProtoError::new("Varint exceeds 32 bits"));
            }
        }
        Err(ProtoError::new("Unexpected end of protobuf varint"))
    }
    fn uint64(&mut self) -> ProtoResult<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        while let Some(byte) = self.input.get(self.pos) {
            self.pos += 1;
            result |= u64::from(byte & 127).wrapping_shl(shift);
            if byte & 128 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 64 {
                return Err(ProtoError::new("Varint exceeds 64 bits"));
            }
        }
        Err(ProtoError::new("Unexpected end of protobuf 64-bit varint"))
    }
    fn bytes(&mut self) -> ProtoResult<ByteView> {
        let len = self.uint32()? as usize;
        if len > self.len().saturating_sub(self.pos) {
            return Err(ProtoError::new("Unexpected EOF reading bytes"));
        }
        let out = self.input.slice(self.pos, self.pos + len);
        self.pos += len;
        Ok(out)
    }
    fn string(&mut self) -> ProtoResult<WireString> {
        let bytes = self.bytes()?.bytes();
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        std::str::from_utf8(bytes)
            .map(WireString::from)
            .map_err(|_| ProtoError::type_error("The encoded data was not valid for encoding utf-8"))
    }
    fn fixed<const N: usize>(&mut self, name: &str) -> ProtoResult<[u8; N]> {
        if N > self.len().saturating_sub(self.pos) {
            return Err(ProtoError::new(format!("Unexpected EOF reading {name}")));
        }
        let bytes = self.input.slice(self.pos, self.pos + N).bytes();
        self.pos += N;
        Ok(bytes.try_into().expect("fixed length"))
    }
    fn skip(&mut self, wt: u8) -> ProtoResult<()> {
        match wt {
            0 => {
                self.uint64()?;
            }
            1 | 5 => {
                let n = if wt == 1 { 8 } else { 4 };
                if n > self.len().saturating_sub(self.pos) {
                    return Err(ProtoError::new(if wt == 1 {
                        "Unexpected EOF skipping 64-bit"
                    } else {
                        "Unexpected EOF skipping 32-bit"
                    }));
                }
                self.pos += n;
            }
            2 => {
                self.bytes()?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

pub fn encode_json_value(value: &JsonValue) -> ProtoResult<ByteView> {
    let mut w = Writer::default();
    write_json_value(&mut w, value)?;
    Ok(ByteView::new(w.0))
}
pub fn decode_json_value(value: &ByteView) -> ProtoResult<JsonValue> {
    read_json_value(&mut Reader::new(value.clone()))
}
fn write_json_value(w: &mut Writer, v: &JsonValue) -> ProtoResult<()> {
    match v {
        ProtoValue::Null => {
            w.tag(1, 0);
            w.uint32(0);
        }
        ProtoValue::Number(v) => {
            w.tag(2, 1);
            w.raw(&v.to_le_bytes());
        }
        ProtoValue::String(v) => {
            w.tag(3, 2);
            w.delimited(String::from_utf16_lossy(v.units()).as_bytes());
        }
        ProtoValue::Bool(v) => {
            w.tag(4, 0);
            w.0.push(u8::from(*v));
        }
        ProtoValue::Array(items) => {
            let mut list = Writer::default();
            for item in items.lock().expect("proto array poisoned").iter() {
                let mut item_w = Writer::default();
                write_json_value(&mut item_w, item)?;
                list.tag(1, 2);
                list.delimited(&item_w.0);
            }
            w.tag(6, 2);
            w.delimited(&list.0);
        }
        ProtoValue::Object(obj) => {
            let mut structure = Writer::default();
            for (key, value) in obj.enumerable_entries() {
                let mut entry = Writer::default();
                entry.tag(1, 2);
                entry.delimited(String::from_utf16_lossy(key.units()).as_bytes());
                entry.tag(2, 2);
                let mut item = Writer::default();
                write_json_value(&mut item, &value)?;
                entry.delimited(&item.0);
                structure.tag(1, 2);
                structure.delimited(&entry.0);
            }
            w.tag(5, 2);
            w.delimited(&structure.0);
        }
        _ => {}
    }
    Ok(())
}
fn read_json_value(r: &mut Reader) -> ProtoResult<JsonValue> {
    let mut value = ProtoValue::Null;
    while r.pos < r.len() {
        let tag = r.uint32()?;
        let no = tag >> 3;
        let wt = (tag & 7) as u8;
        if !valid_wire(wt) {
            return Err(ProtoError::new(format!("Unsupported wire type {wt} in google.protobuf.Value")));
        }
        match no {
            1 => {
                assert_wire(wt, 0)?;
                r.uint32()?;
                value = ProtoValue::Null;
            }
            2 => {
                assert_wire(wt, 1)?;
                value = ProtoValue::Number(f64::from_le_bytes(r.fixed::<8>("double")?));
            }
            3 => {
                assert_wire(wt, 2)?;
                value = ProtoValue::String(r.string()?);
            }
            4 => {
                assert_wire(wt, 0)?;
                value = ProtoValue::Bool(r.uint32()? != 0);
            }
            5 => {
                assert_wire(wt, 2)?;
                value = read_json_struct(&mut Reader::new(r.bytes()?))?;
            }
            6 => {
                assert_wire(wt, 2)?;
                value = read_json_list(&mut Reader::new(r.bytes()?))?;
            }
            _ => r.skip(wt)?,
        }
    }
    Ok(value)
}
fn read_json_struct(r: &mut Reader) -> ProtoResult<JsonValue> {
    let out = ProtoMessage::new(true);
    while r.pos < r.len() {
        let tag = r.uint32()?;
        let no = tag >> 3;
        let wt = (tag & 7) as u8;
        if !valid_wire(wt) {
            return Err(ProtoError::new(format!("Unsupported wire type {wt} in Struct")));
        }
        if no == 1 {
            assert_wire(wt, 2)?;
            let mut entry = Reader::new(r.bytes()?);
            let mut key = WireString::from("");
            let mut val = ProtoValue::Null;
            while entry.pos < entry.len() {
                let tag = entry.uint32()?;
                let no = tag >> 3;
                let wt = (tag & 7) as u8;
                if no == 1 {
                    key = entry.string()?;
                } else if no == 2 {
                    val = read_json_value(&mut Reader::new(entry.bytes()?))?;
                } else if valid_wire(wt) {
                    entry.skip(wt)?;
                }
            }
            out.reflect_set(key, val);
        } else {
            r.skip(wt)?;
        }
    }
    Ok(ProtoValue::Object(out))
}
fn read_json_list(r: &mut Reader) -> ProtoResult<JsonValue> {
    let mut out = vec![];
    while r.pos < r.len() {
        let tag = r.uint32()?;
        let no = tag >> 3;
        let wt = (tag & 7) as u8;
        if !valid_wire(wt) {
            return Err(ProtoError::new(format!("Unsupported wire type {wt} in ListValue")));
        }
        if no == 1 {
            assert_wire(wt, 2)?;
            out.push(read_json_value(&mut Reader::new(r.bytes()?))?);
        } else {
            r.skip(wt)?;
        }
    }
    Ok(ProtoValue::array(out))
}

/// Generated constants are public codec handles, independent of runtime JS.
#[derive(Debug, Clone, Copy)]
pub struct SchemaHandle {
    pub module: &'static str,
    pub type_name: &'static str,
}
impl SchemaHandle {
    pub const fn new(module: &'static str, type_name: &'static str) -> Self {
        Self { module, type_name }
    }
    pub fn codec(&self) -> MessageCodec {
        schema_registry()
            .get(&(self.module.to_owned(), self.type_name.to_owned()))
            .expect("complete generated schema inventory")
            .clone()
    }
    pub fn create(&self, v: Option<&ProtoMessage>) -> ProtoMessage {
        self.codec().create(v)
    }
    pub fn encode(&self, v: &ProtoMessage) -> ProtoResult<ByteView> {
        self.codec().encode(v)
    }
    pub fn decode(&self, v: &ByteView) -> ProtoResult<ProtoMessage> {
        self.codec().decode(v)
    }
    pub fn to_json(&self, v: &ProtoMessage) -> ProtoResult<ProtoValue> {
        self.codec().to_json(v)
    }
}
pub fn all_schemas() -> Vec<(String, String, MessageCodec)> {
    let mut values: Vec<_> = schema_registry().iter().map(|((m, n), v)| (m.clone(), n.clone(), v.clone())).collect();
    values.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    values
}
fn schema_registry() -> &'static HashMap<(String, String), MessageCodec> {
    static REGISTRY: OnceLock<HashMap<(String, String), MessageCodec>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let ir: serde_json::Value =
            serde_json::from_str(crate::catalog_proto_schemas::IR).expect("pinned generated IR");
        let mut result = HashMap::new();
        for module in ir["modules"].as_array().expect("IR modules") {
            let module_name = module["module"].as_str().expect("IR module");
            for schema in module["schemas"].as_array().expect("IR schemas") {
                let fields =
                    schema["fields"].as_array().expect("IR fields").iter().map(|f| ir_field(module_name, f)).collect();
                result.insert(
                    (module_name.to_owned(), schema["typeName"].as_str().expect("IR name").to_owned()),
                    pb(schema["typeName"].as_str().expect("IR name"), fields),
                );
            }
        }
        result
    })
}
fn ir_reference(module: &str, name: &str) -> MessageReference {
    let module = module.to_owned();
    let name = name.to_owned();
    MessageReference::new(move || schema_registry()[&(module.clone(), name.clone())].clone())
}
fn ir_kind(module: &str, f: &serde_json::Value) -> ValueKind {
    if f["kind"] == "message" {
        ValueKind::Message(ir_reference(module, f["T"].as_str().expect("IR T")))
    } else {
        ValueKind::Scalar(ScalarKind::parse(f["kind"].as_str().expect("IR kind")))
    }
}
fn ir_field(module: &str, f: &serde_json::Value) -> FieldDesc {
    let name = WireString::from(f["name"].as_str().expect("IR field name"));
    let no = f["no"].as_u64().unwrap_or(0) as u32;
    let repeat = f["repeat"] == true;
    match f["kind"].as_str().expect("IR kind") {
        "oneof" => FieldDesc::Oneof {
            name,
            variants: f["variants"]
                .as_array()
                .expect("IR variants")
                .iter()
                .map(|v| VariantDesc {
                    no: v["no"].as_u64().expect("IR number") as u32,
                    name: v["name"].as_str().expect("IR variant name").into(),
                    kind: ir_kind(module, v),
                })
                .collect(),
        },
        "map" => FieldDesc::Map {
            no,
            name,
            value: if f["messageValue"] == true {
                ValueKind::Message(ir_reference(module, f["V"].as_str().expect("IR V")))
            } else {
                ValueKind::Scalar(ScalarKind::parse(f["V"].as_str().expect("IR V")))
            },
        },
        "message" => {
            FieldDesc::Message { no, name, reference: ir_reference(module, f["T"].as_str().expect("IR T")), repeat }
        }
        s => FieldDesc::Scalar { no, name, kind: ScalarKind::parse(s), optional: f["optional"] == true, repeat },
    }
}
