//! Bun 1.4.0 / 34cbb9a40b4bd1bd767d134a7065e66c2432a676 stripANSI.
//! Scalar Rust port of Bun-owned stripANSI.cpp and ANSIHelpers.h:244-437.
//! Bun is MIT-licensed; see the accompanying bun.LICENSE.md and source pins.
//! Only the JS UTF-16 domain is needed here. No JavaScriptCore code is linked.

use super::*;

pub(super) fn prepare(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let bun = heap.object()?;
    // BunObject.cpp registers DontDelete|Function, length 1 (not DontEnum).
    heap.define_own(
        &bun,
        "stripANSI".into(),
        PropertyDescriptor::Data {
            value: function(heap, &STRIP_ANSI, prototypes)?,
            writable: true,
            enumerable: true,
            configurable: false,
        },
        runtime,
    )?;
    // BunObject.cpp:969 registers a DontDelete lazy property. This selected
    // binding publishes the same callable shape eagerly; lazy materialization
    // and unselected hash algorithms remain outside this construction batch.
    let hash = function(heap, &HASH_CALL, prototypes)?;
    for (name, spec) in [
        ("wyhash", &WY_HASH),
        ("adler32", &ADLER32),
        ("crc32", &CRC32),
        ("cityHash32", &CITY32),
        ("cityHash64", &CITY64),
        ("xxHash32", &XX32),
        ("xxHash64", &XX_HASH64),
        ("xxHash3", &XX3),
        ("murmur32v2", &MURMUR32V2),
        ("murmur32v3", &MURMUR32V3),
        ("murmur64v2", &MURMUR64V2),
        ("rapidhash", &RAPID),
    ] {
        heap.define_own(
            &hash,
            WireString::from(name).into(),
            PropertyDescriptor::Data {
                value: function(heap, spec, prototypes)?,
                writable: true,
                enumerable: true,
                configurable: true,
            },
            runtime,
        )?;
    }
    heap.define_own(
        &bun,
        "hash".into(),
        PropertyDescriptor::Data { value: hash, writable: true, enumerable: true, configurable: false },
        runtime,
    )?;
    Ok(bun)
}

static HASH_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "bun:34cbb9a40b4bd1bd767d134a7065e66c2432a676:HashObject.rs:226-245 callable shape only",
    name: "hash",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |_, _, _, _, _| Err(ValueError::UnsupportedOperation("Bun.hash wyhash callable body is not installed")),
};

macro_rules! unsupported_hash {
    ($spec:ident, $name:literal) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "bun:34cbb9a40b4bd1bd767d134a7065e66c2432a676:HashObject.rs:226-245 named shape only",
            name: $name,
            length: 1,
            form: CompiledFunctionForm::Builtin,
            capture_count: 4,
            body: |_, _, _, _, _| {
                Err(ValueError::UnsupportedOperation("Unselected Bun hash algorithm is not installed"))
            },
        };
    };
}
unsupported_hash!(WY_HASH, "wyhash");
unsupported_hash!(ADLER32, "adler32");
unsupported_hash!(CRC32, "crc32");
unsupported_hash!(CITY32, "cityHash32");
unsupported_hash!(CITY64, "cityHash64");
unsupported_hash!(XX32, "xxHash32");
unsupported_hash!(XX3, "xxHash3");
unsupported_hash!(MURMUR32V2, "murmur32v2");
unsupported_hash!(MURMUR32V3, "murmur32v3");
unsupported_hash!(MURMUR64V2, "murmur64v2");
unsupported_hash!(RAPID, "rapidhash");

static XX_HASH64: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "bun:34cbb9a40b4bd1bd767d134a7065e66c2432a676:HashObject.rs:195-197,247-306; OMP read.ts:639",
    name: "xxHash64",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, arguments, runtime| {
        native(heap, prototypes, runtime, || {
            if arguments.len() != 1 {
                return Err(ValueError::UnsupportedOperation("Selected Bun.xxHash64 requires one String and no seed"));
            }
            let ValueSnapshot::String(text) = heap.snapshot(&arguments[0])? else {
                return Err(ValueError::UnsupportedOperation("Selected Bun.xxHash64 input must be a String"));
            };
            // UTF-8 replacement encoding matches the eight frozen selected
            // Bun observations, including lone high/low surrogates. The deeper
            // Bun converter and exhaustive malformed UTF-16 remain unverified.
            // WireString.to_utf8 is strict; NUL and valid pairs are kept here.
            let utf8 = char::decode_utf16(text.units().iter().copied())
                .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect::<String>();
            let value = xxhash_rust::xxh64::xxh64(utf8.as_bytes(), 0);
            heap.bigint_decimal(&value.to_string())
        })
    },
};

static STRIP_ANSI: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "bun:34cbb9a40b4bd1bd767d134a7065e66c2432a676:src/jsc/bindings/stripANSI.cpp:132-166",
    name: "stripANSI",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, arguments, runtime| {
        native(heap, prototypes, runtime, || {
            let input = argument(heap, arguments, 0)?;
            // JSC input.toString uses the string hint and rejects Symbols,
            // unlike the explicit String(Symbol) callable. Original throws
            // from getters/Call pass through native() with their same handle.
            let primitive = heap.to_primitive(&input, true, runtime)?;
            if heap.kind(&primitive)? == ValueKind::Symbol {
                return Err(ValueError::Thrown(new_error(
                    heap,
                    prototypes,
                    TYPE,
                    "Cannot convert a symbol to a string".into(),
                    runtime,
                )?));
            }
            let string = if heap.kind(&primitive)? == ValueKind::String {
                primitive
            } else {
                heap.string(heap.to_string(&primitive, runtime)?)?
            };
            let ValueSnapshot::String(text) = heap.snapshot(&string)? else {
                return Err(ValueError::InvalidNode);
            };
            match strip_units(text.units()) {
                Some(output) => heap.string(WireString::from_units(output)),
                None => Ok(string),
            }
        })
    },
};

#[derive(Clone, Copy)]
enum State {
    Start,
    GotEsc,
    IgnoreNextChar,
    InCsi,
    InOsc,
    InOscGotEsc,
    NeedSt,
    NeedStGotEsc,
}

fn consume_ansi(input: &[u16], start: usize) -> usize {
    let mut state = State::Start;
    let mut cursor = start;
    while cursor < input.len() {
        let c = input[cursor];
        // The two escaped-string states fall through to GotEsc unless the
        // next code unit is backslash. No payload is put back on an abort.
        if matches!(state, State::InOscGotEsc | State::NeedStGotEsc) {
            if c == u16::from(b'\\') {
                state = State::Start;
                cursor += 1;
                continue;
            }
            state = State::GotEsc;
        }
        state = match state {
            State::Start => match c {
                0x1b => State::GotEsc,
                0x9b => State::InCsi,
                0x9d => State::InOsc,
                0x90 | 0x98 | 0x9e | 0x9f => State::NeedSt,
                // Standalone ST and every broad candidate that is not an
                // introducer return unchanged. Ordinary controls stay text.
                _ => return cursor,
            },
            State::GotEsc => match c {
                0x1b => State::GotEsc,
                0x18 | 0x1a | 0x9c => State::Start,
                0x5b => State::InCsi,
                0x5d => State::InOsc,
                0x50 | 0x58 | 0x5e | 0x5f => State::NeedSt,
                0x20..=0x2f => State::IgnoreNextChar,
                0x30..=0x7e => State::Start,
                _ => return cursor,
            },
            State::IgnoreNextChar => {
                if c == 0x1b {
                    State::GotEsc
                } else {
                    State::Start
                }
            }
            State::InCsi => match c {
                0x1b => State::GotEsc,
                0x18 | 0x1a | 0x9c | 0x40..=0x7e => State::Start,
                _ => State::InCsi,
            },
            State::InOsc => match c {
                0x1b => State::InOscGotEsc,
                0x07 | 0x18 | 0x1a | 0x9c => State::Start,
                _ => State::InOsc,
            },
            State::NeedSt => match c {
                0x1b => State::NeedStGotEsc,
                0x18 | 0x1a | 0x9c => State::Start,
                _ => State::NeedSt,
            },
            State::InOscGotEsc | State::NeedStGotEsc => unreachable!(),
        };
        cursor += 1;
    }
    cursor
}

fn strip_units(input: &[u16]) -> Option<Vec<u16>> {
    let mut output: Option<Vec<u16>> = None;
    let mut cursor = 0;
    let mut chunk_start = 0;
    while cursor < input.len() {
        let Some(offset) = input[cursor..].iter().position(|c| matches!(*c, 0x1b | 0x90 | 0x98 | 0x9b..=0x9f)) else {
            break;
        };
        let escape = cursor + offset;
        let after = consume_ansi(input, escape);
        if after == escape {
            cursor = escape + 1;
            continue;
        }
        let buffer = output.get_or_insert_with(|| Vec::with_capacity(input.len()));
        buffer.extend_from_slice(&input[chunk_start..escape]);
        chunk_start = after;
        cursor = after;
    }
    if let Some(buffer) = &mut output {
        buffer.extend_from_slice(&input[chunk_start..]);
    }
    output
}
