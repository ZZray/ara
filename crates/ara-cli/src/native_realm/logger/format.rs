//! Original fixed logger logging/transport/startup bodies; effects use Host ports.
use super::*;

pub(super) const EXPORTS: &[(&str, &CompiledFunctionSpec)] = &[
    ("registerLogSink", &REGISTER),
    ("setTransports", &SET_TRANSPORTS),
    ("error", &ERROR),
    ("warn", &WARN),
    ("info", &INFO),
    ("debug", &DEBUG),
    ("startupMarker", &STARTUP),
];
pub(super) const INTERNAL: &[(&str, &CompiledFunctionSpec)] = &[
    ("emitToSinks", &EMIT_SINKS),
    ("jsonReplacer", &REPLACER),
    ("padTimestampPart", &PAD),
    ("formatLocalTimestamp", &TIMESTAMP),
    ("normalizeLogInfo", &NORMALIZE),
    ("formatLogInfo", &FORMAT),
    ("makeFileTransport", &MAKE_FILE),
    ("buildTransports", &BUILD),
    ("getLocalTransports", &GET_TRANSPORTS),
    ("emitLocally", &EMIT_LOCAL),
    ("startupMarker", &STARTUP),
];

original!(REGISTER, "registerLogSink", 1, register);
fn register(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let sink = c.arg(args, 0)?;
    c.method(&c.state("logSinks")?, "add", &[sink.clone()])?;
    let mut captures = c.captures.to_vec();
    captures.push(sink);
    c.heap.fresh_compiled_function(&DISPOSE, &captures)
}
static DISPOSE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da logger.ts registerLogSink return arrow",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: BASE_CAPTURES + 1,
    body: |heap, captures, _, _, runtime| {
        let c = C { heap, captures, runtime };
        let result = (|| {
            c.method(&c.state("logSinks")?, "delete", &[captures[BASE_CAPTURES].clone()])?;
            c.undef()
        })();
        result.map_err(|error| c.original_error(error))
    },
};

original!(EMIT_SINKS, "emitToSinks", 3, emit_sinks);
fn emit_sinks(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let sinks = c.state("logSinks")?;
    if matches!(c.heap.snapshot(&c.get(&sinks, "size")?)?, ValueSnapshot::Number(n) if n == 0.0) {
        return c.undef();
    }
    let date = c.env("Date")?;
    let event = c.object(&[
        ("level", c.arg(args, 0)?),
        ("message", c.arg(args, 1)?),
        ("context", c.arg(args, 2)?),
        ("timestamp", c.heap.construct_compiled_function(&date, &[], c.runtime)?),
    ])?;
    let mut iterator = c.heap.get_iterator(&sinks, c.runtime)?;
    while let Some(sink) = c.heap.iterator_next(&mut iterator, c.runtime)? {
        // Only sink Call is in this catch. Event construction/iterator failures
        // are not swallowed. The iterator is live, never a callback snapshot.
        match c.call(&sink, &[event.clone()]) {
            Ok(_) => {}
            Err(error) if error.is_native_exception() => {}
            Err(error) => return Err(error),
        }
    }
    c.undef()
}

original!(REPLACER, "jsonReplacer", 2, replacer);
fn replacer(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let value = c.arg(args, 1)?;
    if !c.heap.instance_of(&value, &c.env("Error")?, c.runtime)? {
        return Ok(value);
    }
    let out = c.object(&[
        ("name", c.get(&value, "name")?),
        ("message", c.get(&value, "message")?),
        ("stack", c.get(&value, "stack")?),
    ])?;
    for key in c.heap.for_in_string_keys(&value, c.runtime)? {
        // Bun keeps its initial key enumeration, but a deleted own key is
        // eligible only when the current first descriptor is enumerable.
        // A hidden inherited property must not be copied merely because Has
        // reports that its name still exists.
        let property = PropertyKey::String(key.clone());
        let mut current = Some(value.clone());
        let mut visited = std::collections::HashSet::new();
        let mut enumerable = false;
        while let Some(owner) = current {
            if !visited.insert(owner.node_id()) {
                return Err(ValueError::TypeError("Cyclic prototype enumeration".into()));
            }
            if let Some(descriptor) = c.heap.own_descriptor(&owner, &property, c.runtime)? {
                enumerable = descriptor.enumerable();
                break;
            }
            current = c.heap.prototype(&owner, c.runtime)?;
        }
        if enumerable {
            let item = c.heap.get(&value, &property, c.runtime)?;
            c.heap.assign(&out, property, item, c.runtime)?;
        }
    }
    if !c.undefined(&c.get(&value, "cause")?)? {
        c.set(&out, "cause", c.get(&value, "cause")?)?;
    }
    Ok(out)
}

original!(PAD, "padTimestampPart", 1, pad);
fn pad(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let width = c.arg(args, 1)?;
    let width = if c.undefined(&width)? { c.number(2.0)? } else { width };
    let string = c.call(&c.env("String")?, &[c.arg(args, 0)?])?;
    c.method(&string, "padStart", &[width, c.text("0")?])
}
fn date_pad(c: &C<'_>, date: &ValueHandle, getter: &str, width: f64, plus: bool) -> Result<WireString> {
    let value = c.method(date, getter, &[])?;
    let value = if plus { c.plus(&value, &c.number(1.0)?)? } else { value };
    let padded = c.helper("padTimestampPart", &[value, c.number(width)?])?;
    c.string(&padded)
}
original!(TIMESTAMP, "formatLocalTimestamp", 1, timestamp);
fn timestamp(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let date = c.arg(args, 0)?;
    let offset = -c.num(&c.method(&date, "getTimezoneOffset", &[])?)?;
    let math = c.env("Math")?;
    let absolute = c.method(&math, "abs", &[c.number(offset)?])?;
    let sign = if offset >= 0.0 { "+" } else { "-" };
    let mut pieces = vec![
        date_pad(c, &date, "getFullYear", 4.0, false)?,
        "-".into(),
        date_pad(c, &date, "getMonth", 2.0, true)?,
        "-".into(),
        date_pad(c, &date, "getDate", 2.0, false)?,
        "T".into(),
        date_pad(c, &date, "getHours", 2.0, false)?,
        ":".into(),
        date_pad(c, &date, "getMinutes", 2.0, false)?,
        ":".into(),
        date_pad(c, &date, "getSeconds", 2.0, false)?,
        ".".into(),
        date_pad(c, &date, "getMilliseconds", 3.0, false)?,
        sign.into(),
    ];
    let current_math = c.env("Math")?;
    let floor = c.get(&current_math, "floor")?;
    let floor_argument = c.number(c.num(&absolute)? / 60.0)?;
    let hours = c.heap.call(&floor, &current_math, &[floor_argument], c.runtime)?;
    pieces.push(c.string(&c.helper("padTimestampPart", &[hours])?)?);
    pieces.push(":".into());
    pieces.push(c.string(&c.helper("padTimestampPart", &[c.number(c.num(&absolute)? % 60.0)?])?)?);
    c.heap.string(c.join_text(&pieces))
}

original!(NORMALIZE, "normalizeLogInfo", 3, normalize);
fn normalize(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let level = c.arg(args, 0)?;
    let message = c.arg(args, 1)?;
    let context = c.arg(args, 2)?;
    let token = c.method(&c.state("formatTokenPattern")?, "test", &[message.clone()])?;
    let metadata = if !c.truth(&token)? && matches!(c.kind(&context)?, ValueKind::Object | ValueKind::Array) {
        context
    } else {
        c.undef()?
    };
    let object = c.env("Object")?;
    let assign = c.get(&object, "assign")?;
    let info = c.heap.call(
        &assign,
        &object,
        &[c.heap.object()?, metadata.clone(), c.object(&[("level", level), ("message", message.clone())])?],
        c.runtime,
    )?;
    if !c.nullish(&metadata)? && c.truth(&c.get(&metadata, "message")?)? {
        let original_message = c.string(&message)?;
        let metadata_message = c.string(&c.get(&metadata, "message")?)?;
        c.set(&info, "message", c.heap.string(c.join_text(&[original_message, " ".into(), metadata_message]))?)?;
    }
    if !c.nullish(&metadata)? && c.truth(&c.get(&metadata, "stack")?)? {
        c.set(&info, "stack", c.get(&metadata, "stack")?)?;
    }
    if !c.nullish(&metadata)? && c.truth(&c.get(&metadata, "cause")?)? {
        c.set(&info, "cause", c.get(&metadata, "cause")?)?;
    }
    Ok(info)
}

original!(FORMAT, "formatLogInfo", 1, format);
fn format(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let info = c.arg(args, 0)?;
    let date = c.heap.construct_compiled_function(&c.env("Date")?, &[], c.runtime)?;
    let timestamp = c.helper("formatLocalTimestamp", &[date])?;
    c.set(&info, "timestamp", timestamp.clone())?;
    let entry = c.object(&[
        ("timestamp", timestamp),
        ("level", c.get(&info, "level")?),
        ("pid", c.get(&c.env("process")?, "pid")?),
        ("message", c.get(&info, "message")?),
    ])?;
    let entries = c.method(&c.env("Object")?, "entries", &[info])?;
    let mut iterator = c.heap.get_iterator(&entries, c.runtime)?;
    while let Some(pair) = c.heap.iterator_next(&mut iterator, c.runtime)? {
        let mut pair_iterator = c.heap.get_iterator(&pair, c.runtime)?;
        let key = c.heap.iterator_next(&mut pair_iterator, c.runtime)?.unwrap_or(c.undef()?);
        let value = c.heap.iterator_next(&mut pair_iterator, c.runtime)?.unwrap_or(c.undef()?);
        c.heap.iterator_close(&mut pair_iterator, c.runtime)?;
        let reserved = match c.heap.snapshot(&key)? {
            ValueSnapshot::String(text) => {
                ["level", "timestamp", "message"].iter().any(|s| text == WireString::from(*s))
            }
            _ => false,
        };
        if !reserved {
            let property = match c.heap.snapshot(&key)? {
                ValueSnapshot::Symbol { .. } => PropertyKey::Symbol(c.heap.symbol_key(&key)?),
                _ => PropertyKey::String(c.string(&key)?),
            };
            c.heap.assign(&entry, property, value, c.runtime)?;
        }
    }
    c.method(&c.env("JSON")?, "stringify", &[entry, c.state("jsonReplacer")?])
}

original!(MAKE_FILE, "makeFileTransport", 1, make_file);
fn make_file(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let supplied = c.arg(args, 0)?;
    let directory = if c.nullish(&supplied)? { c.port(&c.undef()?, "defaultDir", &[])? } else { supplied };
    // No ToString is inserted after the third opts.file Get. Ordinary invalid
    // directories throw through the original TypeError carrier. Opaque path
    // adapters (including future Buffer/URL representations) remain explicit.
    if c.kind(&directory)? != ValueKind::String {
        if matches!(c.heap.snapshot(&directory)?, ValueSnapshot::Opaque { .. }) {
            return Err(ValueError::UnsupportedOperation("Original fs opaque directory domain is pending"));
        }
        return Err(ValueError::TypeError("The path argument must be a string".into()));
    }
    let file = c.heap.object()?;
    for (name, spec) in [("write", &ports::FILE_WRITE), ("close", &ports::FILE_CLOSE)] {
        c.heap.define_own(
            &file,
            name.into(),
            PropertyDescriptor::Data {
                value: c.heap.fresh_compiled_function(spec, c.captures)?,
                writable: true,
                enumerable: false,
                configurable: true,
            },
            c.runtime,
        )?;
    }
    c.port(&c.undef()?, "makeFile", &[directory, file])
}
original!(BUILD, "buildTransports", 1, build);
fn build(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let opts = c.arg(args, 0)?;
    let file = if c.truth(&c.get(&opts, "file")?)? {
        let test = c.get(&opts, "file")?;
        let directory = if c.kind(&test)? == ValueKind::String { c.get(&opts, "file")? } else { c.undef()? };
        c.helper("makeFileTransport", &[directory])?
    } else {
        c.undef()?
    };
    let console = matches!(c.heap.snapshot(&c.get(&opts, "console")?)?, ValueSnapshot::Boolean(true));
    c.object(&[("file", file), ("console", c.heap.boolean(console)?)])
}
original!(GET_TRANSPORTS, "getLocalTransports", 0, get_transports);
fn get_transports(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    let active = c.state("activeTransports")?;
    if c.nullish(&active)? {
        let built = c.helper("buildTransports", &[c.state("transportOpts")?])?;
        c.put("activeTransports", built)?;
    }
    c.state("activeTransports")
}
original!(EMIT_LOCAL, "emitLocally", 3, emit_local);
fn emit_local(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let transports = c.helper("getLocalTransports", &[])?;
    let info = c.helper("normalizeLogInfo", args)?;
    if !c.truth(&c.get(&transports, "file")?)? && !c.truth(&c.get(&transports, "console")?)? {
        return c.undef();
    }
    let line = c.helper("formatLogInfo", &[info.clone()])?;
    if c.truth(&c.get(&transports, "file")?)? {
        c.method(&c.get(&transports, "file")?, "write", &[line])?;
    }
    if c.truth(&c.get(&transports, "console")?)? {
        let fs = c.state("fs")?;
        let write = c.get(&fs, "writeSync")?;
        let second = c.string(&c.helper("formatLogInfo", &[info])?)?;
        let eol = c.string(&c.get(&c.state("os")?, "EOL")?)?;
        c.heap.call(&write, &fs, &[c.number(1.0)?, c.heap.string(c.join_text(&[second, eol]))?], c.runtime)?;
    }
    c.undef()
}
original!(SET_TRANSPORTS, "setTransports", 1, set_transports);
fn set_transports(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let opts = c.arg(args, 0)?;
    c.put("transportOpts", opts.clone())?;
    let previous = c.state("activeTransports")?;
    if !c.truth(&previous)? {
        return c.undef();
    }
    c.put("activeTransports", c.object(&[("file", c.undef()?), ("console", c.heap.boolean(false)?)])?)?;
    let file = c.get(&previous, "file")?;
    if !c.nullish(&file)? {
        c.method(&file, "close", &[])?;
    }
    let built = c.helper("buildTransports", &[opts])?;
    c.put("activeTransports", built)?;
    c.undef()
}
fn log(c: &C<'_>, args: &[ValueHandle], level: &str) -> Result<ValueHandle> {
    let args = [c.text(level)?, c.arg(args, 0)?, c.arg(args, 1)?];
    match c.helper("emitLocally", &args) {
        Ok(_) => {}
        Err(error) if error.is_native_exception() => {}
        Err(error) => return Err(error),
    }
    c.helper("emitToSinks", &args)?;
    c.undef()
}
original!(ERROR, "error", 2, error);
original!(WARN, "warn", 2, warn);
original!(INFO, "info", 2, info);
original!(DEBUG, "debug", 2, debug);
fn error(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    log(c, args, "error")
}
fn warn(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    log(c, args, "warn")
}
fn info(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    log(c, args, "info")
}
fn debug(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    log(c, args, "debug")
}

original!(STARTUP, "startupMarker", 1, startup);
fn startup(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    if !c.truth(&c.env_value("PI_DEBUG_STARTUP")?)? {
        return c.undef();
    }
    let result = (|| {
        let fs = c.state("fs")?;
        let write = c.get(&fs, "writeSync")?;
        let text = c.interpolate("[startup] ", &c.arg(args, 0)?, "\n")?;
        c.heap.call(&write, &fs, &[c.number(2.0)?, text], c.runtime)
    })();
    match result {
        Ok(_) => {}
        Err(error) if error.is_native_exception() => {}
        Err(error) => return Err(error),
    }
    c.undef()
}
