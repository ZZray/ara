//! One graph serializer: actual Get/toJSON/BigInt replacer order, current
//! property reads, string-key enumeration, aliases versus cycle, full UTF16.
//! JSON tokens use the existing WireValue encoder; strict input uses its
//! existing units grammar. The checked serde bridge is a different boundary.

use super::*;

#[derive(Clone, Copy)]
enum JsonReplacer<'a> {
    Fixed(StringifyMode),
    Callable(&'a ValueHandle),
}

#[derive(Clone, Copy)]
struct JsonOptions<'a> {
    replacer: JsonReplacer<'a>,
    property_list: Option<&'a [WireString]>,
    native: bool,
}
impl<'a> JsonOptions<'a> {
    fn legacy(replacer: JsonReplacer<'a>) -> Self {
        Self { replacer, property_list: None, native: false }
    }
}

enum Prepared {
    Omitted,
    Atom(String),
    Container(ValueHandle, ValueKind),
}
enum JsonFrame {
    Object { value: ValueHandle, keys: Vec<WireString>, next: usize, written: bool, depth: usize },
    Array { value: ValueHandle, length: u64, next: u64, depth: usize },
}

impl ValueHeap {
    fn prepare_json(
        self: &Arc<Self>,
        holder: &ValueHandle,
        key: &PropertyKey,
        options: JsonOptions<'_>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Prepared> {
        let mut value = self.get(holder, key, runtime)?;
        if matches!(self.kind(&value)?, ValueKind::Object | ValueKind::Array | ValueKind::Function | ValueKind::BigInt)
        {
            let method = self.get(&value, &"toJSON".into(), runtime)?;
            if self.kind(&method)? == ValueKind::Function {
                let PropertyKey::String(key) = key else { return Err(ValueError::InvalidNode) };
                let argument = self.string(key.clone())?;
                value = self.call(&method, &value, &[argument], runtime)?;
            }
        }
        // Both replacers run after toJSON and before boxed-primitive unboxing.
        match options.replacer {
            JsonReplacer::Callable(function) => {
                let PropertyKey::String(key) = key else { return Err(ValueError::InvalidNode) };
                let argument = self.string(key.clone())?;
                value = self.call(function, holder, &[argument, value], runtime)?;
            }
            JsonReplacer::Fixed(StringifyMode::BigIntReplacer) if self.kind(&value)? == ValueKind::BigInt => {
                // utils/json.ts calls item.toString(); the mutable prototype method,
                // its actual BigInt receiver, arbitrary result and throw are observable.
                let method = self.get(&value, &"toString".into(), runtime)?;
                value = self.call(&method, &value, &[], runtime)?;
            }
            JsonReplacer::Fixed(_) => {}
        }
        if let ValueSnapshot::Opaque { kind, capability, .. } = self.snapshot(&value)? {
            if options.native && kind == OpaqueKind::BoxedNumber {
                value = self.number(self.native_json_number(&value, runtime)?)?;
            } else if options.native && kind == OpaqueKind::BoxedString {
                value = self.string(self.native_json_string(&value, runtime)?)?;
            } else {
                let expected = match kind {
                    OpaqueKind::BoxedNumber => Some(ValueKind::Number),
                    OpaqueKind::BoxedString => Some(ValueKind::String),
                    OpaqueKind::BoxedBoolean => Some(ValueKind::Boolean),
                    OpaqueKind::BoxedBigInt => Some(ValueKind::BigInt),
                    _ => None,
                };
                if let Some(expected) = expected {
                    let primitive = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                        capability,
                        target: &value,
                        receiver: &value,
                        operation: OpaqueOperation::Unbox,
                    }))?;
                    if self.kind(&primitive)? != expected {
                        return Err(ValueError::WrongRuntimeReply("Boxed primitive payload"));
                    }
                    value = primitive;
                    // Native replacer saw the box; a BigInt unboxed afterward still throws.
                }
            }
        }
        Ok(match self.snapshot(&value)? {
            ValueSnapshot::Undefined | ValueSnapshot::Function(_) | ValueSnapshot::Symbol { .. } => Prepared::Omitted,
            ValueSnapshot::Null => Prepared::Atom("null".into()),
            ValueSnapshot::Boolean(value) => Prepared::Atom(WireValue::Bool(value).stringify()),
            ValueSnapshot::Number(value) => Prepared::Atom(WireValue::Number(value).stringify()),
            ValueSnapshot::String(value) => Prepared::Atom(WireValue::String(value).stringify()),
            ValueSnapshot::BigInt(_) => {
                return Err(ValueError::TypeError(
                    if options.native {
                        "JSON.stringify cannot serialize BigInt."
                    } else {
                        "Do not know how to serialize a BigInt"
                    }
                    .into(),
                ));
            }
            ValueSnapshot::Object => Prepared::Container(value, ValueKind::Object),
            ValueSnapshot::Array => Prepared::Container(value, ValueKind::Array),
            ValueSnapshot::Opaque { kind: OpaqueKind::Function | OpaqueKind::Symbol, .. } => Prepared::Omitted,
            ValueSnapshot::Opaque { kind: OpaqueKind::Array, .. } => Prepared::Container(value, ValueKind::Array),
            ValueSnapshot::Opaque { kind: OpaqueKind::Object, .. } => Prepared::Container(value, ValueKind::Object),
            ValueSnapshot::Opaque { .. } => return Err(ValueError::WrongRuntimeReply("Unboxed primitive")),
        })
    }
    /// JSON.stringify graph operations, never a pre-read tree snapshot.
    pub fn stringify(
        self: &Arc<Self>,
        value: &ValueHandle,
        mode: StringifyMode,
        space: Option<&JsonSpace>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<WireString>> {
        self.stringify_json(value, JsonOptions::legacy(JsonReplacer::Fixed(mode)), space, runtime)
    }
    /// Narrow callable-replacer seam for original Native module bodies.
    /// Same-heap Function and Opaque(Function) are admitted before any value Get.
    /// Noncallables are rejected here; stringify_native owns ignored arguments,
    /// ordered property lists and raw Native space conversion.
    pub fn stringify_with_replacer(
        self: &Arc<Self>,
        value: &ValueHandle,
        replacer: &ValueHandle,
        space: Option<&JsonSpace>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<WireString>> {
        self.admit(value)?;
        if self.kind(replacer)? != ValueKind::Function {
            return Err(ValueError::TypeError("JSON callable replacer is not callable".into()));
        }
        self.stringify_json(value, JsonOptions::legacy(JsonReplacer::Callable(replacer)), space, runtime)
    }
    /// Original Native JSON.stringify argument dispatch. Every raw handle is
    /// admitted before replacer/space effects, then one graph encoder is used.
    pub fn stringify_native(
        self: &Arc<Self>,
        value: &ValueHandle,
        replacer: Option<&ValueHandle>,
        space: Option<&ValueHandle>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<WireString>> {
        self.admit(value)?;
        if let Some(replacer) = replacer {
            self.admit(replacer)?;
        }
        if let Some(space) = space {
            self.admit(space)?;
        }
        let mut callable = None;
        let mut property_list = None;
        if let Some(replacer) = replacer {
            match self.kind(replacer)? {
                ValueKind::Function => callable = Some(replacer),
                ValueKind::Array => {
                    let length = self.native_json_length(replacer, runtime)?;
                    let mut keys = Vec::new();
                    let mut seen = HashSet::new();
                    for index in 0..length {
                        let item = self.get(replacer, &json_index_key(index), runtime)?;
                        let key = match self.snapshot(&item)? {
                            ValueSnapshot::String(key) => Some(key),
                            ValueSnapshot::Number(_) => Some(self.to_string(&item, runtime)?),
                            ValueSnapshot::Opaque {
                                kind: OpaqueKind::BoxedString | OpaqueKind::BoxedNumber, ..
                            } => Some(self.native_json_string(&item, runtime)?),
                            _ => None,
                        };
                        if let Some(key) = key {
                            if seen.insert(key.clone()) {
                                keys.push(key);
                            }
                        }
                    }
                    property_list = Some(keys);
                }
                _ => {} // Ignored noncallables have no property/conversion effects.
            }
        }
        // Box conversion and any original throw occur before root Get/toJSON,
        // even when the final root will be omitted.
        let space = match space {
            Some(space) => match self.snapshot(space)? {
                ValueSnapshot::Number(number) => Some(JsonSpace::Number(number)),
                ValueSnapshot::String(text) => Some(JsonSpace::String(text)),
                ValueSnapshot::Opaque { kind: OpaqueKind::BoxedNumber, .. } => {
                    Some(JsonSpace::Number(self.native_json_number(space, runtime)?))
                }
                ValueSnapshot::Opaque { kind: OpaqueKind::BoxedString, .. } => {
                    Some(JsonSpace::String(self.native_json_string(space, runtime)?))
                }
                _ => None,
            },
            None => None,
        };
        let options = JsonOptions {
            replacer: callable.map_or(JsonReplacer::Fixed(StringifyMode::Plain), JsonReplacer::Callable),
            property_list: property_list.as_deref(),
            native: true,
        };
        self.stringify_json(value, options, space.as_ref(), runtime)
    }
    fn native_json_string(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<WireString> {
        let primitive = self.to_primitive(value, true, runtime)?;
        if self.kind(&primitive)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a symbol to a string".into()));
        }
        self.to_string(&primitive, runtime)
    }
    fn native_json_number(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<f64> {
        let primitive = self.to_primitive(value, false, runtime)?;
        match self.kind(&primitive)? {
            ValueKind::Symbol => Err(ValueError::TypeError("Cannot convert a symbol to a number".into())),
            ValueKind::BigInt => {
                Err(ValueError::TypeError("Conversion from 'BigInt' to 'number' is not allowed.".into()))
            }
            _ => self.to_number(&primitive, runtime),
        }
    }
    fn native_json_length(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<u64> {
        let length = self.get(value, &"length".into(), runtime)?;
        let number = self.native_json_number(&length, runtime)?;
        const MAX_SAFE_LENGTH: u64 = 9_007_199_254_740_991;
        Ok(if number.is_nan() || number <= 0.0 {
            0
        } else if number >= MAX_SAFE_LENGTH as f64 {
            MAX_SAFE_LENGTH
        } else {
            number.floor() as u64
        })
    }
    fn stringify_json(
        self: &Arc<Self>,
        value: &ValueHandle,
        options: JsonOptions<'_>,
        space: Option<&JsonSpace>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<WireString>> {
        self.admit(value)?;
        let holder = self.object()?;
        self.define_own_data(&holder, "".into(), value.clone(), &NoRuntime)?;
        let root = self.prepare_json(&holder, &"".into(), options, runtime)?;
        if matches!(root, Prepared::Omitted) {
            return Ok(None);
        }
        let gap = json_gap(space);
        let mut output = Vec::new();
        let mut frames = Vec::new();
        let mut active = HashSet::new();
        self.write_prepared(root, 0, &mut output, &mut frames, &mut active, options, runtime)?;
        while let Some(frame) = frames.pop() {
            match frame {
                JsonFrame::Object { value, keys, mut next, mut written, depth } => {
                    let mut continued = false;
                    while next < keys.len() {
                        let key = &keys[next];
                        next += 1;
                        let prepared =
                            self.prepare_json(&value, &PropertyKey::String(key.clone()), options, runtime)?;
                        if matches!(prepared, Prepared::Omitted) {
                            continue;
                        }
                        if written {
                            output.push(0x2c);
                        }
                        if !gap.is_empty() {
                            output.push(0x0a);
                            indent(&mut output, &gap, depth + 1);
                        }
                        append(&mut output, &WireValue::String(key.clone()).stringify());
                        output.push(0x3a);
                        if !gap.is_empty() {
                            output.push(0x20);
                        }
                        written = true;
                        frames.push(JsonFrame::Object { value: value.clone(), keys, next, written, depth });
                        self.write_prepared(
                            prepared,
                            depth + 1,
                            &mut output,
                            &mut frames,
                            &mut active,
                            options,
                            runtime,
                        )?;
                        continued = true;
                        break;
                    }
                    if !continued {
                        if written && !gap.is_empty() {
                            output.push(0x0a);
                            indent(&mut output, &gap, depth);
                        }
                        output.push(0x7d);
                        active.remove(&value.node);
                    }
                }
                JsonFrame::Array { value, length, next, depth } => {
                    if next == length {
                        if length != 0 && !gap.is_empty() {
                            output.push(0x0a);
                            indent(&mut output, &gap, depth);
                        }
                        output.push(0x5d);
                        active.remove(&value.node);
                        continue;
                    }
                    if next != 0 {
                        output.push(0x2c);
                    }
                    if !gap.is_empty() {
                        output.push(0x0a);
                        indent(&mut output, &gap, depth + 1);
                    }
                    let prepared = self.prepare_json(&value, &json_index_key(next), options, runtime)?;
                    frames.push(JsonFrame::Array { value, length, next: next + 1, depth });
                    match prepared {
                        Prepared::Omitted => append(&mut output, "null"),
                        prepared => self.write_prepared(
                            prepared,
                            depth + 1,
                            &mut output,
                            &mut frames,
                            &mut active,
                            options,
                            runtime,
                        )?,
                    }
                }
            }
        }
        Ok(Some(WireString::from_units(output)))
    }
    fn write_prepared(
        self: &Arc<Self>,
        prepared: Prepared,
        depth: usize,
        output: &mut Vec<u16>,
        frames: &mut Vec<JsonFrame>,
        active: &mut HashSet<NodeId>,
        options: JsonOptions<'_>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        match prepared {
            Prepared::Omitted => return Err(ValueError::InvalidNode),
            Prepared::Atom(text) => append(output, &text),
            Prepared::Container(value, kind) => {
                if !active.insert(self.admit(&value)?) {
                    return Err(ValueError::TypeError(
                        if options.native {
                            "JSON.stringify cannot serialize cyclic structures."
                        } else {
                            "Converting circular structure to JSON"
                        }
                        .into(),
                    ));
                }
                match kind {
                    ValueKind::Array => {
                        let length = if options.native {
                            self.native_json_length(&value, runtime)?
                        } else {
                            u64::from(self.array_length(&value, runtime)?)
                        };
                        output.push(0x5b);
                        frames.push(JsonFrame::Array { value, length, next: 0, depth });
                    }
                    ValueKind::Object => {
                        let keys = match options.property_list {
                            Some(keys) => keys.to_vec(),
                            None => self.own_enumerable_string_keys(&value, runtime)?,
                        };
                        output.push(0x7b);
                        frames.push(JsonFrame::Object { value, keys, next: 0, written: false, depth });
                    }
                    _ => return Err(ValueError::InvalidNode),
                }
            }
        }
        Ok(())
    }
    /// Checked JSON/serde ingress cannot invent source order or numeric precision
    /// that the legacy caller already discarded. It rejects a lossy number import.
    pub fn import_serde(self: &Arc<Self>, source: &serde_json::Value) -> ValueResult<ValueHandle> {
        enum Frame<'a> {
            Value(&'a serde_json::Value),
            Array(usize),
            Object(Vec<WireString>),
        }
        let mut frames = vec![Frame::Value(source)];
        let mut values = Vec::new();
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Value(value) => match value {
                    serde_json::Value::Null => values.push(WireValue::Null),
                    serde_json::Value::Bool(value) => values.push(WireValue::Bool(*value)),
                    serde_json::Value::String(value) => {
                        values.push(WireValue::String(WireString::from(value.as_str())))
                    }
                    serde_json::Value::Number(value) => {
                        let number = value
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .ok_or(ValueError::LegacyBridge("nonfinite number"))?;
                        if value.is_i64() && value.as_i64().is_some_and(|integer| number as i128 != i128::from(integer))
                        {
                            return Err(ValueError::LegacyBridge("integer precision loss"));
                        }
                        if value.is_u64() && value.as_u64().is_some_and(|integer| number as u128 != u128::from(integer))
                        {
                            return Err(ValueError::LegacyBridge("integer precision loss"));
                        }
                        values.push(WireValue::Number(number));
                    }
                    serde_json::Value::Array(array) => {
                        frames.push(Frame::Array(array.len()));
                        for value in array.iter().rev() {
                            frames.push(Frame::Value(value));
                        }
                    }
                    serde_json::Value::Object(object) => {
                        frames.push(Frame::Object(object.keys().map(|key| WireString::from(key.as_str())).collect()));
                        for (_, value) in object.iter().rev() {
                            frames.push(Frame::Value(value));
                        }
                    }
                },
                Frame::Array(length) => {
                    let begin = values.len().checked_sub(length).ok_or(ValueError::InvalidNode)?;
                    let children = values.split_off(begin);
                    values.push(WireValue::Array(children));
                }
                Frame::Object(keys) => {
                    let begin = values.len().checked_sub(keys.len()).ok_or(ValueError::InvalidNode)?;
                    let children = values.split_off(begin);
                    values.push(WireValue::Object(keys.into_iter().zip(children).collect()));
                }
            }
        }
        if values.len() != 1 {
            return Err(ValueError::InvalidNode);
        }
        self.import_strict(values.pop().ok_or(ValueError::InvalidNode)?)
    }
    /// No callbacks occur in this explicit bridge, so one guard gives a
    /// coherent true-tree projection rather than races or lossy JSON fallback.
    pub fn export_serde(self: &Arc<Self>, value: &ValueHandle) -> ValueResult<serde_json::Value> {
        enum Frame {
            Value(NodeId),
            Array(usize),
            Object(Vec<String>),
        }
        let root = self.admit(value)?;
        let arena = self.lock()?;
        let mut frames = vec![Frame::Value(root)];
        let mut values = Vec::new();
        let mut identities = HashSet::new();
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Value(node) => match arena.get(node)? {
                    Node::Null => values.push(serde_json::Value::Null),
                    Node::Boolean(value) => values.push(serde_json::Value::Bool(*value)),
                    Node::String(value) => values.push(serde_json::Value::String(
                        value.to_utf8().map_err(|_| ValueError::LegacyBridge("unpaired UTF16 unit"))?,
                    )),
                    Node::Number(value) => {
                        // Native has one Number type. Emit exact integers in
                        // serde's integer domain for legacy integer consumers,
                        // while preserving fractional values and negative zero.
                        // Half-open bounds avoid Rust's saturating float casts
                        // accepting 2^64 or 2^63 as a different integer.
                        let integral = value.fract() == 0.0 && !(*value == 0.0 && value.is_sign_negative());
                        let number = if integral && *value >= 0.0 && *value < u64::MAX as f64 {
                            serde_json::Number::from(*value as u64)
                        } else if integral && *value < 0.0 && *value >= i64::MIN as f64 {
                            serde_json::Number::from(*value as i64)
                        } else {
                            serde_json::Number::from_f64(*value).ok_or(ValueError::LegacyBridge("nonfinite number"))?
                        };
                        if number.as_f64().is_none_or(|number| number.to_bits() != value.to_bits()) {
                            return Err(ValueError::LegacyBridge("f64 precision or sign loss"));
                        }
                        values.push(serde_json::Value::Number(number));
                    }
                    Node::Object(object) => {
                        if !identities.insert(node) {
                            return Err(ValueError::LegacyBridge("object alias or cycle"));
                        }
                        if object.prototype != Some(self.intrinsics.object_proto) || !object.extensible {
                            return Err(ValueError::LegacyBridge("altered object prototype or extensibility"));
                        }
                        let mut fields = Vec::new();
                        for entry in &object.properties {
                            let PropertyKey::String(key) = &entry.key else {
                                return Err(ValueError::LegacyBridge("symbol property"));
                            };
                            let StoredDescriptor::Data { value, writable: true, enumerable: true, configurable: true } =
                                &entry.descriptor
                            else {
                                return Err(ValueError::LegacyBridge("non-default property descriptor"));
                            };
                            let text =
                                key.to_utf8().map_err(|_| ValueError::LegacyBridge("unpaired key UTF16 unit"))?;
                            fields.push((text, *value, array_index(&entry.key)));
                        }
                        fields.sort_by_key(|(_, _, index)| index.map_or((1, 0), |index| (0, index)));
                        let keys = fields.iter().map(|(key, _, _)| key.clone()).collect::<Vec<_>>();
                        // If this build's serde Map sorts strings, reject lost insertion order.
                        let mut probe = serde_json::Map::new();
                        for key in &keys {
                            probe.insert(key.clone(), serde_json::Value::Null);
                        }
                        let mut roundtrip = probe.keys().cloned().collect::<Vec<_>>();
                        roundtrip.sort_by_key(|key| {
                            array_index(&PropertyKey::from(key.as_str())).map_or((1, 0), |index| (0, index))
                        });
                        if roundtrip != keys {
                            return Err(ValueError::LegacyBridge("property insertion order loss"));
                        }
                        frames.push(Frame::Object(keys));
                        for (_, child, _) in fields.into_iter().rev() {
                            frames.push(Frame::Value(child));
                        }
                    }
                    Node::Array { object, length, length_writable } => {
                        if !identities.insert(node) {
                            return Err(ValueError::LegacyBridge("array alias or cycle"));
                        }
                        if object.prototype != Some(self.intrinsics.array_proto)
                            || !object.extensible
                            || !length_writable
                        {
                            return Err(ValueError::LegacyBridge("altered array prototype or descriptor"));
                        }
                        if object.properties.len() != *length as usize {
                            return Err(ValueError::LegacyBridge("array hole or named/symbol property"));
                        }
                        let mut entries = Vec::new();
                        for entry in &object.properties {
                            let Some(index) = array_index(&entry.key) else {
                                return Err(ValueError::LegacyBridge("array named/symbol property"));
                            };
                            let StoredDescriptor::Data { value, writable: true, enumerable: true, configurable: true } =
                                &entry.descriptor
                            else {
                                return Err(ValueError::LegacyBridge("non-default array descriptor"));
                            };
                            entries.push((index, *value));
                        }
                        entries.sort_unstable_by_key(|(index, _)| *index);
                        if entries.iter().enumerate().any(|(expected, (index, _))| expected != *index as usize) {
                            return Err(ValueError::LegacyBridge("array hole"));
                        }
                        frames.push(Frame::Array(entries.len()));
                        for (_, child) in entries.into_iter().rev() {
                            frames.push(Frame::Value(child));
                        }
                    }
                    _ => return Err(ValueError::LegacyBridge("Undefined, BigInt, Function, Symbol or opaque value")),
                },
                Frame::Array(length) => {
                    let begin = values.len().checked_sub(length).ok_or(ValueError::InvalidNode)?;
                    let children = values.split_off(begin);
                    values.push(serde_json::Value::Array(children));
                }
                Frame::Object(keys) => {
                    let begin = values.len().checked_sub(keys.len()).ok_or(ValueError::InvalidNode)?;
                    let children = values.split_off(begin);
                    values.push(serde_json::Value::Object(keys.into_iter().zip(children).collect()));
                }
            }
        }
        if values.len() != 1 {
            return Err(ValueError::InvalidNode);
        }
        values.pop().ok_or(ValueError::InvalidNode)
    }
}

fn json_index_key(index: u64) -> PropertyKey {
    WireString::from(index.to_string()).into()
}

fn append(output: &mut Vec<u16>, value: &str) {
    output.extend(value.encode_utf16());
}
fn indent(output: &mut Vec<u16>, gap: &[u16], depth: usize) {
    for _ in 0..depth {
        output.extend_from_slice(gap);
    }
}
fn json_gap(space: Option<&JsonSpace>) -> Vec<u16> {
    match space {
        Some(JsonSpace::String(value)) => value.units()[..value.len().min(10)].to_vec(),
        Some(JsonSpace::Number(number)) if *number > 0.0 => vec![0x20; number.trunc().min(10.0) as usize],
        _ => Vec::new(),
    }
}
