//! Fixed grammar.ts compactGrammarDefinition (OMP596f2da, MIT).
use super::callbacks::Exec;
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle},
};
type Result<T> = std::result::Result<T, ValueError>;
pub(super) fn compact(ex: Exec<'_>, syntax: &ValueHandle, definition: &ValueHandle) -> Result<ValueHandle> {
    if !matches!(ex.heap.snapshot(syntax)?,ara_rpc::value_heap::ValueSnapshot::String(text) if text.equals_ascii("lark"))
    {
        return Ok(definition.clone());
    }
    let split = ex.get(definition, "split")?;
    let separator = ex.heap.regexp(r"\r?\n".into(), "")?;
    let source = ex.heap.call(&split, definition, &[separator], ex.runtime)?;
    let lines = ex.heap.array_with_length(0)?;
    ex.for_of(&source, |line| {
        let source = ex.string(line)?;
        let mut quote = None;
        let mut regex = false;
        let mut escaped = false;
        let mut end = source.len();
        for (i, unit) in source.units().iter().copied().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            if unit == 92 {
                escaped = true;
                continue;
            }
            if let Some(current) = quote {
                if unit == current {
                    quote = None;
                }
                continue;
            }
            if regex {
                if unit == 47 {
                    regex = false;
                }
                continue;
            }
            if unit == 47 && source.units().get(i + 1) == Some(&47) {
                end = i;
                break;
            }
            if matches!(unit, 34 | 39) {
                quote = Some(unit);
                continue;
            }
            if unit == 47 {
                regex = true;
            }
        }
        // stripLarkLineComment's slice remains an actual string method when
        // there is a comment. Lone UTF16 units never pass through Rust UTF8.
        let uncommented = if end < source.len() {
            ex.method(line, "slice", &[ex.heap.number(0.0)?, ex.heap.number(end as f64)?])?
        } else {
            line.clone()
        };
        let uncommented = ex.method(&uncommented, "trimEnd", &[])?;
        if ex.heap.truthy(&ex.method(&uncommented, "trim", &[])?)? {
            ex.method(&lines, "push", &[uncommented])?;
        }
        Ok(true)
    })?;
    ex.method(&lines, "join", &[ex.text(WireString::from("\n"))?])
}
