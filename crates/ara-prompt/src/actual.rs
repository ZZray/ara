//! Fixed OMP596f2da Native prompt consumer for the trusted tool-loop template.
//! MIT; Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! See THIRD_PARTY_NOTICES.md. Context values never pass through serde/UTF8.
//! General registered helpers/partials/blocks and classConstruct remain open.

#[path = "actual_format.rs"]
mod formatter;
#[path = "actual_operators.rs"]
mod operators;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) type Result<T> = std::result::Result<T, ValueError>;
#[derive(Clone, Copy)]
pub(crate) struct Exec<'a> {
    pub heap: &'a Arc<ValueHeap>,
    pub runtime: &'a dyn ValueRuntime,
}
impl Exec<'_> {
    pub fn get(self, root: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(root, &key.into(), self.runtime)
    }
    pub fn own(self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<()> {
        self.heap.define_own_data(root, key.into(), value, self.runtime)
    }
    pub fn object(self, fields: impl IntoIterator<Item = (&'static str, ValueHandle)>) -> Result<ValueHandle> {
        let root = self.heap.object()?;
        for (key, member) in fields {
            self.own(&root, key, member)?;
        }
        Ok(root)
    }
    pub fn text(self, value: impl Into<WireString>) -> Result<ValueHandle> {
        self.heap.string(value.into())
    }
    pub fn number(self, value: usize) -> Result<ValueHandle> {
        self.heap.number(value as f64)
    }
    pub fn method(self, root: &ValueHandle, key: &str, arguments: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(root, key)?;
        self.heap.call(&method, root, arguments, self.runtime)
    }
    pub fn apply(
        self,
        root: &ValueHandle,
        key: &str,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
    ) -> Result<ValueHandle> {
        let method = self.get(root, key)?;
        let callback = self.heap.fresh_compiled_function(spec, captures)?;
        self.heap.call(&method, root, &[callback], self.runtime)
    }
    pub fn nullish(self, root: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(root)?, ValueKind::Null | ValueKind::Undefined))
    }
    pub fn eq(self, root: &ValueHandle, text: &str) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(root)?, ValueSnapshot::String(value) if value.equals_ascii(text)))
    }
    pub fn num_eq(self, root: &ValueHandle, number: f64) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(root)?, ValueSnapshot::Number(value) if value == number))
    }
    pub fn strict_equal(self, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
        match (self.heap.snapshot(left)?, self.heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
            _ => self.heap.object_is(left, right),
        }
    }
    pub fn implicit(self, root: &ValueHandle) -> Result<WireString> {
        if self.heap.kind(root)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        self.heap.to_string(root, self.runtime)
    }
    pub fn interpolation(self, values: &[ValueHandle]) -> Result<ValueHandle> {
        let mut units = Vec::new();
        for value in values {
            units.extend_from_slice(self.implicit(value)?.units());
        }
        self.heap.string(WireString::from_units(units))
    }
    pub fn for_of(self, root: &ValueHandle, mut body: impl FnMut(ValueHandle) -> Result<()>) -> Result<()> {
        let mut iterator = self.heap.get_iterator(root, self.runtime)?;
        while let Some(value) = self.heap.iterator_next(&mut iterator, self.runtime)? {
            if let Err(primary) = body(value) {
                return Err(match self.heap.iterator_close(&mut iterator, self.runtime) {
                    Ok(()) => primary,
                    Err(close) if primary.is_native_exception() && close.is_native_exception() => primary,
                    Err(close) => ValueError::IteratorClose { original: Box::new(primary), close: Box::new(close) },
                });
            }
        }
        Ok(())
    }
}

/// Host installs one module owner per persistent realm. These fields are only
/// heap edges; compiled captures contain no Host/factory/runtime leases.
#[derive(Clone)]
pub struct NativePromptModule {
    prompt: ValueHandle,
    date: ValueHandle,
    safe_string_class: ValueHandle,
    private_values: ValueHandle,
}
impl NativePromptModule {
    pub fn prompt(&self) -> &ValueHandle {
        &self.prompt
    }
    pub fn date(&self) -> &ValueHandle {
        &self.date
    }
    pub fn safe_string_constructor(&self) -> &ValueHandle {
        &self.safe_string_class
    }

    /// Selected executable construction/brand path. General JS new and class
    /// subclass construction are not claimed by this scoped module.
    pub fn safe_string(
        &self,
        heap: &Arc<ValueHeap>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let text = heap.string(heap.to_string(value, runtime)?)?;
        let prototype = heap.get(&self.safe_string_class, &"prototype".into(), runtime)?;
        let object = heap.object()?;
        heap.set_prototype(&object, Some(&prototype), runtime)?;
        heap.compiled_private_insert(&self.private_values, &object, &text)?;
        Ok(object)
    }
}

pub fn prepare_module(heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<NativePromptModule> {
    let date = heap.fresh_compiled_function(&DATE_BINDING, &[])?;
    heap.define_own(
        &date,
        "now".into(),
        PropertyDescriptor::Data {
            value: heap.fresh_compiled_function(&DATE_NOW, &[])?,
            writable: true,
            enumerable: false,
            configurable: true,
        },
        runtime,
    )?;
    prepare_module_with_date(heap, runtime, date)
}

/// Prepare the prompt module using its persistent Session's original Date.
/// Date.now is not read or cached here: consumers keep their dynamic Get/Call.
pub fn prepare_module_with_date(
    heap: &Arc<ValueHeap>,
    runtime: &dyn ValueRuntime,
    date: ValueHandle,
) -> Result<NativePromptModule> {
    heap.adopt(&date)?;
    if heap.kind(&date)? != ValueKind::Function {
        return Err(ValueError::TypeError("Original prompt Date binding must be callable".into()));
    }
    let ex = Exec { heap, runtime };
    let private_values = heap.new_map(&heap.undefined()?, runtime)?;
    let safe_string_class = heap.fresh_compiled_function(&SAFE_STRING_CLASS, &[])?;
    let prototype = ex.get(&safe_string_class, "prototype")?;
    // Class .prototype is non-writable; ordinary compiled functions default
    // writable, so specialize the class descriptor before publication.
    heap.define_own(
        &safe_string_class,
        "prototype".into(),
        PropertyDescriptor::Data { value: prototype.clone(), writable: false, enumerable: false, configurable: false },
        runtime,
    )?;
    for (key, spec) in [("toString", &SAFE_STRING_TEXT), ("toHTML", &SAFE_STRING_HTML)] {
        let method = heap.fresh_compiled_function(spec, &[private_values.clone()])?;
        heap.define_own(
            &prototype,
            key.into(),
            PropertyDescriptor::Data { value: method, writable: true, enumerable: false, configurable: true },
            runtime,
        )?;
    }
    let ascii_callback = formatter::prepare_module(ex)?;
    let cache = heap.new_map(&heap.undefined()?, runtime)?;
    let render = heap.fresh_compiled_function(&RENDER, &[cache, safe_string_class.clone(), ascii_callback.clone()])?;
    let format = heap.fresh_compiled_function(&FORMAT, &[ascii_callback])?;
    let prompt = ex.object([("render", render), ("format", format)])?;
    Ok(NativePromptModule { prompt, date, safe_string_class, private_values })
}

static SAFE_STRING_CLASS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "class SafeString",
    name: "SafeString",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError("Class constructor SafeString cannot be invoked without new".into()))
    },
};
fn safe_text(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    _: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.compiled_private_get(&captures[0], receiver)?.ok_or_else(|| {
        ValueError::TypeError("Cannot read private member #value from an object whose class did not declare it".into())
    })
}
static SAFE_STRING_TEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "toString() { return this.#value; }",
    name: "toString",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: safe_text,
};
static SAFE_STRING_HTML: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "toHTML() { return this.#value; }",
    name: "toHTML",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: safe_text,
};
static DATE_NOW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Date.now() [Host wall-clock binding]",
    name: "now",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, _, _, _| {
        let time = SystemTime::now().duration_since(UNIX_EPOCH);
        let milliseconds = match time {
            Ok(time) => time.as_millis() as f64,
            Err(time) => -(time.duration().as_millis() as f64),
        };
        heap.number(milliseconds)
    },
};
static DATE_BINDING: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Date [Host clock namespace; selected consumer is Date.now]",
    name: "Date",
    length: 7,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |_, _, _, _, _| {
        Err(ValueError::UnsupportedOperation("General Date call/classConstruct is outside fixed notice clock binding"))
    },
};
static RENDER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "render(template, context = {})",
    name: "render",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        let ex = Exec { heap, runtime };
        let source = args.first().cloned().unwrap_or(heap.undefined()?);
        let context = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let get = ex.get(&captures[0], "get")?;
        let cached = heap.call(&get, &captures[0], &[source.clone()], runtime)?;
        let compiled = if heap.truthy(&cached)? {
            cached
        } else {
            let nodes = crate::template::actual::compile_nodes(ex, &source)?;
            let delegate =
                heap.fresh_compiled_function(&crate::template::actual::DELEGATE, &[nodes, captures[1].clone()])?;
            let set = ex.get(&captures[0], "set")?;
            heap.call(&set, &captures[0], &[source, delegate.clone()], runtime)?;
            delegate
        };
        let context = if ex.nullish(&context)? { heap.object()? } else { context };
        let rendered = heap.call(&compiled, &heap.undefined()?, &[context], runtime)?;
        let options = ex.object([("renderPhase", ex.text("post-render")?)])?;
        formatter::format(ex, &rendered, &options, &captures[2])
    },
};
static FORMAT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "format(content, options = {})",
    name: "format",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        let content = args.first().cloned().unwrap_or(heap.undefined()?);
        let options = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let options = if heap.kind(&options)? == ValueKind::Undefined { heap.object()? } else { options };
        formatter::format(Exec { heap, runtime }, &content, &options, &captures[0])
    },
};
