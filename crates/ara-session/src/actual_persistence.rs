//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d:
//! session-persistence.ts, blob-store.ts and anthropic-wire.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//!
//! Same-realm input/output handles remain canonical. The Host owns native
//! Buffer conversion, synchronous blob publication and optional resource cap.
//! Serialization observes the returned source; it never exports/imports serde.

use ara_rpc::WireString;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, PropertyKey, StringifyMode, ValueError, ValueHandle,
    ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;

pub const MAX_PERSIST_CHARS: usize = 500_000;
pub const TRUNCATION_NOTICE: &str = "\n\n[Session persistence truncated large content]";
pub const BLOB_EXTERNALIZE_THRESHOLD: usize = 1024;
const BLOB_PREFIX: &str = "blob:sha256:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceBufferEncoding {
    Base64,
    Utf8,
}

/// Native boundary. Preserve the Host throw or explicit unsupported carrier;
/// an I/O error cannot become a ref. Bytes must precede the journal reference.
/// This does not require per-tool/per-partial fsync.
pub trait ActualPersistenceBlobs: Send + Sync {
    fn buffer_from(
        &self,
        heap: &Arc<ValueHeap>,
        data: &ValueHandle,
        encoding: PersistenceBufferEncoding,
        runtime: &dyn ValueRuntime,
    ) -> Result<Vec<u8>, ValueError>;
    fn put_sync(&self, bytes: &[u8], extension: Option<&WireString>) -> Result<WireString, ValueError>;
}

/// Create once at Host realm/module admission, not once per entry. The native
/// ordinary MIME lookup object retains inherited getter/receiver behavior.
pub struct ActualPersistenceModule {
    heap: Arc<ValueHeap>,
    image_extension_by_mime: ValueHandle,
}
impl ActualPersistenceModule {
    pub fn new(heap: Arc<ValueHeap>) -> Result<Self, ValueError> {
        let table = heap.object()?;
        for (mime, extension) in [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/jpg", "jpg"),
            ("image/gif", "gif"),
            ("image/webp", "webp"),
            ("image/svg+xml", "svg"),
        ] {
            heap.define_own_data(&table, mime.into(), heap.string(extension.into())?, &ara_rpc::value_heap::NoRuntime)?;
        }
        Ok(Self { heap, image_extension_by_mime: table })
    }
    pub fn heap(&self) -> &Arc<ValueHeap> {
        &self.heap
    }

    /// Explicit frames avoid Rust stack abort. None imposes no artificial depth
    /// cap on a finite input. Some retains the Host's explicit frame budget;
    /// it is not a universal JavaScript stack-overflow threshold. A Host that
    /// admits indefinitely recursive input must supply its resource policy.
    pub fn prepare_entry(
        &self,
        entry: &ValueHandle,
        blobs: &dyn ActualPersistenceBlobs,
        runtime: &dyn ValueRuntime,
        native_call_depth: Option<usize>,
    ) -> Result<ValueHandle, ValueError> {
        self.heap.adopt(entry)?;
        let read = Read { heap: &self.heap, runtime };
        let entry = strip_replayed_reasoning_signatures(read, entry)?;
        Truncator { module: self, blobs, read, native_call_depth }.truncate(entry)
    }
    /// Fixed #lineFor: preprocess, then stringifyJson with BigInt replacer.
    pub fn line_for(
        &self,
        entry: &ValueHandle,
        blobs: &dyn ActualPersistenceBlobs,
        runtime: &dyn ValueRuntime,
        native_call_depth: Option<usize>,
    ) -> Result<WireString, ValueError> {
        let prepared = self.prepare_entry(entry, blobs, runtime, native_call_depth)?;
        let mut line = self
            .heap
            .stringify(&prepared, StringifyMode::BigIntReplacer, None, runtime)?
            .unwrap_or_else(|| "null".into());
        line.append_str("\n");
        Ok(line)
    }
}

#[derive(Clone, Copy)]
struct Read<'a> {
    heap: &'a Arc<ValueHeap>,
    runtime: &'a dyn ValueRuntime,
}
impl Read<'_> {
    fn get(self, value: &ValueHandle, name: &str) -> Result<ValueHandle, ValueError> {
        self.heap.get(value, &name.into(), self.runtime)
    }
    fn has(self, value: &ValueHandle, name: &str) -> Result<bool, ValueError> {
        self.heap.has(value, &name.into(), self.runtime)
    }
    fn own(self, target: &ValueHandle, name: &str, value: ValueHandle) -> Result<(), ValueError> {
        self.heap.define_own_data(target, name.into(), value, self.runtime)
    }
    fn method(self, receiver: &ValueHandle, name: &str, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
        let method = self.get(receiver, name)?;
        self.heap.call(&method, receiver, args, self.runtime)
    }
    fn text(self, value: &ValueHandle, expected: &str) -> Result<bool, ValueError> {
        Ok(matches!(self.heap.snapshot(value)?, ValueSnapshot::String(text) if text.equals_ascii(expected)))
    }
    fn string(self, value: &ValueHandle) -> Result<Option<WireString>, ValueError> {
        Ok(match self.heap.snapshot(value)? {
            ValueSnapshot::String(text) => Some(text),
            _ => None,
        })
    }
    fn object(self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Object | ValueKind::Array))
    }
    fn nullish(self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Undefined))
    }
    fn nonempty(self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.string(value)?.is_some_and(|text| !text.is_empty()))
    }
    fn equal(self, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
        match (self.heap.snapshot(left)?, self.heap.snapshot(right)?) {
            (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => Ok(a == b),
            _ => self.heap.object_is(left, right),
        }
    }
    fn key(self, value: &ValueHandle) -> Result<PropertyKey, ValueError> {
        let value = self.heap.to_primitive(value, true, self.runtime)?;
        if self.heap.kind(&value)? == ValueKind::Symbol {
            Ok(PropertyKey::Symbol(self.heap.symbol_key(&value)?))
        } else {
            Ok(self.heap.to_string(&value, self.runtime)?.into())
        }
    }
    fn close(self, iterator: &mut IteratorRecord, error: ValueError) -> ValueError {
        match self.heap.iterator_close(iterator, self.runtime) {
            Ok(()) => error,
            Err(close) if error.is_native_exception() && close.is_native_exception() => error,
            Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
        }
    }
    fn pair(self, pair: &ValueHandle, count: usize) -> Result<Vec<ValueHandle>, ValueError> {
        let mut iterator = self.heap.get_iterator(pair, self.runtime)?;
        let result = (|| {
            let mut values = Vec::new();
            for _ in 0..count {
                values.push(self.heap.iterator_next(&mut iterator, self.runtime)?.unwrap_or(self.heap.undefined()?));
            }
            self.heap.iterator_close(&mut iterator, self.runtime)?;
            Ok(values)
        })();
        result.map_err(|error| self.close(&mut iterator, error))
    }
    fn from_entries(self, entries: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let object = self.heap.object()?;
        let mut iterator = self.heap.get_iterator(entries, self.runtime)?;
        while let Some(pair) = self.heap.iterator_next(&mut iterator, self.runtime)? {
            let result = (|| {
                if !matches!(self.heap.kind(&pair)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                    return Err(ValueError::TypeError("Iterator value is not an entry object".into()));
                }
                let key = self.get(&pair, "0")?;
                let value = self.get(&pair, "1")?;
                self.heap.define_own_data(&object, self.key(&key)?, value, self.runtime)
            })();
            if let Err(error) = result {
                return Err(self.close(&mut iterator, error));
            }
        }
        Ok(object)
    }
    fn implicit(self, value: &ValueHandle) -> Result<WireString, ValueError> {
        if self.heap.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
        }
        self.heap.to_string(value, self.runtime)
    }
    fn above(self, value: &ValueHandle, bound: f64) -> Result<bool, ValueError> {
        let value = self.heap.to_primitive(value, false, self.runtime)?;
        if let ValueSnapshot::BigInt(decimal) = self.heap.snapshot(&value)? {
            return Ok(match decimal.as_str().parse::<i128>() {
                Ok(number) => (number as f64) > bound,
                Err(_) => !decimal.as_str().starts_with('-'),
            });
        }
        Ok(self.heap.to_number(&value, self.runtime)? > bound)
    }
    fn at_least(self, value: &ValueHandle, bound: f64) -> Result<bool, ValueError> {
        let value = self.heap.to_primitive(value, false, self.runtime)?;
        if let ValueSnapshot::BigInt(decimal) = self.heap.snapshot(&value)? {
            return Ok(match decimal.as_str().parse::<i128>() {
                Ok(number) => (number as f64) >= bound,
                Err(_) => !decimal.as_str().starts_with('-'),
            });
        }
        Ok(self.heap.to_number(&value, self.runtime)? >= bound)
    }
    fn at_most(self, value: &ValueHandle, bound: f64) -> Result<bool, ValueError> {
        let value = self.heap.to_primitive(value, false, self.runtime)?;
        if let ValueSnapshot::BigInt(decimal) = self.heap.snapshot(&value)? {
            return Ok(match decimal.as_str().parse::<i128>() {
                Ok(number) => (number as f64) <= bound,
                Err(_) => decimal.as_str().starts_with('-'),
            });
        }
        Ok(self.heap.to_number(&value, self.runtime)? <= bound)
    }
    fn number(self, value: &ValueHandle) -> Result<f64, ValueError> {
        self.heap.to_number(value, self.runtime)
    }
}

pub fn is_persistence_truncated_string(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    let read = Read { heap, runtime };
    if heap.kind(value)? != ValueKind::String {
        return Ok(false);
    }
    heap.truthy(&read.method(value, "endsWith", &[heap.string(TRUNCATION_NOTICE.into())?])?)
}
pub fn is_image_block(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    let r = Read { heap, runtime };
    Ok(r.object(value)?
        && heap.kind(value)? != ValueKind::Null
        && r.has(value, "type")?
        && r.text(&r.get(value, "type")?, "image")?
        && r.has(value, "data")?
        && heap.kind(&r.get(value, "data")?)? == ValueKind::String)
}
fn image_mime(r: Read<'_>, value: &ValueHandle) -> Result<bool, ValueError> {
    if r.heap.kind(value)? != ValueKind::String {
        return Ok(false);
    }
    let lower = r.method(value, "toLowerCase", &[])?;
    r.heap.truthy(&r.method(&lower, "startsWith", &[r.heap.string("image/".into())?])?)
}
pub fn is_image_data_payload(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    let r = Read { heap, runtime };
    Ok(r.object(value)?
        && heap.kind(value)? != ValueKind::Null
        && r.has(value, "data")?
        && heap.kind(&r.get(value, "data")?)? == ValueKind::String
        && (is_image_block(heap, value, runtime)?
            || (r.has(value, "mimeType")? && image_mime(r, &r.get(value, "mimeType")?)?)))
}
pub fn is_externalizable_image_position(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    key: Option<&ValueHandle>,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    let r = Read { heap, runtime };
    if !is_image_data_payload(heap, value, runtime)? {
        return Ok(false);
    }
    let key = key.cloned().unwrap_or(heap.undefined()?);
    Ok((r.text(&key, "content")? && is_image_block(heap, value, runtime)?)
        || r.text(&key, "images")?
        || r.text(&key, "frames")?)
}
fn blob_ref(r: Read<'_>, value: &ValueHandle) -> Result<bool, ValueError> {
    r.heap.truthy(&r.method(value, "startsWith", &[r.heap.string(BLOB_PREFIX.into())?])?)
}
fn image_url(r: Read<'_>, value: &ValueHandle) -> Result<bool, ValueError> {
    Ok(r.heap.truthy(&r.method(value, "startsWith", &[r.heap.string("data:image/".into())?])?)?
        && r.heap.truthy(&r.method(value, "includes", &[r.heap.string(";base64,".into())?])?)?)
}
fn array(r: Read<'_>, value: &ValueHandle) -> Result<bool, ValueError> {
    Ok(r.heap.kind(value)? == ValueKind::Array)
}
fn truncate_string(r: Read<'_>, value: &ValueHandle, limit: f64) -> Result<ValueHandle, ValueError> {
    let length = r.get(value, "length")?;
    if r.number(&length)? <= limit {
        return Ok(value.clone());
    }
    let mut cut = r.method(value, "slice", &[r.heap.number(0.0)?, r.heap.number(limit)?])?;
    if r.above(&r.get(&cut, "length")?, 0.0)? {
        let length = r.get(&cut, "length")?;
        let last = r.method(&cut, "charCodeAt", &[r.heap.number(r.number(&length)? - 1.0)?])?;
        // A native charCodeAt is Number, but an override can return BigInt.
        let primitive = r.heap.to_primitive(&last, false, r.runtime)?;
        let lower = match r.heap.snapshot(&primitive)? {
            ValueSnapshot::BigInt(decimal) => match decimal.as_str().parse::<i128>() {
                Ok(number) => number >= 0xd800,
                Err(_) => !decimal.as_str().starts_with('-'),
            },
            _ => r.number(&primitive)? >= 0xd800 as f64,
        };
        if lower && r.at_most(&last, 0xdbff as f64)? {
            cut = r.method(&cut, "slice", &[r.heap.number(0.0)?, r.heap.number(-1.0)?])?;
        }
    }
    Ok(cut)
}

// Retained functions capture only NodeIds, never borrowed BlobStore/runtime.
fn first(heap: &Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
    args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
static SIGNED_THINKING: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:session-persistence.ts:286",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: signed_thinking,
};
fn signed_thinking(
    heap: &Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let r = Read { heap, runtime };
    let block = first(heap, args)?;
    heap.boolean(
        r.text(&r.get(&block, "type")?, "thinking")?
            && heap.kind(&r.get(&block, "thinkingSignature")?)? == ValueKind::String
            && r.nonempty(&r.get(&block, "thinkingSignature")?)?,
    )
}
static STRIP_SIGNATURE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:session-persistence.ts:303",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: strip_signature,
};
fn strip_signature(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let r = Read { heap, runtime };
    let block = first(heap, args)?;
    if !r.text(&r.get(&block, "type")?, "thinking")?
        || heap.kind(&r.get(&block, "thinkingSignature")?)? != ValueKind::String
        || !r.nonempty(&r.get(&block, "thinkingSignature")?)?
    {
        return Ok(block);
    }
    if !signature_covered(r, &r.get(&block, "thinkingSignature")?, &captures[0], &captures[1])? {
        return Ok(block);
    }
    r.own(&captures[2], "value", heap.boolean(true)?)?;
    let out = heap.spread(&block, runtime)?;
    r.own(&out, "thinkingSignature", heap.undefined()?)?;
    Ok(out)
}
static FIND_CONTENT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:session-persistence.ts:205",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: find_content,
};
static FIND_LINE_COUNT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:session-persistence.ts:206",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: find_line_count,
};
fn find_key(
    heap: &Arc<ValueHeap>,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    key: &str,
) -> Result<ValueHandle, ValueError> {
    let r = Read { heap, runtime };
    let pair = first(heap, args)?;
    heap.boolean(r.text(&r.pair(&pair, 1)?[0], key)?)
}
fn find_content(
    heap: &Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    find_key(heap, args, runtime, "content")
}
fn find_line_count(
    heap: &Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    find_key(heap, args, runtime, "lineCount")
}
static UPDATE_LINE_COUNT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:session-persistence.ts:214",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: update_line_count,
};
fn update_line_count(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let r = Read { heap, runtime };
    let pair = first(heap, args)?;
    let pair = r.pair(&pair, 2)?;
    let value = if r.text(&pair[0], "lineCount")? {
        let split = r.method(&captures[0], "split", &[heap.string("\n".into())?])?;
        r.get(&split, "length")?
    } else {
        pair[1].clone()
    };
    heap.array(vec![pair[0].clone(), value])
}
fn reasoning_item(r: Read<'_>, item: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
    let heap = r.heap;
    if heap.kind(item)? == ValueKind::Null || !r.object(item)? {
        return Ok(None);
    }
    if !r.has(item, "type")? || !r.text(&r.get(item, "type")?, "reasoning")? {
        return Ok(None);
    }
    let out = heap.object()?;
    if r.has(item, "encrypted_content")?
        && heap.kind(&r.get(item, "encrypted_content")?)? == ValueKind::String
        && r.nonempty(&r.get(item, "encrypted_content")?)?
    {
        r.own(&out, "encrypted_content", r.get(item, "encrypted_content")?)?;
    }
    if r.has(item, "id")? && heap.kind(&r.get(item, "id")?)? == ValueKind::String && r.nonempty(&r.get(item, "id")?)? {
        r.own(&out, "id", r.get(item, "id")?)?;
    }
    Ok(Some(out))
}
fn signature_covered(
    r: Read<'_>,
    signature: &ValueHandle,
    encrypted: &ValueHandle,
    ids: &ValueHandle,
) -> Result<bool, ValueError> {
    // JSON.parse's ToString belongs inside the source try/catch.
    let parsed = (|| {
        let text = r.implicit(signature)?;
        r.heap.parse_strict_units(&text)
    })();
    let parsed = match parsed {
        Ok(value) => value,
        Err(error) if matches!(&error, ValueError::StrictParse(_)) || error.is_native_exception() => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(reasoning) = reasoning_item(r, &parsed)? else { return Ok(false) };
    if r.heap.truthy(&r.get(&reasoning, "encrypted_content")?)? {
        return r.heap.truthy(&r.method(encrypted, "has", &[r.get(&reasoning, "encrypted_content")?])?);
    }
    if r.heap.truthy(&r.get(&reasoning, "id")?)? {
        return r.heap.truthy(&r.method(ids, "has", &[r.get(&reasoning, "id")?])?);
    }
    Ok(false)
}
fn strip_replayed_reasoning_signatures(r: Read<'_>, entry: &ValueHandle) -> Result<ValueHandle, ValueError> {
    let h = r.heap;
    if !r.text(&r.get(entry, "type")?, "message")?
        || !r.text(&r.get(&r.get(entry, "message")?, "role")?, "assistant")?
    {
        return Ok(entry.clone());
    }
    let message = r.get(entry, "message")?;
    let payload = r.get(&message, "providerPayload")?;
    if r.nullish(&payload)?
        || !r.text(&r.get(&payload, "type")?, "openaiResponsesHistory")?
        || !array(r, &r.get(&payload, "items")?)?
    {
        return Ok(entry.clone());
    }
    let content = r.get(&message, "content")?;
    let some = r.get(&content, "some")?;
    let callback = h.fresh_compiled_function(&SIGNED_THINKING, &[])?;
    if !h.truthy(&h.call(&some, &content, &[callback], r.runtime)?)? {
        return Ok(entry.clone());
    }
    let encrypted = h.new_set(&h.undefined()?, r.runtime)?;
    let ids = h.new_set(&h.undefined()?, r.runtime)?;
    let items = r.get(&payload, "items")?;
    let mut it = h.get_iterator(&items, r.runtime)?;
    while let Some(item) = h.iterator_next(&mut it, r.runtime)? {
        let body = (|| {
            let Some(reasoning) = reasoning_item(r, &item)? else {
                return Ok(());
            };
            if h.truthy(&r.get(&reasoning, "encrypted_content")?)? {
                r.method(&encrypted, "add", &[r.get(&reasoning, "encrypted_content")?])?;
            }
            if h.truthy(&r.get(&reasoning, "id")?)? {
                r.method(&ids, "add", &[r.get(&reasoning, "id")?])?;
            }
            Ok::<_, ValueError>(())
        })();
        if let Err(error) = body {
            return Err(r.close(&mut it, error));
        }
    }
    if r.equal(&r.get(&encrypted, "size")?, &h.number(0.0)?)? && r.equal(&r.get(&ids, "size")?, &h.number(0.0)?)? {
        return Ok(entry.clone());
    }
    let changed = h.object()?;
    r.own(&changed, "value", h.boolean(false)?)?;
    let content = r.get(&message, "content")?;
    let map = r.get(&content, "map")?;
    let callback = h.fresh_compiled_function(&STRIP_SIGNATURE, &[encrypted, ids, changed.clone()])?;
    let content = h.call(&map, &content, &[callback], r.runtime)?;
    if !h.truthy(&r.get(&changed, "value")?)? {
        return Ok(entry.clone());
    }
    // Evaluation order: spread entry, spread message, replace content/message.
    let out = h.spread(entry, r.runtime)?;
    let body = h.spread(&message, r.runtime)?;
    r.own(&body, "content", content)?;
    r.own(&out, "message", body)?;
    Ok(out)
}

struct ArrayFrame {
    source: ValueHandle,
    key: Option<ValueHandle>,
    result: ValueHandle,
    index: u64,
    changed: bool,
    item: ValueHandle,
}
struct ObjectFrame {
    source: ValueHandle,
    entries: ValueHandle,
    iterator: IteratorRecord,
    key: ValueHandle,
    value: ValueHandle,
    changed: bool,
}
enum Frame {
    Array(ArrayFrame),
    Object(ObjectFrame),
}
struct Truncator<'a> {
    module: &'a ActualPersistenceModule,
    blobs: &'a dyn ActualPersistenceBlobs,
    read: Read<'a>,
    native_call_depth: Option<usize>,
}
impl Truncator<'_> {
    fn extension(&self, mime: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let r = self.read;
        let h = r.heap;
        if !h.truthy(mime)? {
            return h.undefined();
        }
        let lower = r.method(mime, "toLowerCase", &[])?;
        let known = h.get(&self.module.image_extension_by_mime, &r.key(&lower)?, r.runtime)?;
        if h.truthy(&known)? {
            return Ok(known);
        }
        if !h.truthy(&r.method(&lower, "startsWith", &[h.string("image/".into())?])?)? {
            return h.undefined();
        }
        let sliced = r.method(&lower, "slice", &[h.number(6.0)?])?;
        let parts = r.method(&sliced, "split", &[h.string(";".into())?])?;
        let first = r.get(&parts, "0")?;
        let subtype = if r.nullish(&first)? {
            h.undefined()?
        } else {
            let parts = r.method(&first, "split", &[h.string("+".into())?])?;
            r.get(&parts, "0")?
        };
        self.normalize_extension(&subtype)
    }
    fn normalize_extension(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let r = self.read;
        let h = r.heap;
        if !h.truthy(value)? {
            return h.undefined();
        }
        let normalized = if h.truthy(&r.method(value, "startsWith", &[h.string(".".into())?])?)? {
            r.method(value, "slice", &[h.number(1.0)?])?
        } else {
            value.clone()
        };
        if r.equal(&r.get(&normalized, "length")?, &h.number(0.0)?)? || r.above(&r.get(&normalized, "length")?, 32.0)? {
            return h.undefined();
        }
        // A regex literal is fresh on every original evaluation, not cached.
        let regexp = h.regexp(r"^[a-zA-Z0-9][a-zA-Z0-9._-]*$".into(), "")?;
        if !h.truthy(&r.method(&regexp, "test", std::slice::from_ref(&normalized))?)? {
            return h.undefined();
        }
        r.method(&normalized, "toLowerCase", &[])
    }
    fn externalize(
        &self,
        data: &ValueHandle,
        mime: Option<&ValueHandle>,
        encoding: PersistenceBufferEncoding,
    ) -> Result<ValueHandle, ValueError> {
        let r = self.read;
        if blob_ref(r, data)? {
            return Ok(data.clone());
        }
        // Buffer conversion precedes MIME/putSync; faults do not run extension
        // getters or publish a ref. The Host receives original carrier/runtime.
        let bytes = self.blobs.buffer_from(r.heap, data, encoding, r.runtime)?;
        let extension = match (encoding, mime) {
            (PersistenceBufferEncoding::Base64, Some(mime)) => self.extension(mime)?,
            _ => r.heap.undefined()?,
        };
        // BlobStore.putSync normalizes options.extension a second time.
        let extension = self.normalize_extension(&extension)?;
        let extension = if r.heap.truthy(&extension)? { Some(r.implicit(&extension)?) } else { None };
        r.heap.string(self.blobs.put_sync(&bytes, extension.as_ref())?)
    }
    fn atomic(&self, obj: &ValueHandle) -> Result<bool, ValueError> {
        let r = self.read;
        let h = r.heap;
        if r.object(obj)?
            && r.has(obj, "type")?
            && r.text(&r.get(obj, "type")?, "anthropicServerTool")?
            && r.has(obj, "block")?
        {
            let block = r.get(obj, "block")?;
            if r.object(&block)?
                && h.kind(&block)? != ValueKind::Null
                && r.has(&block, "type")?
                && h.kind(&r.get(&block, "type")?)? == ValueKind::String
            {
                let view = h.object()?;
                r.own(&view, "type", r.get(&block, "type")?)?;
                for field in ["name", "id", "tool_use_id", "content"] {
                    if r.has(&block, field)? {
                        r.own(&view, field, r.get(&block, field)?)?;
                    }
                }
                if r.text(&r.get(&view, "type")?, "server_tool_use")? {
                    let name = r.text(&r.get(&view, "name")?, "web_search")?
                        || r.text(&r.get(&view, "name")?, "tool_search_tool_regex")?
                        || r.text(&r.get(&view, "name")?, "tool_search_tool_bm25")?;
                    if name && h.kind(&r.get(&view, "id")?)? == ValueKind::String && r.nonempty(&r.get(&view, "id")?)? {
                        return Ok(true);
                    }
                } else if (r.text(&r.get(&view, "type")?, "web_search_tool_result")?
                    || r.text(&r.get(&view, "type")?, "tool_search_tool_result")?)
                    && h.kind(&r.get(&view, "tool_use_id")?)? == ValueKind::String
                    && r.nonempty(&r.get(&view, "tool_use_id")?)?
                    && h.has_own(&view, &"content".into(), r.runtime)?
                {
                    return Ok(true);
                }
            }
        }
        if r.object(obj)? && r.has(obj, "type")? {
            let signed = (r.text(&r.get(obj, "type")?, "thinking")?
                && r.has(obj, "thinkingSignature")?
                && r.nonempty(&r.get(obj, "thinkingSignature")?)?)
                || (r.text(&r.get(obj, "type")?, "text")?
                    && r.has(obj, "textSignature")?
                    && r.nonempty(&r.get(obj, "textSignature")?)?)
                || (r.text(&r.get(obj, "type")?, "toolCall")?
                    && r.has(obj, "thoughtSignature")?
                    && r.nonempty(&r.get(obj, "thoughtSignature")?)?);
            let redacted = r.text(&r.get(obj, "type")?, "redactedThinking")?
                && r.has(obj, "data")?
                && r.nonempty(&r.get(obj, "data")?)?;
            let encrypted = r.text(&r.get(obj, "type")?, "reasoning")?
                && r.has(obj, "encrypted_content")?
                && r.nonempty(&r.get(obj, "encrypted_content")?)?;
            return Ok(signed || redacted || encrypted);
        }
        Ok(false)
    }
    fn leaf(&self, obj: &ValueHandle, key: Option<&ValueHandle>) -> Result<Option<ValueHandle>, ValueError> {
        let r = self.read;
        let h = r.heap;
        if r.nullish(obj)? {
            return Ok(Some(obj.clone()));
        }
        if r.object(obj)?
            && r.has(obj, "type")?
            && r.text(&r.get(obj, "type")?, "image_generation_call")?
            && r.has(obj, "result")?
            && h.kind(&r.get(obj, "result")?)? == ValueKind::String
            && !blob_ref(r, &r.get(obj, "result")?)?
            && r.at_least(&r.get(&r.get(obj, "result")?, "length")?, BLOB_EXTERNALIZE_THRESHOLD as f64)?
        {
            let out = h.spread(obj, r.runtime)?;
            let data = r.get(obj, "result")?;
            r.own(&out, "result", self.externalize(&data, None, PersistenceBufferEncoding::Base64)?)?;
            return Ok(Some(out));
        }
        if is_externalizable_image_position(h, obj, key, r.runtime)?
            && !blob_ref(r, &r.get(obj, "data")?)?
            && r.at_least(&r.get(&r.get(obj, "data")?, "length")?, BLOB_EXTERNALIZE_THRESHOLD as f64)?
        {
            let out = h.spread(obj, r.runtime)?;
            let data = r.get(obj, "data")?;
            let mime = r.get(obj, "mimeType")?;
            r.own(&out, "data", self.externalize(&data, Some(&mime), PersistenceBufferEncoding::Base64)?)?;
            return Ok(Some(out));
        }
        if self.atomic(obj)? {
            return Ok(Some(obj.clone()));
        }
        if h.kind(obj)? == ValueKind::String {
            let key = key.cloned().unwrap_or(h.undefined()?);
            if r.text(&key, "image_url")? && image_url(r, obj)? {
                return self.externalize(obj, None, PersistenceBufferEncoding::Utf8).map(Some);
            }
            if r.above(&r.get(obj, "length")?, MAX_PERSIST_CHARS as f64)? {
                if r.text(&key, "thinkingSignature")?
                    || r.text(&key, "thoughtSignature")?
                    || r.text(&key, "textSignature")?
                {
                    return Ok(Some(obj.clone()));
                }
                let notice = TRUNCATION_NOTICE.encode_utf16().count();
                let cut = truncate_string(r, obj, MAX_PERSIST_CHARS.saturating_sub(notice) as f64)?;
                let mut text = r.implicit(&cut)?;
                text.append_str(TRUNCATION_NOTICE);
                return Ok(Some(h.string(text)?));
            }
            return Ok(Some(obj.clone()));
        }
        if !r.object(obj)? {
            return Ok(Some(obj.clone()));
        }
        Ok(None)
    }
    fn array_next(&self, frame: &mut ArrayFrame) -> Result<bool, ValueError> {
        let r = self.read;
        let length = r.get(&frame.source, "length")?;
        if !r.above(&length, frame.index as f64)? {
            return Ok(false);
        }
        frame.item = r.get(&frame.source, &frame.index.to_string())?;
        Ok(true)
    }
    fn object_next(&self, frame: &mut ObjectFrame) -> Result<bool, ValueError> {
        let r = self.read;
        while let Some(pair) = r.heap.iterator_next(&mut frame.iterator, r.runtime)? {
            let result = (|| {
                let pair = r.pair(&pair, 2)?;
                if r.text(&pair[0], "jsonlEvents")? {
                    frame.changed = true;
                    return Ok(false);
                }
                frame.key = pair[0].clone();
                frame.value = pair[1].clone();
                Ok(true)
            })();
            match result {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(error) => return Err(r.close(&mut frame.iterator, error)),
            }
        }
        Ok(false)
    }
    fn finish_object(&self, frame: &ObjectFrame) -> Result<ValueHandle, ValueError> {
        let r = self.read;
        let h = r.heap;
        if !frame.changed {
            return Ok(frame.source.clone());
        }
        let method = r.get(&frame.entries, "find")?;
        let callback = h.fresh_compiled_function(&FIND_CONTENT, &[])?;
        let content = h.call(&method, &frame.entries, &[callback], r.runtime)?;
        let method = r.get(&frame.entries, "find")?;
        let callback = h.fresh_compiled_function(&FIND_LINE_COUNT, &[])?;
        let count = h.call(&method, &frame.entries, &[callback], r.runtime)?;
        if h.truthy(&content)?
            && h.kind(&r.get(&content, "1")?)? == ValueKind::String
            && h.truthy(&count)?
            && h.kind(&r.get(&count, "1")?)? == ValueKind::Number
        {
            let text = r.get(&content, "1")?;
            let method = r.get(&frame.entries, "map")?;
            let callback = h.fresh_compiled_function(&UPDATE_LINE_COUNT, &[text])?;
            let updated = h.call(&method, &frame.entries, &[callback], r.runtime)?;
            return r.from_entries(&updated);
        }
        r.from_entries(&frame.entries)
    }
    fn unwind(&self, frames: &mut [Frame], mut error: ValueError) -> ValueError {
        for frame in frames.iter_mut().rev() {
            if let Frame::Object(frame) = frame {
                error = self.read.close(&mut frame.iterator, error);
            }
        }
        error
    }
    fn truncate(&self, root: ValueHandle) -> Result<ValueHandle, ValueError> {
        let r = self.read;
        let h = r.heap;
        let mut frames = Vec::<Frame>::new();
        let mut current = root;
        let mut key = None;
        loop {
            let step = (|| {
                if self.native_call_depth.is_some_and(|depth| frames.len() >= depth) {
                    return Err(ValueError::RangeError("Maximum call stack size exceeded".into()));
                }
                if let Some(value) = self.leaf(&current, key.as_ref())? {
                    return Ok(Some(value));
                }
                if array(r, &current)? {
                    let length = r.get(&current, "length")?;
                    let result = match h.snapshot(&length)? {
                        ValueSnapshot::Number(length)
                            if length.is_finite()
                                && length >= 0.0
                                && length <= u32::MAX as f64
                                && length.fract() == 0.0 =>
                        {
                            h.array_with_length(length as u32)?
                        }
                        ValueSnapshot::Number(_) => return Err(ValueError::RangeError("Invalid array length".into())),
                        _ => h.array(vec![length])?,
                    };
                    let mut frame = ArrayFrame {
                        source: current.clone(),
                        key: key.clone(),
                        result,
                        index: 0,
                        changed: false,
                        item: h.undefined()?,
                    };
                    if !self.array_next(&mut frame)? {
                        return Ok(Some(frame.source));
                    }
                    current = frame.item.clone();
                    key = frame.key.clone();
                    frames.push(Frame::Array(frame));
                    return Ok(None);
                }
                let kept_entries = h.array(vec![])?;
                let entries = h.object_entries(&current, r.runtime)?;
                let iterator = h.get_iterator(&entries, r.runtime)?;
                let mut frame = ObjectFrame {
                    source: current.clone(),
                    entries: kept_entries,
                    iterator,
                    key: h.undefined()?,
                    value: h.undefined()?,
                    changed: false,
                };
                if !self.object_next(&mut frame)? {
                    return self.finish_object(&frame).map(Some);
                }
                current = frame.value.clone();
                key = Some(frame.key.clone());
                frames.push(Frame::Object(frame));
                Ok(None)
            })();
            let mut value = match step {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(error) => return Err(self.unwind(&mut frames, error)),
            };
            loop {
                let Some(mut frame) = frames.pop() else {
                    return Ok(value);
                };
                let completion = (|| match &mut frame {
                    Frame::Array(frame) => {
                        if !r.equal(&value, &frame.item)? {
                            frame.changed = true;
                        }
                        h.assign(
                            &frame.result,
                            WireString::from(frame.index.to_string()).into(),
                            value.clone(),
                            r.runtime,
                        )?;
                        frame.index += 1;
                        if self.array_next(frame)? {
                            current = frame.item.clone();
                            key = frame.key.clone();
                            Ok(None)
                        } else {
                            Ok(Some(if frame.changed { frame.result.clone() } else { frame.source.clone() }))
                        }
                    }
                    Frame::Object(frame) => {
                        if !r.equal(&value, &frame.value)? {
                            frame.changed = true;
                        }
                        let pair = h.array(vec![frame.key.clone(), value.clone()])?;
                        r.method(&frame.entries, "push", &[pair])?;
                        if self.object_next(frame)? {
                            current = frame.value.clone();
                            key = Some(frame.key.clone());
                            Ok(None)
                        } else {
                            self.finish_object(frame).map(Some)
                        }
                    }
                })();
                match completion {
                    Ok(Some(completed)) => value = completed,
                    Ok(None) => {
                        frames.push(frame);
                        break;
                    }
                    Err(error) => {
                        let error = match &mut frame {
                            Frame::Object(frame) => r.close(&mut frame.iterator, error),
                            Frame::Array(_) => error,
                        };
                        return Err(self.unwind(&mut frames, error));
                    }
                }
            }
        }
    }
}

/// String-only default Buffer conversion. Another native carrier requires
/// the Host implementation; do not silently coerce it through serde/String.
pub fn buffer_from_wire_string(
    heap: &Arc<ValueHeap>,
    data: &ValueHandle,
    encoding: PersistenceBufferEncoding,
) -> Result<Vec<u8>, ValueError> {
    let ValueSnapshot::String(data) = heap.snapshot(data)? else {
        return Err(ValueError::UnsupportedOperation("Actual persistence Buffer.from non-string Host carrier"));
    };
    match encoding {
        PersistenceBufferEncoding::Utf8 => {
            let text = char::decode_utf16(data.units().iter().copied())
                .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect::<String>();
            Ok(text.into_bytes())
        }
        PersistenceBufferEncoding::Base64 => {
            use base64::{
                Engine as _, alphabet,
                engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
            };
            let mut cleaned = Vec::new();
            for unit in data.units() {
                let byte = *unit as u8;
                if byte == b'=' {
                    break;
                }
                match byte {
                    b'-' => cleaned.push(b'+'),
                    b'_' => cleaned.push(b'/'),
                    byte if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/') => cleaned.push(byte),
                    _ => {}
                }
            }
            if cleaned.len() % 4 == 1 {
                cleaned.pop();
            }
            let decoder = GeneralPurpose::new(
                &alphabet::STANDARD,
                GeneralPurposeConfig::new()
                    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
                    .with_decode_allow_trailing_bits(true),
            );
            decoder.decode(cleaned).map_err(|_| ValueError::TypeError("Native Buffer base64 conversion failed".into()))
        }
    }
}
