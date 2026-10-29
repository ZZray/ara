//! Fixed response-global sequential-cutoff reasoning, shared.ts 2484-2642.
use crate::actual_provider::{GraphBuilder, concat};
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle},
};
type Result<T> = std::result::Result<T, ValueError>;
pub(super) struct Cutoff {
    summary: ValueHandle,
    emitted: WireString,
}
impl Cutoff {
    pub fn new(b: &GraphBuilder) -> Result<Self> {
        Ok(Self { summary: b.0.heap().array_with_length(0)?, emitted: "".into() })
    }
    fn fold(b: &GraphBuilder, summary: &ValueHandle) -> Result<WireString> {
        if b.is_nullish(summary)? {
            return Ok("".into());
        }
        let mut canonical = WireString::from("");
        for part in b.members(summary)? {
            let value = b.get(&part, "text")?;
            if !b.0.heap().truthy(&value)? {
                continue;
            }
            let text = b.string(&value)?;
            if text == canonical {
                continue;
            }
            let extends = text.units().starts_with(canonical.units()) && text.units().get(canonical.len()) == Some(&10);
            canonical =
                if canonical.is_empty() || extends { text } else { concat(&concat(&canonical, &"\n\n".into()), &text) };
        }
        Ok(canonical)
    }
    pub fn done(&mut self, b: &GraphBuilder, text: WireString, index: u32, empty_block: bool) -> Result<WireString> {
        while b.0.heap().array_length(&self.summary, b.0.runtime())? <= index {
            b.push(&self.summary, b.object([("type", b.text("summary_text")?), ("text", b.text("")?)])?)?;
        }
        let part = b.get(&self.summary, &index.to_string())?;
        b.put(&part, "text", b.text(text)?)?;
        let after = Self::fold(b, &self.summary)?;
        if !after.units().starts_with(self.emitted.units()) {
            return Ok("".into());
        }
        let mut delta = WireString::from_units(after.units()[self.emitted.len()..].to_vec());
        if delta.is_empty() {
            return Ok(delta);
        }
        self.emitted = after;
        if empty_block {
            delta = WireString::from_units(delta.units().iter().skip_while(|unit| **unit == 10).copied().collect());
        }
        Ok(delta)
    }
    pub fn finalize(
        &mut self,
        b: &GraphBuilder,
        item: &ValueHandle,
        streamed: &WireString,
    ) -> Result<Option<WireString>> {
        if !streamed.is_empty() {
            return Ok(Some(streamed.clone()));
        }
        let summary = b.get(item, "summary")?;
        let thinking = Self::fold(b, &summary)?;
        if thinking.is_empty() {
            return Ok(None);
        }
        if self.emitted.units().starts_with(thinking.units()) {
            return Ok(Some("".into()));
        }
        if self.emitted.is_empty() || thinking.units().starts_with(self.emitted.units()) {
            let suffix = WireString::from_units(
                thinking.units()[self.emitted.len()..].iter().skip_while(|unit| **unit == 10).copied().collect(),
            );
            let copy = b.0.heap().array_with_length(0)?;
            if !b.is_nullish(&summary)? {
                for part in b.members(&summary)? {
                    b.push(&copy, b.0.heap().spread(&part, b.0.runtime())?)?;
                }
            }
            self.summary = copy;
            self.emitted = thinking;
            return Ok(Some(suffix));
        }
        Ok(Some("".into()))
    }
}
