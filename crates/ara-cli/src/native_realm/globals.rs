//! Concrete Native globals used by fixed OMP596f2da flags/rate-limit/fetch-retry.
//! Builtin semantics follow ECMAScript conversion/descriptor order. These are
//! Rust function bodies; no Host evaluator or serde/tree classifier is used.

use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
        ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;

mod bun;
mod date;
mod json;
mod mcp_dependencies;

type Result<T> = std::result::Result<T, ValueError>;
const ERROR: usize = 0;
const TYPE: usize = 1;
const SYNTAX: usize = 2;
const RANGE: usize = 3;

#[derive(Clone)]
pub struct NativeErrorClasses {
    prototypes: [ValueHandle; 4],
    pub error: ValueHandle,
    pub type_error: ValueHandle,
    pub syntax_error: ValueHandle,
    pub range_error: ValueHandle,
}
#[derive(Clone)]
pub struct NativeGlobals {
    pub environment: ValueHandle,
    pub errors: NativeErrorClasses,
    pub json: ValueHandle,
    pub array: ValueHandle,
    pub map: ValueHandle,
    pub number: ValueHandle,
    pub string: ValueHandle,
    pub date: ValueHandle,
    pub bun: ValueHandle,
}

fn nonenumerable(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    key: &str,
    value: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    heap.define_own(
        object,
        WireString::from(key).into(),
        PropertyDescriptor::Data {
            value,
            writable: true,
            enumerable: false,
            configurable: true,
        },
        runtime,
    )
}
fn function(
    heap: &Arc<ValueHeap>,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
) -> Result<ValueHandle> {
    heap.fresh_compiled_function(spec, captures)
}
fn argument(heap: &Arc<ValueHeap>, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    args.get(index)
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| heap.undefined())
}
fn implicit_string(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<WireString> {
    if heap.kind(value)? == ValueKind::Symbol {
        return Err(ValueError::TypeError(
            "Cannot convert a Symbol value to a string".into(),
        ));
    }
    heap.to_string(value, runtime)
}
fn new_error(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    kind: usize,
    message: WireString,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let error = heap.object_with_prototype(Some(&prototypes[kind]))?;
    nonenumerable(heap, &error, "message", heap.string(message)?, runtime)?;
    Ok(error)
}
fn original_exception(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    error: ValueError,
    runtime: &dyn ValueRuntime,
) -> ValueError {
    let (kind, message) = match error {
        ValueError::TypeError(message) => (TYPE, message),
        ValueError::SyntaxError(message) => (SYNTAX, message),
        ValueError::RangeError(message) => (RANGE, message),
        // Original thrown handles and all non-Native infrastructure failures
        // preserve their carrier; no Display-driven catch/reconstruction.
        other => return other,
    };
    match new_error(heap, prototypes, kind, message.into(), runtime) {
        Ok(original) => ValueError::Thrown(original),
        Err(failure) => failure,
    }
}
fn native<T>(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    operation().map_err(|error| original_exception(heap, prototypes, error, runtime))
}
impl NativeErrorClasses {
    pub fn create_type_error(
        &self,
        message: WireString,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        new_error(
            self.error.heap(),
            &self.prototypes,
            TYPE,
            message,
            effective_runtime,
        )
    }
    pub fn create_syntax_error(
        &self,
        message: WireString,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        new_error(
            self.error.heap(),
            &self.prototypes,
            SYNTAX,
            message,
            effective_runtime,
        )
    }
    pub fn create_range_error(
        &self,
        message: WireString,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        new_error(
            self.error.heap(),
            &self.prototypes,
            RANGE,
            message,
            effective_runtime,
        )
    }
    /// For a documented native-origin seam only. This does not classify an
    /// arbitrary error/string as native and is not an end-consumer formatter.
    pub fn create_at_native_origin(
        &self,
        error: ValueError,
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueError {
        original_exception(
            self.error.heap(),
            &self.prototypes,
            error,
            effective_runtime,
        )
    }
}

impl NativeGlobals {
    /// Explicit MCP assembly after the same-realm Logger globals are prepared.
    /// The original Object/Bun/Set identities remain shared; only missing MCP
    /// dependency members are supplied, with no replacement of user overrides.
    pub fn install_mcp_dependencies(&self, runtime: &dyn ValueRuntime) -> Result<()> {
        mcp_dependencies::install(self.environment.heap(), self, runtime)
    }

    pub fn prepare(heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<Self> {
        let error_proto = heap.object()?;
        let type_proto = heap.object_with_prototype(Some(&error_proto))?;
        let syntax_proto = heap.object_with_prototype(Some(&error_proto))?;
        let range_proto = heap.object_with_prototype(Some(&error_proto))?;
        let prototypes = [error_proto, type_proto, syntax_proto, range_proto];
        let error = heap.fresh_native_constructor_with_prototype(
            &ERROR_CTOR,
            &prototypes,
            &prototypes[ERROR],
        )?;
        let type_error = heap.fresh_native_constructor_with_prototype(
            &TYPE_CTOR,
            &prototypes,
            &prototypes[TYPE],
        )?;
        let syntax_error = heap.fresh_native_constructor_with_prototype(
            &SYNTAX_CTOR,
            &prototypes,
            &prototypes[SYNTAX],
        )?;
        let range_error = heap.fresh_native_constructor_with_prototype(
            &RANGE_CTOR,
            &prototypes,
            &prototypes[RANGE],
        )?;
        for (i, name, constructor) in [
            (ERROR, "Error", &error),
            (TYPE, "TypeError", &type_error),
            (SYNTAX, "SyntaxError", &syntax_error),
            (RANGE, "RangeError", &range_error),
        ] {
            heap.define_own(
                constructor,
                "prototype".into(),
                PropertyDescriptor::Data {
                    value: prototypes[i].clone(),
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
                runtime,
            )?;
            nonenumerable(
                heap,
                &prototypes[i],
                "constructor",
                constructor.clone(),
                runtime,
            )?;
            nonenumerable(
                heap,
                &prototypes[i],
                "name",
                heap.string(name.into())?,
                runtime,
            )?;
            nonenumerable(
                heap,
                &prototypes[i],
                "message",
                heap.string("".into())?,
                runtime,
            )?;
            if i != ERROR {
                heap.set_prototype(constructor, Some(&error), runtime)?;
            }
        }
        nonenumerable(
            heap,
            &prototypes[ERROR],
            "toString",
            function(heap, &ERROR_TO_STRING, &prototypes)?,
            runtime,
        )?;
        let errors = NativeErrorClasses {
            prototypes: prototypes.clone(),
            error,
            type_error,
            syntax_error,
            range_error,
        };
        let json = heap.object()?;
        nonenumerable(
            heap,
            &json,
            "parse",
            function(heap, &JSON_PARSE, &prototypes)?,
            runtime,
        )?;
        json::install(heap, &json, &prototypes, runtime)?;
        let array = heap.get(&heap.array_prototype(), &"constructor".into(), runtime)?;
        nonenumerable(
            heap,
            &array,
            "isArray",
            function(heap, &ARRAY_IS_ARRAY, &prototypes)?,
            runtime,
        )?;
        // Preserve the original content.indexOf Get/Call and any preinstalled
        // own override. The compiled builtin uses the existing Array operation
        // owner, rather than a Provider-local identity scan.
        let array_prototype = heap.array_prototype();
        if heap
            .own_descriptor(&array_prototype, &"indexOf".into(), runtime)?
            .is_none()
        {
            nonenumerable(
                heap,
                &array_prototype,
                "indexOf",
                function(heap, &ARRAY_INDEX_OF, &prototypes)?,
                runtime,
            )?;
        }
        let map = heap.map_constructor();
        let boolean = function(heap, &BOOLEAN_CALL, &[])?;
        let parse_int = function(heap, &PARSE_INT, &prototypes)?;
        let parse_float = function(heap, &PARSE_FLOAT, &prototypes)?;
        let number = function(heap, &NUMBER_CALL, &prototypes)?;
        // Reuse the realm's existing String.prototype. This selected callable
        // has no construct domain; new String/boxed String remains unavailable.
        let string = function(heap, &STRING_CALL, &prototypes)?;
        let string_prototype = heap.prototype(&heap.string("".into())?, runtime)?.ok_or(
            ValueError::UnsupportedOperation("Native String.prototype is unavailable"),
        )?;
        heap.define_own(
            &string,
            "prototype".into(),
            PropertyDescriptor::Data {
                value: string_prototype.clone(),
                writable: false,
                enumerable: false,
                configurable: false,
            },
            runtime,
        )?;
        nonenumerable(
            heap,
            &string_prototype,
            "constructor",
            string.clone(),
            runtime,
        )?;
        for (name, spec) in [
            ("isFinite", &NUMBER_FINITE),
            ("isNaN", &NUMBER_NAN),
            ("isInteger", &NUMBER_INTEGER),
            ("isSafeInteger", &NUMBER_SAFE_INTEGER),
        ] {
            nonenumerable(
                heap,
                &number,
                name,
                function(heap, spec, &prototypes)?,
                runtime,
            )?;
        }
        nonenumerable(heap, &number, "parseInt", parse_int.clone(), runtime)?;
        nonenumerable(heap, &number, "parseFloat", parse_float.clone(), runtime)?;
        for (name, n) in [
            ("NaN", f64::NAN),
            ("POSITIVE_INFINITY", f64::INFINITY),
            ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("MAX_SAFE_INTEGER", 9_007_199_254_740_991.0),
            ("MIN_SAFE_INTEGER", -9_007_199_254_740_991.0),
            ("EPSILON", f64::EPSILON),
        ] {
            heap.define_own(
                &number,
                WireString::from(name).into(),
                PropertyDescriptor::Data {
                    value: heap.number(n)?,
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
                runtime,
            )?;
        }
        let date = date::prepare(heap, &prototypes, runtime)?;
        let mut date_parse_captures = prototypes.to_vec();
        date_parse_captures.push(heap.function(crate::native_realm::date::DATE_PORT)?);
        nonenumerable(
            heap,
            &date,
            "parse",
            function(heap, &DATE_PARSE, &date_parse_captures)?,
            runtime,
        )?;
        let bun = bun::prepare(heap, &prototypes, runtime)?;
        // Xdev builtin approval consumes Construct(RegExp, [string, string]).
        // Reuse the heap's original regexp slots/prototype/test implementation.
        let regexp_prototype = heap
            .prototype(&heap.regexp("".into(), "")?, runtime)?
            .ok_or(ValueError::UnsupportedOperation(
                "Native RegExp.prototype is unavailable",
            ))?;
        let regexp =
            heap.fresh_native_constructor_with_prototype(&REGEXP, &prototypes, &regexp_prototype)?;
        nonenumerable(
            heap,
            &regexp_prototype,
            "constructor",
            regexp.clone(),
            runtime,
        )?;
        let environment = heap.object()?;
        for (name, value) in [
            ("Error", errors.error.clone()),
            ("TypeError", errors.type_error.clone()),
            ("SyntaxError", errors.syntax_error.clone()),
            ("RangeError", errors.range_error.clone()),
            ("JSON", json.clone()),
            ("Array", array.clone()),
            ("Map", map.clone()),
            ("Boolean", boolean),
            ("Number", number.clone()),
            ("String", string.clone()),
            ("RegExp", regexp),
            ("Date", date.clone()),
            ("Bun", bun.clone()),
            ("parseInt", parse_int),
            ("parseFloat", parse_float),
        ] {
            nonenumerable(heap, &environment, name, value, runtime)?;
        }
        Ok(Self {
            environment,
            errors,
            json,
            array,
            map,
            number,
            string,
            date,
            bun,
        })
    }

    /// Install the original prepared default Promise constructor in this realm.
    /// Domain private brands and state remain in the CLI Promise owner.
    pub fn install_native_promise(
        &self,
        promise: ValueHandle,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<()> {
        let heap = self.environment.heap();
        heap.adopt(&promise)?;
        if !heap.is_constructor(&promise, effective_runtime)? {
            return Err(ValueError::TypeError(
                "Native Promise binding is not a constructor".into(),
            ));
        }
        nonenumerable(
            heap,
            &self.environment,
            "Promise",
            promise,
            effective_runtime,
        )
    }

    pub fn install_original_string(
        &self,
        string: ValueHandle,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<()> {
        let heap = self.environment.heap();
        heap.adopt(&string)?;
        if heap.kind(&string)? != ValueKind::Function {
            return Err(ValueError::TypeError(
                "Original String binding must be callable".into(),
            ));
        }
        nonenumerable(heap, &self.environment, "String", string, effective_runtime)
    }
}

// Fixed xdev approval supplies string patterns and flags. General RegExp Call,
// object/RegExp-copy operands and unsupported heap flags remain explicit gaps.
static REGEXP: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da tools/read.ts PDF/selector approval and bash pattern RegExp construction; original ValueHeap regexp owner",
    name: "RegExp",
    length: 2,
    form: CompiledFunctionForm::NativeConstructor(
        |heap, captures, receiver, arguments, runtime| {
            native(heap, captures, runtime, || {
                let operand = |index: usize| -> Result<WireString> {
                    let value = arguments.get(index).cloned().unwrap_or(heap.undefined()?);
                    match heap.snapshot(&value)? {
                        ValueSnapshot::Undefined => Ok("".into()),
                        ValueSnapshot::String(value) => Ok(value),
                        _ => Err(ValueError::UnsupportedOperation(
                            "Native RegExp constructor operands outside fixed string domain",
                        )),
                    }
                };
                let pattern = operand(0)?;
                let flags = operand(1)?;
                let flags = flags
                    .to_utf8()
                    .map_err(|_| ValueError::SyntaxError("Invalid RegExp flags".into()))?;
                let regexp = heap.regexp(pattern, &flags)?;
                // Native Construct already resolved newTarget.prototype on receiver.
                heap.set_prototype(
                    &regexp,
                    heap.prototype(receiver, runtime)?.as_ref(),
                    runtime,
                )?;
                Ok(regexp)
            })
        },
    ),
    capture_count: 4,
    body: |_, _, _, _, _| {
        Err(ValueError::UnsupportedOperation(
            "Native RegExp callable outside fixed xdev Construct domain",
        ))
    },
};

static BOOLEAN_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Boolean(value) callable; shared Native environment binding for fixed OMP EDIT filter(Boolean)",
    name: "Boolean",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, _, args, _| {
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        heap.boolean(heap.truthy(&value)?)
    },
};

static STRING_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA String(value) callable; direct Symbol description, UTF-16 and original coercion",
    name: "String",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, arguments, runtime| {
        native(heap, prototypes, runtime, || {
            let text = match arguments.first() {
                None => "".into(),
                Some(value) => heap.to_string(value, runtime)?,
            };
            heap.string(text)
        })
    },
};

fn constructor(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    kind: usize,
) -> Result<ValueHandle> {
    native(heap, prototypes, runtime, || {
        let error = heap.object_with_prototype(Some(&prototypes[kind]))?;
        initialize_error_receiver(heap, prototypes, &error, args, runtime)?;
        Ok(error)
    })
}
pub(crate) fn initialize_error_receiver(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, prototypes, runtime, || {
        let error = receiver.clone();
        let message = argument(heap, args, 0)?;
        if heap.kind(&message)? != ValueKind::Undefined {
            nonenumerable(
                heap,
                &error,
                "message",
                heap.string(implicit_string(heap, &message, runtime)?)?,
                runtime,
            )?;
        }
        let options = argument(heap, args, 1)?;
        if matches!(
            heap.kind(&options)?,
            ValueKind::Object | ValueKind::Array | ValueKind::Function
        ) && heap.has(&options, &"cause".into(), runtime)?
        {
            let cause = heap.get(&options, &"cause".into(), runtime)?;
            nonenumerable(heap, &error, "cause", cause, runtime)?;
        }
        Ok(error)
    })
}
macro_rules! ctor {
    ($body:ident,$spec:ident,$name:literal,$kind:ident) => {
        fn $body(
            heap: &Arc<ValueHeap>,
            captures: &[ValueHandle],
            _: &ValueHandle,
            args: &[ValueHandle],
            effective_runtime: &dyn ValueRuntime,
        ) -> Result<ValueHandle> {
            constructor(heap, captures, args, effective_runtime, $kind)
        }
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("ECMA Native ", $name, " constructor consumed by OMP596f2da"),
            name: $name,
            length: 1,
            form: CompiledFunctionForm::NativeConstructor(
                |heap, captures, receiver, args, runtime| {
                    initialize_error_receiver(heap, captures, receiver, args, runtime)
                },
            ),
            capture_count: 4,
            body: $body,
        };
    };
}
ctor!(error_ctor, ERROR_CTOR, "Error", ERROR);
ctor!(type_ctor, TYPE_CTOR, "TypeError", TYPE);
ctor!(syntax_ctor, SYNTAX_CTOR, "SyntaxError", SYNTAX);
ctor!(range_ctor, RANGE_CTOR, "RangeError", RANGE);

fn error_to_string(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, captures, runtime, || {
        if !matches!(
            heap.kind(receiver)?,
            ValueKind::Object | ValueKind::Array | ValueKind::Function
        ) {
            return Err(ValueError::TypeError(
                "Error.prototype.toString called on a non-object".into(),
            ));
        }
        let name = heap.get(receiver, &"name".into(), runtime)?;
        let name = if heap.kind(&name)? == ValueKind::Undefined {
            WireString::from("Error")
        } else {
            implicit_string(heap, &name, runtime)?
        };
        let message = heap.get(receiver, &"message".into(), runtime)?;
        let message = if heap.kind(&message)? == ValueKind::Undefined {
            WireString::from("")
        } else {
            implicit_string(heap, &message, runtime)?
        };
        if name.is_empty() {
            return heap.string(message);
        }
        if message.is_empty() {
            return heap.string(name);
        }
        let mut units = name.units().to_vec();
        units.extend(": ".encode_utf16());
        units.extend_from_slice(message.units());
        heap.string(WireString::from_units(units))
    })
}
static ERROR_TO_STRING: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Error.prototype.toString",
    name: "toString",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: error_to_string,
};

fn json_parse(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, captures, runtime, || {
        let text = implicit_string(heap, &argument(heap, args, 0)?, runtime)?;
        let parsed = match heap.parse_strict_units(&text) {
            Ok(parsed) => parsed,
            Err(ValueError::StrictParse(diagnostic)) => {
                // Explicit strict parser origin. Diagnostic offsets count
                // UTF16 units; this is not an arbitrary Display/error bridge.
                let message = WireString::from(format!(
                    "JSON at UTF-16 unit {}: {}",
                    diagnostic.offset, diagnostic.message
                ));
                return Err(ValueError::Thrown(new_error(
                    heap, captures, SYNTAX, message, runtime,
                )?));
            }
            Err(error) => return Err(error),
        };
        if heap.kind(&argument(heap, args, 1)?)? == ValueKind::Function {
            return Err(ValueError::UnsupportedOperation(
                "Native JSON.parse reviver algorithm is not installed",
            ));
        }
        Ok(parsed)
    })
}
static JSON_PARSE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native JSON.parse strict UTF16 parser/import",
    name: "parse",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: json_parse,
};

fn array_is_array(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(heap.kind(&argument(heap, args, 0)?)? == ValueKind::Array)
}
static ARRAY_IS_ARRAY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Array.isArray admitted Array carriers",
    name: "isArray",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: array_is_array,
};

static ARRAY_INDEX_OF: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Array.prototype.indexOf consumed by fixed OMP596f2da openai-shared.ts:3027",
    name: "indexOf",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, receiver, arguments, runtime| {
        native(heap, prototypes, runtime, || {
            heap.array_index_of_native(receiver, arguments, runtime)
        })
    },
};

fn whitespace(unit: u16) -> bool {
    matches!(unit,0x09..=0x0d|0x20|0xa0|0x1680|0x2000..=0x200a|0x2028..=0x2029|0x202f|0x205f|0x3000|0xfeff)
}
fn trim_start(units: &[u16]) -> &[u16] {
    let start = units
        .iter()
        .position(|&u| !whitespace(u))
        .unwrap_or(units.len());
    &units[start..]
}
fn digit(unit: u16) -> Option<u32> {
    match unit {
        0x30..=0x39 => Some(u32::from(unit - 0x30)),
        0x41..=0x5a => Some(u32::from(unit - 0x41) + 10),
        0x61..=0x7a => Some(u32::from(unit - 0x61) + 10),
        _ => None,
    }
}
fn to_int32(number: f64) -> i32 {
    if !number.is_finite() || number == 0.0 {
        0
    } else {
        (number.trunc().rem_euclid(4_294_967_296.0) as u32) as i32
    }
}
fn exact_integer_number(digits: &[u16], radix: u32, negative: bool) -> f64 {
    let significant = digits
        .iter()
        .position(|&u| digit(u) != Some(0))
        .unwrap_or(digits.len());
    let digits = &digits[significant..];
    if digits.is_empty() {
        return if negative { -0.0 } else { 0.0 };
    }
    // More than 1024 significant radix>=2 digits proves >=2^1024. Keep the
    // original full prefix scan, but don't grow irrelevant huge decimal state.
    if digits.len() > 1024 {
        return if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let mut decimal = vec![0_u32];
    for &unit in digits {
        let mut carry = digit(unit).expect("prefix digit already admitted");
        for d in &mut decimal {
            let sum = *d * radix + carry;
            *d = sum % 10;
            carry = sum / 10;
        }
        while carry > 0 {
            decimal.push(carry % 10);
            carry /= 10;
        }
    }
    let mut text = String::with_capacity(decimal.len() + 1);
    if negative {
        text.push('-');
    }
    for d in decimal.into_iter().rev() {
        text.push(char::from(b'0' + d as u8));
    }
    text.parse::<f64>().unwrap_or(if negative {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    })
}
fn parse_int(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, captures, runtime, || {
        let text = implicit_string(heap, &argument(heap, args, 0)?, runtime)?;
        let mut units = trim_start(text.units());
        let negative = units.first() == Some(&0x2d);
        if matches!(units.first(), Some(0x2b | 0x2d)) {
            units = &units[1..];
        }
        // String ToPrimitive precedes original radix ToNumber, even if the
        // eventual prefix has no digits.
        let mut radix = to_int32(heap.to_number(&argument(heap, args, 1)?, runtime)?);
        if radix != 0 && !(2..=36).contains(&radix) {
            return heap.number(f64::NAN);
        }
        let strip = radix == 0 || radix == 16;
        if radix == 0 {
            radix = 10;
        }
        if strip && units.len() >= 2 && units[0] == 0x30 && matches!(units[1], 0x58 | 0x78) {
            units = &units[2..];
            radix = 16;
        }
        let end = units
            .iter()
            .position(|&u| digit(u).is_none_or(|d| d >= radix as u32))
            .unwrap_or(units.len());
        heap.number(if end == 0 {
            f64::NAN
        } else {
            exact_integer_number(&units[..end], radix as u32, negative)
        })
    })
}
static PARSE_INT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA parseInt UTF16 prefix/radix/rounding",
    name: "parseInt",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: parse_int,
};

fn decimal_prefix(units: &[u16]) -> Option<usize> {
    let mut p = usize::from(matches!(units.first(), Some(0x2b | 0x2d)));
    if units
        .get(p..p + 8)
        .is_some_and(|s| s.iter().copied().eq("Infinity".encode_utf16()))
    {
        return Some(p + 8);
    }
    let mut digits = 0;
    while units.get(p).is_some_and(|&u| matches!(u, 0x30..=0x39)) {
        digits += 1;
        p += 1;
    }
    if units.get(p) == Some(&0x2e) {
        p += 1;
        while units.get(p).is_some_and(|&u| matches!(u, 0x30..=0x39)) {
            digits += 1;
            p += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    let exponent_start = p;
    if units.get(p).is_some_and(|&u| matches!(u, 0x45 | 0x65)) {
        p += 1;
        if units.get(p).is_some_and(|&u| matches!(u, 0x2b | 0x2d)) {
            p += 1;
        }
        let first = p;
        while units.get(p).is_some_and(|&u| matches!(u, 0x30..=0x39)) {
            p += 1;
        }
        if p == first {
            p = exponent_start;
        }
    }
    Some(p)
}
fn parse_float(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, captures, runtime, || {
        let text = implicit_string(heap, &argument(heap, args, 0)?, runtime)?;
        let units = trim_start(text.units());
        let Some(end) = decimal_prefix(units) else {
            return heap.number(f64::NAN);
        };
        // Only the admitted ASCII numeric prefix is encoded. Original UTF16
        // input, trailing surrogates and discarded suffixes are not decoded.
        let prefix = units[..end]
            .iter()
            .map(|&u| char::from(u as u8))
            .collect::<String>();
        let number = match prefix.as_str() {
            "Infinity" | "+Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => prefix.parse::<f64>().unwrap_or(f64::NAN),
        };
        heap.number(number)
    })
}
static PARSE_FLOAT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA parseFloat UTF16 longest decimal prefix",
    name: "parseFloat",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: parse_float,
};

/// Number's unsigned nondecimal String grammar requires the whole trimmed
/// input, unlike parseInt's longest digit prefix. Other String forms continue
/// through the original neutral conversion grammar unchanged.
fn unsigned_nondecimal_number(text: &WireString) -> Option<f64> {
    let mut units = trim_start(text.units());
    while units.last().is_some_and(|&unit| whitespace(unit)) {
        units = &units[..units.len() - 1];
    }
    if units.first() != Some(&0x30) {
        return None;
    }
    let radix = match units.get(1) {
        Some(0x58 | 0x78) => 16,
        Some(0x42 | 0x62) => 2,
        Some(0x4f | 0x6f) => 8,
        _ => return None,
    };
    let digits = &units[2..];
    if digits.is_empty()
        || digits
            .iter()
            .any(|&unit| digit(unit).is_none_or(|d| d >= radix))
    {
        return Some(f64::NAN);
    }
    Some(exact_integer_number(digits, radix, false))
}

fn number_call(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, captures, runtime, || {
        if args.is_empty() {
            return heap.number(0.0);
        }
        let primitive = heap.to_primitive(&args[0], false, runtime)?;
        let n = match heap.snapshot(&primitive)? {
            ValueSnapshot::BigInt(n) => {
                n.as_str()
                    .parse::<f64>()
                    .unwrap_or(if n.as_str().starts_with('-') {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    })
            }
            ValueSnapshot::String(text) => match unsigned_nondecimal_number(&text) {
                Some(number) => number,
                None => heap.to_number(&primitive, runtime)?,
            },
            _ => heap.to_number(&primitive, runtime)?,
        };
        heap.number(n)
    })
}
static NUMBER_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Number call with explicit BigInt conversion",
    name: "Number",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: number_call,
};
fn number_check(
    heap: &Arc<ValueHeap>,
    args: &[ValueHandle],
    check: fn(f64) -> bool,
) -> Result<ValueHandle> {
    match heap.snapshot(&argument(heap, args, 0)?)? {
        ValueSnapshot::Number(n) => heap.boolean(check(n)),
        _ => heap.boolean(false),
    }
}
macro_rules! number_predicate {
    ($body:ident,$spec:ident,$name:literal,$check:expr) => {
        fn $body(
            heap: &Arc<ValueHeap>,
            _: &[ValueHandle],
            _: &ValueHandle,
            args: &[ValueHandle],
            _: &dyn ValueRuntime,
        ) -> Result<ValueHandle> {
            number_check(heap, args, $check)
        }
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("Native Number.", $name),
            name: $name,
            length: 1,
            form: CompiledFunctionForm::Builtin,
            capture_count: 4,
            body: $body,
        };
    };
}
number_predicate!(number_finite, NUMBER_FINITE, "isFinite", |n| n.is_finite());
number_predicate!(number_nan, NUMBER_NAN, "isNaN", |n| n.is_nan());
number_predicate!(number_integer, NUMBER_INTEGER, "isInteger", |n| n
    .is_finite()
    && n.fract() == 0.0);
number_predicate!(
    number_safe_integer,
    NUMBER_SAFE_INTEGER,
    "isSafeInteger",
    |n| n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0
);

fn date_parse(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, &captures[..4], runtime, || {
        let input = argument(heap, args, 0)?;
        let source = implicit_string(heap, &input, runtime)?;
        heap.call(
            &captures[4],
            &heap.undefined()?,
            &[heap.string("parse".into())?, heap.string(source)?],
            runtime,
        )
    })
}
static DATE_PARSE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Date.parse original ToString and Host calendar/timezone",
    name: "parse",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 5,
    body: date_parse,
};
