//! Actual provider inputs and graph construction, fixed OMP 596f2da (MIT).
//! The Host supplies one factory for a Session lineage. These are temporary
//! request projections; this module neither owns nor replaces the transcript.
use crate::graph_values::GraphValueFactory;
use ara_rpc::{
    WireString, WireValue,
    value_heap::{PropertyKey, StringifyMode, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::sync::Arc;

/// A Host binding for the fixed Bun string-hash primitive. AI constructs the
/// original source seed; the binding carries no Session or product state.
pub trait HistoryIdHashPort: Send + Sync + std::fmt::Debug {
    fn hash_string_base36(&self, value: &WireString) -> Result<WireString, ValueError>;
}

#[derive(Clone, Debug)]
pub struct GraphProviderContext {
    /// Original post-hook Context in this Session lineage. Actual consumers
    /// Get its fields when the fixed source does; Vec fields below are legacy
    /// derived observations and must never be re-imported as this container.
    pub original_context: Option<ValueHandle>,
    pub system_prompt: Vec<WireString>,
    pub messages: Vec<ValueHandle>,
    pub tools: Option<Arc<[ValueHandle]>>,
    /// Host's fixed catalog classification for the selected target model.
    /// Only a cross-model demotion consumes it; it is not transcript content.
    pub history_demotion_dialect: Option<crate::dialect::Dialect>,
    pub history_id_hash: Option<Arc<dyn HistoryIdHashPort>>,
    /// Original imported Bun.hash Function for callbacks that a selected
    /// Native map may retain. The Function is an arena edge and invokes the
    /// current runtime; this must never capture a Host/factory Arc.
    pub history_id_hash_function: Option<ValueHandle>,
}

/// Neutral name mapping for Hosts that expose the fixed catalog's preferred
/// dialect as a string. No model-name classification is performed in AI.
pub fn graph_history_demotion_dialect(name: &str) -> Option<crate::dialect::Dialect> {
    use crate::dialect::Dialect;
    Some(match name {
        "glm" => Dialect::Glm,
        "hermes" => Dialect::Hermes,
        "kimi" => Dialect::Kimi,
        "xml" => Dialect::Xml,
        "anthropic" => Dialect::Anthropic,
        "deepseek" => Dialect::Deepseek,
        "harmony" => Dialect::Harmony,
        "qwen3" => Dialect::Qwen3,
        "gemini" => Dialect::Gemini,
        "gemma" => Dialect::Gemma,
        "minimax" => Dialect::Minimax,
        _ => return None,
    })
}
impl GraphProviderContext {
    pub fn original_field(&self, factory: &GraphValueFactory, key: &str) -> Result<ValueHandle, ValueError> {
        let original = self
            .original_context
            .as_ref()
            .ok_or(ValueError::UnsupportedOperation("Original Native provider Context is not connected"))?;
        factory.heap().adopt(original)?;
        factory.heap().get(original, &key.into(), factory.runtime())
    }
    pub fn check_lineage(&self, factory: &GraphValueFactory) -> Result<(), ValueError> {
        if let Some(original) = &self.original_context {
            factory.heap().adopt(original)?;
        }
        if let Some(hash) = &self.history_id_hash_function {
            factory.heap().adopt(hash)?;
        }
        for value in self.messages.iter().chain(self.tools.iter().flat_map(|tools| tools.iter())) {
            factory.heap().adopt(value)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct GraphBuilder(pub Arc<GraphValueFactory>);
impl GraphBuilder {
    /// Only newly derived protocol knobs/transport counters use this encoder.
    /// Actual Context, transcript, content and arguments never enter serde.
    pub(crate) fn derived<T: serde::Serialize>(&self, value: &T) -> Result<ValueHandle, ValueError> {
        let text = serde_json::to_string(value)
            .map_err(|_| ValueError::UnsupportedOperation("Derived provider metadata JSON encoding failed"))?;
        let wire = WireValue::parse(&text)
            .map_err(|_| ValueError::UnsupportedOperation("Derived provider metadata JSON parsing failed"))?;
        self.import_wire(wire)
    }
    pub(crate) fn get(&self, root: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.0.heap().get(root, &PropertyKey::from(key), self.0.runtime())
    }
    pub(crate) fn put(&self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<(), ValueError> {
        self.0.heap().assign(root, PropertyKey::from(key), value, self.0.runtime())
    }
    pub(crate) fn define(&self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<(), ValueError> {
        self.0.heap().define_own_data(root, PropertyKey::from(key), value, self.0.runtime())
    }
    pub(crate) fn object(
        &self,
        entries: impl IntoIterator<Item = (&'static str, ValueHandle)>,
    ) -> Result<ValueHandle, ValueError> {
        let value = self.0.heap().object()?;
        for (key, member) in entries {
            self.define(&value, key, member)?;
        }
        Ok(value)
    }
    pub(crate) fn text(&self, value: impl Into<WireString>) -> Result<ValueHandle, ValueError> {
        self.0.heap().string(value.into())
    }
    pub(crate) fn string(&self, value: &ValueHandle) -> Result<WireString, ValueError> {
        match self.0.heap().snapshot(value)? {
            ValueSnapshot::String(value) => Ok(value),
            _ => Err(ValueError::TypeError("Provider string field must be a string".into())),
        }
    }
    pub(crate) fn optional_string(&self, root: &ValueHandle, key: &str) -> Result<Option<WireString>, ValueError> {
        let value = self.get(root, key)?;
        match self.0.heap().snapshot(&value)? {
            ValueSnapshot::Undefined | ValueSnapshot::Null => Ok(None),
            ValueSnapshot::String(value) => Ok(Some(value)),
            _ => Err(ValueError::TypeError("Provider optional string field must be a string".into())),
        }
    }
    pub(crate) fn push(&self, array: &ValueHandle, member: ValueHandle) -> Result<(), ValueError> {
        let method = self.get(array, "push")?;
        self.0.heap().call(&method, array, &[member], self.0.runtime())?;
        Ok(())
    }
    pub(crate) fn members(&self, array: &ValueHandle) -> Result<Vec<ValueHandle>, ValueError> {
        let heap = self.0.heap();
        let mut iterator = heap.get_iterator(array, self.0.runtime())?;
        let mut members = Vec::new();
        loop {
            match heap.iterator_next(&mut iterator, self.0.runtime()) {
                Ok(Some(member)) => members.push(member),
                Ok(None) => return Ok(members),
                Err(primary) => {
                    // Preserve the primary Native abrupt completion. Closing
                    // a Native iterator cannot replace an existing throw.
                    let _ = heap.iterator_close(&mut iterator, self.0.runtime());
                    return Err(primary);
                }
            }
        }
    }
    pub(crate) fn plain_json(&self, value: &ValueHandle) -> Result<WireString, ValueError> {
        self.0
            .heap()
            .stringify(value, StringifyMode::Plain, None, self.0.runtime())?
            .ok_or(ValueError::TypeError("Provider JSON root is undefined".into()))
    }
    pub(crate) fn is_nullish(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.0.heap().kind(value)?, ValueKind::Undefined | ValueKind::Null))
    }
    pub(crate) fn import_wire(&self, value: WireValue) -> Result<ValueHandle, ValueError> {
        self.0.heap().import_strict(value)
    }
}

pub(crate) fn new_assistant(
    builder: &GraphBuilder,
    model: &crate::Model,
) -> Result<(ValueHandle, ValueHandle), ValueError> {
    let heap = builder.0.heap();
    let content = heap.array_with_length(0)?;
    let root = builder.object([
        ("role", builder.text("assistant")?),
        ("content", content.clone()),
        ("api", builder.text(model.api.clone())?),
        ("provider", builder.text(model.provider.clone())?),
        ("model", builder.text(model.id.clone())?),
        ("usage", heap.object()?),
        ("stopReason", builder.text("stop")?),
        ("timestamp", heap.number(crate::types::now_ms() as f64)?),
    ])?;
    Ok((root, content))
}

pub(crate) fn concat(left: &WireString, right: &WireString) -> WireString {
    WireString::from_units(left.units().iter().chain(right.units()).copied().collect())
}
pub(crate) fn trim_empty(value: &WireString) -> bool {
    value.units().iter().all(|unit| char::from_u32(u32::from(*unit)).is_some_and(ara_prompt::js::is_space))
}
