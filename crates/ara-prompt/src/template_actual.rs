//! Graph consumer of parsed fixed tool-loop, late-LSP and launch-completion nodes.
//! Launch template: fixed OMP b91db66e prompts/session/launch-completion.md.
//! Fixed OMP596f2da template.ts:326-358,374-385,518-523; MIT notices above.
use super::{Expr, Node, parse_template, strip_standalone};
use crate::actual::{Exec, Result};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueKind, ValueSnapshot,
};

pub(crate) fn compile_nodes(ex: Exec<'_>, source: &ValueHandle) -> Result<ValueHandle> {
    let ValueSnapshot::String(source) = ex.heap.snapshot(source)? else {
        return Err(ValueError::TypeError("Trusted Native prompt template must be a string".into()));
    };
    let source =
        source.to_utf8().map_err(|_| ValueError::UnsupportedOperation("Native template source UTF16 parser"))?;
    let nodes = parse_template(&strip_standalone(&source)).map_err(|error| ValueError::SyntaxError(error.0))?;
    encode_nodes(ex, nodes)
}

fn encode_nodes(ex: Exec<'_>, nodes: Vec<Node>) -> Result<ValueHandle> {
    let mut values = Vec::new();
    for node in nodes {
        let node = match node {
            Node::Text(text) => ex.object([("kind", ex.text("text")?), ("value", ex.text(text)?)])?,
            Node::Output { call, escaped }
                if call.args.is_empty()
                    && call.hash.is_empty()
                    && matches!(
                        call.name.as_str(),
                        "tool_name"
                            | "name"
                            | "state"
                            | "exitCode"
                            | "count"
                            | "arguments_summary"
                            | "result_summary"
                            | "files.length"
                            | "this.path"
                            | "this.summary"
                            | "this"
                    ) =>
            {
                ex.object([
                    ("kind", ex.text("output")?),
                    ("name", ex.text(call.name)?),
                    ("escaped", ex.heap.boolean(escaped)?),
                ])?
            }
            Node::Block { call, body, inverse } if call.hash.is_empty() && call.args.len() == 1 => {
                let Expr::Path(path) = &call.args[0] else {
                    return unsupported();
                };
                if !matches!(
                    (call.name.as_str(), path.as_str()),
                    ("if", "multiple") | ("if", "hasExitCode") | ("each", "files") | ("each", "this.messages") | ("unless", "@last")
                ) {
                    return unsupported();
                }
                ex.object([
                    ("kind", ex.text("block")?),
                    ("name", ex.text(call.name.clone())?),
                    ("path", ex.text(path.clone())?),
                    ("body", encode_nodes(ex, body)?),
                    ("inverse", encode_nodes(ex, inverse)?),
                ])?
            }
            _ => return unsupported(),
        };
        values.push(node);
    }
    ex.heap.array(values)
}

fn unsupported<T>() -> Result<T> {
    Err(ValueError::UnsupportedOperation("Native prompt body is scoped to fixed loop scalar, late LSP and launch completion nodes"))
}

fn property(ex: Exec<'_>, context: &ValueHandle, key: &str) -> Result<ValueHandle> {
    if ex.nullish(context)? {
        return ex.heap.undefined();
    }
    if key == "length" && matches!(ex.heap.kind(context)?, ValueKind::String | ValueKind::Array) {
        return ex.get(context, key);
    }
    if !matches!(ex.heap.kind(context)?, ValueKind::Object | ValueKind::Array | ValueKind::Function)
        || matches!(key, "__proto__" | "prototype" | "constructor")
    {
        return ex.heap.undefined();
    }
    if ex.heap.own_descriptor(context, &key.into(), ex.runtime)?.is_some() {
        ex.get(context, key)
    } else {
        ex.heap.undefined()
    }
}

fn path_text(ex: Exec<'_>, value: &ValueHandle) -> Result<String> {
    let ValueSnapshot::String(path) = ex.heap.snapshot(value)? else {
        return Err(ValueError::TypeError("Trusted Native output path is not a string".into()));
    };
    path.to_utf8().map_err(|_| ValueError::UnsupportedOperation("Native template path UTF16 parser"))
}

fn resolve_path(ex: Exec<'_>, frame: &ValueHandle, path: &str) -> Result<ValueHandle> {
    let context = ex.get(frame, "context")?;
    if path == "this" {
        return Ok(context);
    }
    if path == "@last" {
        return ex.get(&ex.get(frame, "data")?, "last");
    }
    let path = path.strip_prefix("this.").unwrap_or(path);
    let mut value = context.clone();
    for key in path.split('.') {
        value = property(ex, &value, key)?;
    }
    if ex.heap.kind(&value)? == ValueKind::Function {
        // Fixed resolvePath retains the original Function.call Get and receiver.
        ex.method(&value, "call", &[context])
    } else {
        Ok(value)
    }
}

fn conditional(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    if ex.num_eq(value, 0.0)? || !ex.heap.truthy(value)? {
        return Ok(false);
    }
    Ok(ex.heap.kind(value)? != ValueKind::Array || !ex.num_eq(&ex.get(value, "length")?, 0.0)?)
}

fn stringify(ex: Exec<'_>, value: &ValueHandle, safe: &ValueHandle) -> Result<ValueHandle> {
    if ex.nullish(value)? {
        ex.text("")
    } else if ex.heap.instance_of(value, safe, ex.runtime)? {
        ex.method(value, "toString", &[])
    } else {
        ex.heap.string(ex.heap.to_string(value, ex.runtime)?)
    }
    // Fixed prompt.ts:535 compiles with noEscape:true. Escaped AST outputs
    // therefore retain HTML characters; SafeString follows its original brand.
}

static EACH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixed template.ts each map callback: child context, index/key/first/last",
    name: "",
    length: 2,
    form: CompiledFunctionForm::Arrow,
    capture_count: 5,
    body: |heap, captures, _, args, runtime| {
        let ex = Exec { heap, runtime };
        let item = args.first().cloned().unwrap_or(heap.undefined()?);
        let index = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let (key, item) = if heap.truthy(&captures[4])? {
            // Object.entries creates native pairs; source parameter destructuring
            // still enters the pair's actual iterator and closes after two items.
            let mut iterator = heap.get_iterator(&item, runtime)?;
            let key = heap.iterator_next(&mut iterator, runtime)?.unwrap_or(heap.undefined()?);
            let value = heap.iterator_next(&mut iterator, runtime)?.unwrap_or(heap.undefined()?);
            heap.iterator_close(&mut iterator, runtime)?;
            (key, value)
        } else {
            (index.clone(), item)
        };
        let length = ex.get(&captures[3], "length")?;
        let last_index = heap.number(heap.to_number(&length, runtime)? - 1.0)?;
        let data = ex.object([
            ("index", index.clone()),
            ("key", key),
            ("first", heap.boolean(ex.num_eq(&index, 0.0)?)?),
            ("last", heap.boolean(ex.strict_equal(&index, &last_index)?)?),
        ])?;
        let child = ex.object([("context", item), ("data", data)])?;
        render_nodes(ex, &captures[0], &child, &captures[1])
    },
};

fn block(ex: Exec<'_>, node: &ValueHandle, frame: &ValueHandle, safe: &ValueHandle) -> Result<ValueHandle> {
    let name = ex.get(node, "name")?;
    let value = resolve_path(ex, frame, &path_text(ex, &ex.get(node, "path")?)?)?;
    let body = ex.get(node, "body")?;
    let inverse = ex.get(node, "inverse")?;
    if ex.eq(&name, "if")? || ex.eq(&name, "unless")? {
        let truth = conditional(ex, &value)?;
        let branch = if ex.eq(&name, "unless")? { !truth } else { truth };
        return render_nodes(ex, if branch { &body } else { &inverse }, frame, safe);
    }
    let (entries, object) = if ex.heap.kind(&value)? == ValueKind::Array {
        (value, false)
    } else if ex.heap.truthy(&value)? && ex.heap.kind(&value)? == ValueKind::Object {
        (ex.heap.object_entries(&value, ex.runtime)?, true)
    } else {
        return render_nodes(ex, &inverse, frame, safe);
    };
    if !ex.heap.truthy(&ex.get(&entries, "length")?)? {
        return render_nodes(ex, &inverse, frame, safe);
    }
    let mapped = ex.apply(
        &entries,
        "map",
        &EACH,
        &[body, safe.clone(), frame.clone(), entries.clone(), ex.heap.boolean(object)?],
    )?;
    ex.method(&mapped, "join", &[ex.text("")?])
}

fn render_nodes(ex: Exec<'_>, nodes: &ValueHandle, frame: &ValueHandle, safe: &ValueHandle) -> Result<ValueHandle> {
    let mut result = ex.text("")?;
    ex.for_of(nodes, |node| {
        let kind = ex.get(&node, "kind")?;
        let text = if ex.eq(&kind, "text")? {
            ex.get(&node, "value")?
        } else if ex.eq(&kind, "output")? {
            stringify(ex, &resolve_path(ex, frame, &path_text(ex, &ex.get(&node, "name")?)?)?, safe)?
        } else {
            block(ex, &node, frame, safe)?
        };
        // Original result += retains default ToPrimitive and the original reply.
        result = ex.heap.add(&result, &text, ex.runtime)?;
        Ok(())
    })?;
    Ok(result)
}

pub(crate) static DELEGATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "compiled trusted loop notice, fixed late LSP and launch completion templates",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, args, runtime| {
        let ex = Exec { heap, runtime };
        let context = args.first().cloned().unwrap_or(heap.undefined()?);
        let context = if ex.nullish(&context)? { heap.object()? } else { context };
        let frame = ex.object([("context", context), ("data", heap.object()?)])?;
        render_nodes(ex, &captures[0], &frame, &captures[1])
    },
};
