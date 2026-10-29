//! Original Graph calls in fixed OMP596f2da prompt.ts format/helper pipeline.
//! Default loop-notice render uses post-render + false optional flags; option
//! getters can still enable the other source branches, implemented here.
use super::{Exec, Result, operators};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueKind, ValueSnapshot,
};

const MODULE: &str = "ara-prompt.fixed-native-format";
const REGEXES: &[(&str, &str, &str)] = &[
    ("OPENING_XML", r"^<([a-z_-]+)(?:\s+[^>]*)?>$", ""),
    ("TABLE_ROW", r"^\|.*\|$", ""),
    ("TABLE_SEP", r"^\|[-:\s|]+\|$", ""),
    ("NON_BLANK", r"\S", ""),
    ("RFC2119_BOLD", r"\*\*(MUST NOT|SHOULD NOT|RECOMMENDED|REQUIRED|OPTIONAL|SHOULD|MUST|MAY|NEVER|AVOID)\*\*", "g"),
    ("RFC2119_GUARD", r"\*\*(?:MUST|SHOULD|RECOMMENDED|REQUIRED|OPTIONAL|MAY|NEVER|AVOID)|MUST NOT|SHOULD NOT", ""),
    ("MUST_NOT", r"\bMUST NOT\b", "g"),
    ("SHOULD_NOT", r"\bSHOULD NOT\b", "g"),
    ("ASCII_SYMBOLS", r"\.{3}|<->|->|<-|!=|<=|>=", "g"),
];
pub(super) fn prepare_module(ex: Exec<'_>) -> Result<ValueHandle> {
    for &(name, pattern, flags) in REGEXES {
        ex.heap.module_regexp(MODULE, name, pattern, flags)?;
    }
    let replacements = ex.object([
        ("...", ex.text("…")?),
        ("<->", ex.text("↔")?),
        ("->", ex.text("→")?),
        ("<-", ex.text("←")?),
        ("!=", ex.text("≠")?),
        ("<=", ex.text("≤")?),
        (">=", ex.text("≥")?),
    ])?;
    ex.heap.fresh_compiled_function(&ASCII_REPLACEMENT, &[replacements])
}
fn regex(ex: Exec<'_>, name: &str) -> Result<ValueHandle> {
    let &(name, pattern, flags) = REGEXES
        .iter()
        .find(|(key, _, _)| *key == name)
        .ok_or(ValueError::WrongRuntimeReply("Fixed prompt RegExp binding"))?;
    ex.heap.module_regexp(MODULE, name, pattern, flags)
}
fn test(ex: Exec<'_>, name: &str, text: &ValueHandle) -> Result<bool> {
    let value = ex.method(&regex(ex, name)?, "test", &[text.clone()])?;
    ex.heap.truthy(&value)
}
fn numeric_compare(ex: Exec<'_>, value: &ValueHandle, number: f64, operator: &str) -> Result<bool> {
    operators::compare(ex, value, &ex.heap.number(number)?, operator)
}
fn numeric_length_compare(ex: Exec<'_>, number: f64, root: &ValueHandle, operator: &str) -> Result<bool> {
    let length = ex.get(root, "length")?;
    operators::compare(ex, &ex.heap.number(number)?, &length, operator)
}
fn index(ex: Exec<'_>, root: &ValueHandle, index: usize) -> Result<ValueHandle> {
    ex.get(root, &index.to_string())
}
fn assign_index(ex: Exec<'_>, root: &ValueHandle, index: usize, value: ValueHandle) -> Result<()> {
    ex.heap.assign(root, index.to_string().as_str().into(), value, ex.runtime)
}
fn code(ex: Exec<'_>, root: &ValueHandle, offset: &ValueHandle) -> Result<ValueHandle> {
    ex.method(root, "charCodeAt", &[offset.clone()])
}
fn slice(ex: Exec<'_>, root: &ValueHandle, start: &ValueHandle, end: Option<&ValueHandle>) -> Result<ValueHandle> {
    let mut arguments = vec![start.clone()];
    if let Some(end) = end {
        arguments.push(end.clone());
    }
    ex.method(root, "slice", &arguments)
}
fn starts(ex: Exec<'_>, root: &ValueHandle, text: &str, position: Option<&ValueHandle>) -> Result<bool> {
    let mut args = vec![ex.text(text)?];
    if let Some(position) = position {
        args.push(position.clone());
    }
    ex.heap.truthy(&ex.method(root, "startsWith", &args)?)
}
fn ends(ex: Exec<'_>, root: &ValueHandle, text: &str) -> Result<bool> {
    ex.heap.truthy(&ex.method(root, "endsWith", &[ex.text(text)?])?)
}
fn indent(ex: Exec<'_>, line: &ValueHandle) -> Result<(ValueHandle, ValueHandle)> {
    let mut offset = ex.heap.number(0.0)?;
    let mut first = code(ex, line, &offset)?;
    while ex.num_eq(&first, 32.0)? || ex.num_eq(&first, 9.0)? {
        // Original Get of charCodeAt precedes ++s argument evaluation.
        let method = ex.get(line, "charCodeAt")?;
        offset = ex.heap.increment(&offset, ex.runtime)?;
        first = ex.heap.call(&method, line, &[offset.clone()], ex.runtime)?;
    }
    if numeric_compare(ex, &first, 128.0, ">=")? {
        let full_length = ex.get(line, "length")?;
        let trimmed = ex.method(line, "trimStart", &[])?;
        let trimmed_length = ex.get(&trimmed, "length")?;
        offset = operators::subtract(ex, &full_length, &trimmed_length)?;
        first = code(ex, line, &offset)?;
    }
    Ok((offset, first))
}

fn closing_tag(ex: Exec<'_>, text: &ValueHandle) -> Result<Option<ValueHandle>> {
    let n = ex.get(text, "length")?;
    if numeric_compare(ex, &n, 4.0, "<")? {
        return Ok(None);
    }
    let method = ex.get(text, "charCodeAt")?;
    let end = operators::subtract(ex, &n, &ex.heap.number(1.0)?)?;
    if !ex.num_eq(&ex.heap.call(&method, text, &[end], ex.runtime)?, 62.0)? {
        return Ok(None);
    }
    let mut j = 2.0;
    while operators::compare(ex, &ex.heap.number(j)?, &operators::subtract(ex, &n, &ex.heap.number(1.0)?)?, "<")? {
        let c = code(ex, text, &ex.heap.number(j)?)?;
        if !((numeric_compare(ex, &c, 97.0, ">=")? && numeric_compare(ex, &c, 122.0, "<=")?)
            || ex.num_eq(&c, 45.0)?
            || ex.num_eq(&c, 95.0)?)
        {
            return Ok(None);
        }
        j += 1.0;
    }
    let method = ex.get(text, "slice")?;
    let end = operators::subtract(ex, &n, &ex.heap.number(1.0)?)?;
    Ok(Some(ex.heap.call(&method, text, &[ex.heap.number(2.0)?, end], ex.runtime)?))
}
fn opening_tag(ex: Exec<'_>, text: &ValueHandle) -> Result<Option<ValueHandle>> {
    let n = ex.get(text, "length")?;
    if numeric_compare(ex, &n, 3.0, "<")? {
        return Ok(None);
    }
    let method = ex.get(text, "charCodeAt")?;
    let end = operators::subtract(ex, &n, &ex.heap.number(1.0)?)?;
    if !ex.num_eq(&ex.heap.call(&method, text, &[end], ex.runtime)?, 62.0)? {
        return Ok(None);
    }
    let mut j = 1.0;
    while operators::compare(ex, &ex.heap.number(j)?, &operators::subtract(ex, &n, &ex.heap.number(1.0)?)?, "<")? {
        let c = code(ex, text, &ex.heap.number(j)?)?;
        if (numeric_compare(ex, &c, 97.0, ">=")? && numeric_compare(ex, &c, 122.0, "<=")?)
            || ex.num_eq(&c, 45.0)?
            || ex.num_eq(&c, 95.0)?
        {
            j += 1.0;
        } else {
            break;
        }
    }
    if j == 1.0 {
        return Ok(None);
    }
    if ex.num_eq(&operators::subtract(ex, &n, &ex.heap.number(1.0)?)?, j)? {
        return Ok(Some(slice(ex, text, &ex.heap.number(1.0)?, Some(&ex.heap.number(j)?))?));
    }
    let c = code(ex, text, &ex.heap.number(j)?)?;
    if !ex.num_eq(&c, 32.0)? && !ex.num_eq(&c, 9.0)? {
        if numeric_compare(ex, &c, 128.0, "<")? {
            return Ok(None);
        }
        let matched = ex.method(&regex(ex, "OPENING_XML")?, "exec", &[text.clone()])?;
        return if ex.heap.truthy(&matched)? { Ok(Some(ex.get(&matched, "1")?)) } else { Ok(None) };
    }
    let at = ex.method(text, "indexOf", &[ex.text(">")?, ex.heap.number(j + 1.0)?])?;
    let end = operators::subtract(ex, &n, &ex.heap.number(1.0)?)?;
    if ex.strict_equal(&at, &end)? {
        Ok(Some(slice(ex, text, &ex.heap.number(1.0)?, Some(&ex.heap.number(j)?))?))
    } else {
        Ok(None)
    }
}
fn table_row(ex: Exec<'_>, line: &ValueHandle) -> Result<ValueHandle> {
    let cells = ex.method(line, "split", &[ex.text("|")?])?;
    let cells = ex.apply(&cells, "map", &TRIM_CELL, &[])?;
    ex.method(&cells, "join", &[ex.text("|")?])
}
fn table_separator(ex: Exec<'_>, line: &ValueHandle) -> Result<ValueHandle> {
    let cells = ex.method(line, "split", &[ex.text("|")?])?;
    let cells = ex.apply(&cells, "filter", &TRIM_CELL, &[])?;
    let normalized = ex.apply(&cells, "map", &NORMALIZE_CELL, &[])?;
    let joined = ex.method(&normalized, "join", &[ex.text("|")?])?;
    ex.interpolation(&[ex.text("|")?, joined, ex.text("|")?])
}
fn rfc2119(ex: Exec<'_>, text: &ValueHandle) -> Result<ValueHandle> {
    let unbolded = ex.method(text, "replace", &[regex(ex, "RFC2119_BOLD")?, ex.text("$1")?])?;
    let never = ex.method(&unbolded, "replace", &[regex(ex, "MUST_NOT")?, ex.text("NEVER")?])?;
    ex.method(&never, "replace", &[regex(ex, "SHOULD_NOT")?, ex.text("AVOID")?])
}
fn normalize_rfc(ex: Exec<'_>, line: &ValueHandle) -> Result<ValueHandle> {
    if !test(ex, "RFC2119_GUARD", line)? {
        return Ok(line.clone());
    }
    let includes = ex.method(line, "includes", &[ex.text("`")?])?;
    if !ex.heap.truthy(&includes)? {
        return rfc2119(ex, line);
    }
    let segments = ex.method(line, "split", &[ex.text("`")?])?;
    let mut i = 0;
    while numeric_length_compare(ex, i as f64, &segments, "<")? {
        let value = index(ex, &segments, i)?;
        assign_index(ex, &segments, i, rfc2119(ex, &value)?)?;
        i += 2;
    }
    ex.method(&segments, "join", &[ex.text("`")?])
}
fn replace_ascii(ex: Exec<'_>, line: &ValueHandle, callback: &ValueHandle) -> Result<ValueHandle> {
    ex.method(line, "replace", &[regex(ex, "ASCII_SYMBOLS")?, callback.clone()])
}
fn ascii_outside_comments(
    ex: Exec<'_>,
    line: &ValueHandle,
    state: &ValueHandle,
    callback: &ValueHandle,
) -> Result<ValueHandle> {
    if !ex.heap.truthy(&ex.get(state, "inHtmlComment")?)? {
        let includes = ex.method(line, "includes", &[ex.text("<!--")?])?;
        if !ex.heap.truthy(&includes)? {
            return replace_ascii(ex, line, callback);
        }
    }
    let mut result = ex.text("")?;
    let mut cursor = ex.heap.number(0.0)?;
    while operators::compare(ex, &cursor, &ex.get(line, "length")?, "<")? {
        if ex.heap.truthy(&ex.get(state, "inHtmlComment")?)? {
            let close_index = ex.method(line, "indexOf", &[ex.text("-->")?, cursor.clone()])?;
            if ex.num_eq(&close_index, -1.0)? {
                return ex.heap.add(&result, &slice(ex, line, &cursor, None)?, ex.runtime);
            }
            let method = ex.get(line, "slice")?;
            let end = ex.heap.add(&close_index, &ex.heap.number(3.0)?, ex.runtime)?;
            let part = ex.heap.call(&method, line, &[cursor.clone(), end], ex.runtime)?;
            result = ex.heap.add(&result, &part, ex.runtime)?;
            // Source evaluates closeIndex + 3 again for this assignment.
            cursor = ex.heap.add(&close_index, &ex.heap.number(3.0)?, ex.runtime)?;
            ex.heap.assign(state, "inHtmlComment".into(), ex.heap.boolean(false)?, ex.runtime)?;
            continue;
        }
        let open_index = ex.method(line, "indexOf", &[ex.text("<!--")?, cursor.clone()])?;
        if ex.num_eq(&open_index, -1.0)? {
            let part = replace_ascii(ex, &slice(ex, line, &cursor, None)?, callback)?;
            return ex.heap.add(&result, &part, ex.runtime);
        }
        let part = ex.method(line, "slice", &[cursor.clone(), open_index.clone()])?;
        result = ex.heap.add(&result, &replace_ascii(ex, &part, callback)?, ex.runtime)?;
        let method = ex.get(line, "indexOf")?;
        let close_start = ex.heap.add(&open_index, &ex.heap.number(4.0)?, ex.runtime)?;
        let close_index = ex.heap.call(&method, line, &[ex.text("-->")?, close_start], ex.runtime)?;
        if ex.num_eq(&close_index, -1.0)? {
            let part = ex.method(line, "slice", &[open_index])?;
            result = ex.heap.add(&result, &part, ex.runtime)?;
            ex.heap.assign(state, "inHtmlComment".into(), ex.heap.boolean(true)?, ex.runtime)?;
            return Ok(result);
        }
        let method = ex.get(line, "slice")?;
        let end = ex.heap.add(&close_index, &ex.heap.number(3.0)?, ex.runtime)?;
        let part = ex.heap.call(&method, line, &[open_index, end], ex.runtime)?;
        result = ex.heap.add(&result, &part, ex.runtime)?;
        cursor = ex.heap.add(&close_index, &ex.heap.number(3.0)?, ex.runtime)?;
    }
    Ok(result)
}
fn empty_length(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    ex.num_eq(&ex.get(value, "length")?, 0.0)
}
fn pop_blanks(ex: Exec<'_>, result: &ValueHandle, n: &mut usize) -> Result<()> {
    while *n > 0 && empty_length(ex, &index(ex, result, *n - 1)?)? {
        *n -= 1;
    }
    Ok(())
}
fn blank(ex: Exec<'_>, raw: &ValueHandle) -> Result<bool> {
    Ok(empty_length(ex, raw)? || !test(ex, "NON_BLANK", raw)?)
}
fn source_array(ex: Exec<'_>, length: ValueHandle) -> Result<ValueHandle> {
    if let ValueSnapshot::Number(length) = ex.heap.snapshot(&length)? {
        if !length.is_finite() || length < 0.0 || length.fract() != 0.0 || length > u32::MAX as f64 {
            return Err(ValueError::RangeError("Invalid array length".into()));
        }
        ex.heap.array_with_length(length as u32)
    } else {
        ex.heap.array(vec![length])
    }
}

pub(super) fn format(
    ex: Exec<'_>,
    content: &ValueHandle,
    options: &ValueHandle,
    ascii_callback: &ValueHandle,
) -> Result<ValueHandle> {
    // Destructure before split, preserving all inherited option getters.
    let phase = ex.get(options, "renderPhase")?;
    let phase = if ex.heap.kind(&phase)? == ValueKind::Undefined { ex.text("post-render")? } else { phase };
    let replace_ascii_symbols = ex.get(options, "replaceAsciiSymbols")?;
    let normalize = ex.get(options, "normalizeRfc2119")?;
    let pre_render = ex.eq(&phase, "pre-render")?;
    let lines = ex.method(content, "split", &[ex.text("\n")?])?;
    let result = source_array(ex, ex.get(&lines, "length")?)?;
    let mut n = 0;
    let mut in_code_block = false;
    let state = ex.object([("inHtmlComment", ex.heap.boolean(false)?)])?;
    let tags = ex.heap.array(Vec::new())?;
    let mut i = 0;
    while numeric_length_compare(ex, i as f64, &lines, "<")? {
        let raw = index(ex, &lines, i)?;
        // raw.charCodeAt callee Get precedes raw.length - 1.
        let char_code_at = ex.get(&raw, "charCodeAt")?;
        let raw_length = ex.get(&raw, "length")?;
        let offset = operators::subtract(ex, &raw_length, &ex.heap.number(1.0)?)?;
        let last = ex.heap.call(&char_code_at, &raw, &[offset], ex.runtime)?;
        let mut line = if numeric_compare(ex, &last, 32.0, "<=")? || numeric_compare(ex, &last, 128.0, ">=")? {
            ex.method(&raw, "trimEnd", &[])?
        } else {
            raw
        };
        let (mut s, mut first) = indent(ex, &line)?;
        if (ex.num_eq(&first, 96.0)? || ex.num_eq(&first, 126.0)?)
            && (starts(ex, &line, "```", Some(&s))? || starts(ex, &line, "~~~", Some(&s))?)
        {
            in_code_block = !in_code_block;
            assign_index(ex, &result, n, line)?;
            n += 1;
            i += 1;
            continue;
        }
        if in_code_block {
            assign_index(ex, &result, n, line)?;
            n += 1;
            i += 1;
            continue;
        }
        if ex.heap.truthy(&replace_ascii_symbols)? {
            let replaced = ascii_outside_comments(ex, &line, &state, ascii_callback)?;
            if !ex.strict_equal(&replaced, &line)? {
                line = replaced;
                (s, first) = indent(ex, &line)?;
            }
        }
        let mut closing_line = false;
        if ex.num_eq(&first, 60.0)? {
            let trimmed = if ex.num_eq(&s, 0.0)? { line.clone() } else { slice(ex, &line, &s, None)? };
            if ex.num_eq(&code(ex, &trimmed, &ex.heap.number(1.0)?)?, 47.0)? {
                if let Some(tag) = closing_tag(ex, &trimmed)? {
                    closing_line = true;
                    if numeric_length_compare(ex, 0.0, &tags, "<")? {
                        let length = ex.get(&tags, "length")?;
                        let index = operators::subtract(ex, &length, &ex.heap.number(1.0)?)?;
                        let key = ex.heap.to_string(&index, ex.runtime)?;
                        let last = ex.heap.get(&tags, &key.into(), ex.runtime)?;
                        if ex.strict_equal(&last, &tag)? {
                            ex.method(&tags, "pop", &[])?;
                        }
                    }
                }
            } else if ex.num_eq(&s, 0.0)? && !ends(ex, &trimmed, "/>")? {
                if let Some(tag) = opening_tag(ex, &trimmed)? {
                    ex.method(&tags, "push", &[tag])?;
                }
            }
        } else if ex.num_eq(&first, 124.0)? {
            let trimmed = if ex.num_eq(&s, 0.0)? { line.clone() } else { slice(ex, &line, &s, None)? };
            if test(ex, "TABLE_SEP", &trimmed)? {
                let prefix = slice(ex, &line, &ex.heap.number(0.0)?, Some(&s))?;
                let prefix = ex.interpolation(&[prefix])?;
                let compacted = table_separator(ex, &trimmed)?;
                line = ex.interpolation(&[prefix, compacted])?;
            } else if test(ex, "TABLE_ROW", &trimmed)? {
                let prefix = slice(ex, &line, &ex.heap.number(0.0)?, Some(&s))?;
                let prefix = ex.interpolation(&[prefix])?;
                let compacted = table_row(ex, &trimmed)?;
                line = ex.interpolation(&[prefix, compacted])?;
            }
        }
        if ex.heap.truthy(&normalize)? {
            line = normalize_rfc(ex, &line)?;
        }
        if operators::compare(ex, &s, &ex.get(&line, "length")?, ">=")? {
            let next = index(ex, &lines, i + 1)?;
            if ex.heap.kind(&next)? == ValueKind::Undefined || blank(ex, &next)? {
                pop_blanks(ex, &result, &mut n)?;
                let mut j = i + 1;
                while numeric_length_compare(ex, j as f64, &lines, "<")? {
                    let candidate = index(ex, &lines, j)?;
                    if !empty_length(ex, &candidate)? && test(ex, "NON_BLANK", &index(ex, &lines, j)?)? {
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            if n == 0 || empty_length(ex, &index(ex, &result, n - 1)?)? {
                i += 1;
                continue;
            }
        }
        if closing_line || (pre_render && ex.num_eq(&first, 123.0)? && starts(ex, &line, "{{/", Some(&s))?) {
            pop_blanks(ex, &result, &mut n)?;
        }
        assign_index(ex, &result, n, line)?;
        n += 1;
        i += 1;
    }
    pop_blanks(ex, &result, &mut n)?;
    ex.heap.assign(&result, "length".into(), ex.number(n)?, ex.runtime)?;
    ex.method(&result, "join", &[ex.text("\n")?])
}

static TRIM_CELL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "c => c.trim()",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        Exec { heap, runtime }.method(&value, "trim", &[])
    },
};
static NORMALIZE_CELL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "c => { const trimmed = c.trim(); const left = trimmed.startsWith(\":\"); const right = trimmed.endsWith(\":\"); if (left && right) return \":---:\"; if (left) return \":---\"; if (right) return \"---:\"; return \"---\"; }",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let ex = Exec { heap, runtime };
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        let trimmed = ex.method(&value, "trim", &[])?;
        let left = ex.method(&trimmed, "startsWith", &[ex.text(":")?])?;
        let right = ex.method(&trimmed, "endsWith", &[ex.text(":")?])?;
        let left = heap.truthy(&left)?;
        let right = heap.truthy(&right)?;
        ex.text(match (left, right) {
            (true, true) => ":---:",
            (true, false) => ":---",
            (false, true) => "---:",
            _ => "---",
        })
    },
};
static ASCII_REPLACEMENT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "match => ASCII_SYMBOL_REPLACEMENTS[match]",
    name: "replaceAsciiSymbol",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        let primitive = heap.to_primitive(&value, true, runtime)?;
        let key = match heap.snapshot(&primitive)? {
            ValueSnapshot::Symbol { key, .. } => ara_rpc::value_heap::PropertyKey::Symbol(key),
            _ => heap.to_string(&primitive, runtime)?.into(),
        };
        heap.get(&captures[0], &key, runtime)
    },
};
