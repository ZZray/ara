//! Narrow graph-facing Host import adapters. Preparation causes no Host effect.
use super::*;

fn define(c: &C<'_>, object: &ValueHandle, key: &str, value: ValueHandle) -> Result<()> {
    c.heap.define_own(
        object,
        key.into(),
        PropertyDescriptor::Data { value, writable: true, enumerable: false, configurable: true },
        c.runtime,
    )
}
fn getter(
    c: &C<'_>,
    object: &ValueHandle,
    key: &str,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
) -> Result<()> {
    let function = c.heap.fresh_compiled_function(spec, captures)?;
    let ValueSnapshot::Function(target) = c.heap.snapshot(&function)? else {
        return Err(ValueError::WrongRuntimeReply("Native logger getter target"));
    };
    c.heap.define_own(
        object,
        key.into(),
        PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
        c.runtime,
    )
}
pub(super) fn install(c: &C<'_>) -> Result<()> {
    let process = c.heap.object()?;
    let env = c.heap.object()?;
    for key in ["PI_TIMING", "PI_DEBUG_STARTUP", "HOME"] {
        let mut captures = c.captures.to_vec();
        captures.push(c.text(key)?);
        getter(c, &env, key, &ENVIRONMENT, &captures)?;
    }
    define(c, &process, "env", env)?;
    define(c, &process, "cwd", c.heap.fresh_compiled_function(&CWD, c.captures)?)?;
    getter(c, &process, "pid", &PID, c.captures)?;
    let performance = c.heap.object()?;
    define(c, &performance, "now", c.heap.fresh_compiled_function(&PERFORMANCE, c.captures)?)?;
    let console = c.heap.object()?;
    define(c, &console, "error", c.heap.fresh_compiled_function(&CONSOLE_ERROR, c.captures)?)?;
    let fs = c.heap.object()?;
    define(c, &fs, "writeSync", c.heap.fresh_compiled_function(&WRITE_SYNC, c.captures)?)?;
    let os = c.heap.object()?;
    getter(c, &os, "EOL", &EOL, c.captures)?;
    define(c, &c.captures[1], "process", process)?;
    define(c, &c.captures[1], "performance", performance)?;
    define(c, &c.captures[1], "console", console)?;
    c.put("fs", fs)?;
    c.put("os", os)?;
    Ok(())
}
macro_rules! host_function {
    ($spec:ident, $name:literal, $len:expr, $op:literal) => {
        pub(super) static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("Native logger original Host import ", $name),
            name: $name,
            length: $len,
            form: CompiledFunctionForm::Builtin,
            capture_count: BASE_CAPTURES,
            body: |heap, captures, receiver, args, runtime| {
                let c = C { heap, captures, runtime };
                c.port(receiver, $op, args).map_err(|error| c.original_error(error))
            },
        };
    };
}
host_function!(CWD, "cwd", 0, "cwd");
host_function!(PID, "get pid", 0, "pid");
host_function!(EOL, "get EOL", 0, "eol");
host_function!(PERFORMANCE, "now", 0, "performance");
host_function!(FILE_CLOSE, "close", 0, "closeFile");
host_function!(FILE_WRITE, "write", 1, "writeFile");
host_function!(WRITE_SYNC, "writeSync", 2, "writeSync");
host_function!(CONSOLE_ERROR, "error", 0, "consoleError");
static ENVIRONMENT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native logger live process.env getter",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: BASE_CAPTURES + 1,
    body: |heap, captures, receiver, _, runtime| {
        let c = C { heap, captures, runtime };
        c.port(receiver, "environment", &[captures[BASE_CAPTURES].clone()]).map_err(|error| c.original_error(error))
    },
};
