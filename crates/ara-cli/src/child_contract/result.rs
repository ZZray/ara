//! Fixed b91db66e yield assembly/executor slices, MIT; THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
use super::{
    ChildContractOwner, YieldReceipt, YieldStatus,
    schema::{self, get, members, normalize_graph, nullish, string, validate},
};
use ara_agent::ActualProviderError;
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildResultStatus {
    Success,
    Aborted,
    Invalid,
    PendingOwnedWork,
    Failed,
}
#[derive(Clone)]
pub struct ObservedChildResult {
    pub status: ChildResultStatus,
    pub data: Option<ValueHandle>,
    pub error: Option<String>,
    pub schema_overridden: bool,
    pub accepted: bool,
}
pub struct ChildRunFinish {
    pub clean_exit: bool,
    pub cancelled: bool,
    pub owned_jobs_settled: bool,
    pub last_assistant_text: Option<WireString>,
}
struct Observations {
    admitted: HashSet<WireString>,
    consumed: HashSet<WireString>,
    receipts: Vec<YieldReceipt>,
    invalidated_by_async: bool,
}
pub struct ChildRunObserver {
    owner: Arc<ChildContractOwner>,
    pub turn_id: u64,
    state: Mutex<Observations>,
}
impl ChildRunObserver {
    pub(crate) fn new(owner: Arc<ChildContractOwner>, turn_id: u64) -> Arc<Self> {
        Arc::new(Self {
            owner,
            turn_id,
            state: Mutex::new(Observations {
                admitted: HashSet::new(),
                consumed: HashSet::new(),
                receipts: Vec::new(),
                invalidated_by_async: false,
            }),
        })
    }
    pub(crate) fn admit(&self, id: WireString) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admitted
            .insert(id);
    }
    pub(crate) fn observe(
        &self,
        id: WireString,
        receipt: YieldReceipt,
    ) -> Result<(), ActualProviderError> {
        let v = self.owner.values();
        let h = v.heap();
        if !matches!(h.snapshot(&get(h,&self.owner.graph,"turn",v.runtime())?)?,ValueSnapshot::Number(n) if n==self.turn_id as f64)
        {
            return Err(ActualProviderError::Protocol(
                "Yield result belongs to a superseded child turn",
            ));
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.admitted.contains(&id) {
            return Err(ActualProviderError::Protocol(
                "Yield result has no exact admitted call",
            ));
        }
        if s.consumed.insert(id) {
            if receipt.terminal {
                s.invalidated_by_async = false;
            }
            s.receipts.push(receipt)
        }
        Ok(())
    }
    pub fn invalidate_for_async_work(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invalidated_by_async = true;
    }
    pub fn has_terminal_yield(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        !s.invalidated_by_async && s.receipts.iter().any(|r| r.terminal)
    }
    pub fn has_yields(&self) -> bool {
        !self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .receipts
            .is_empty()
    }
    pub fn receipts(&self) -> Vec<YieldReceipt> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .receipts
            .clone()
    }
    pub fn finish(&self, run: ChildRunFinish) -> Result<ObservedChildResult, ActualProviderError> {
        self.finish_with_runtime(run, self.owner.values().runtime())
    }
    /// Preserve the operation-local resolver for Graph accessors when finishing
    /// within a borrowed Native operation; never replace it with a serde mirror.
    pub fn finish_with_runtime(
        &self,
        run: ChildRunFinish,
        runtime: &dyn ara_rpc::value_heap::ValueRuntime,
    ) -> Result<ObservedChildResult, ActualProviderError> {
        // A salvageable output is distinct from a delivered structured result.
        // fixed executor.ts2343 requires the actual monitor's yieldCalled receipt.
        let delivered = !self.owner.config().structured || self.has_terminal_yield();
        let result = |status, data, error, overridden| ObservedChildResult {
            status,
            data,
            error,
            schema_overridden: overridden,
            accepted: status == ChildResultStatus::Success && delivered,
        };
        if run.cancelled || !run.clean_exit {
            return Ok(result(
                ChildResultStatus::Failed,
                None,
                Some("Child Run failed or was cancelled".into()),
                false,
            ));
        }
        if !run.owned_jobs_settled {
            return Ok(result(
                ChildResultStatus::PendingOwnedWork,
                None,
                Some("Child-owned jobs have not settled".into()),
                false,
            ));
        }
        if self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invalidated_by_async
        {
            return Ok(result(
                ChildResultStatus::PendingOwnedWork,
                None,
                Some("Child must submit a fresh yield after owned job results were adopted".into()),
                false,
            ));
        }
        let v = self.owner.values();
        let h = v.heap();
        let config = self.owner.config();
        let receipts = self.receipts();
        if let Some(last) = receipts.last() {
            if last.status == YieldStatus::Aborted {
                let error = get(h, &last.details, "error", runtime)?;
                let text = if h.kind(&error)? == ValueKind::String {
                    string(h, &error)?
                        .to_utf8()
                        .unwrap_or_else(|_| "Subagent aborted task".into())
                } else {
                    "Subagent aborted task".into()
                };
                return Ok(result(ChildResultStatus::Aborted, None, Some(text), false));
            }
        }
        if receipts.is_empty() && !config.structured {
            let mut text = run.last_assistant_text;
            if let Some(value) = &text {
                let value = h.string(value.clone())?;
                let trim = get(h, &value, "trim", runtime)?;
                let trimmed = h.call(&trim, &value, &[], runtime)?;
                if string(h, &trimmed)?.units().is_empty() {
                    text = None;
                }
            }
            return Ok(match text {
                Some(text) => result(
                    ChildResultStatus::Success,
                    Some(h.string(text)?),
                    None,
                    false,
                ),
                None => result(
                    ChildResultStatus::Failed,
                    None,
                    Some("Child final assistant message is empty".into()),
                    false,
                ),
            });
        }
        let last = run.last_assistant_text.map(|s| h.string(s)).transpose()?;
        let sections = h.object()?;
        let mut counts: HashMap<WireString, usize> = HashMap::new();
        let mut scalar_overrides = HashSet::new();
        let mut any_override = false;
        let mut missing = false;
        let mut has_sections = false;
        let declared = get(h, &self.owner.graph, "labelSchema", runtime)?;
        for receipt in &receipts {
            if receipt.status == YieldStatus::Aborted || receipt.labels.is_empty() {
                continue;
            }
            let value = if receipt.use_last_turn || receipt.data.is_none() {
                last.clone()
            } else {
                receipt.data.clone()
            };
            missing |= value.as_ref().is_none_or(|v| nullish(h, v).unwrap_or(true));
            let value = value.unwrap_or(h.undefined()?);
            let mut labels = Vec::new();
            for label in &receipt.labels {
                let text = h.string(label.clone())?;
                let trim = get(h, &text, "trim", runtime)?;
                let trimmed = h.call(&trim, &text, &[], runtime)?;
                let label = string(h, &trimmed)?;
                if !label.units().is_empty() {
                    labels.push(label);
                }
            }
            if labels.is_empty() {
                any_override |= receipt.schema_overridden;
            }
            for label in &labels {
                let shape = section_shape(h, &declared, label, runtime)?;
                let count = counts.entry(label.clone()).or_default();
                let previous = h.get(&sections, &label.clone().into(), runtime)?;
                let next = if shape == Some(false) {
                    value.clone()
                } else if *count == 0 {
                    if shape == Some(true) {
                        h.array(vec![value.clone()])?
                    } else {
                        value.clone()
                    }
                } else if h.kind(&previous)? == ValueKind::Array {
                    let mut a = members(h, &previous, runtime)?;
                    a.push(value.clone());
                    h.array(a)?
                } else {
                    h.array(vec![previous, value.clone()])?
                };
                h.define_own_data(&sections, label.clone().into(), next, runtime)?;
                *count += 1;
                has_sections = true;
                if shape == Some(false) {
                    if receipt.schema_overridden {
                        scalar_overrides.insert(label.clone());
                    } else {
                        scalar_overrides.remove(label);
                    }
                } else {
                    any_override |= receipt.schema_overridden;
                }
            }
        }
        let terminal = receipts.iter().rev().find(|r| r.labels.is_empty());
        let (mut data, overridden, missing) =
            if let Some(terminal) = terminal.filter(|t| t.data.is_some()) {
                (
                    terminal.data.clone(),
                    terminal.schema_overridden,
                    terminal
                        .data
                        .as_ref()
                        .is_none_or(|d| nullish(h, d).unwrap_or(true)),
                )
            } else if has_sections {
                (
                    Some(sections),
                    any_override || !scalar_overrides.is_empty(),
                    missing,
                )
            } else if let Some(terminal) = terminal {
                let value = terminal.data.clone().or_else(|| last.clone());
                let missing = value.as_ref().is_none_or(|d| nullish(h, d).unwrap_or(true));
                (value, terminal.schema_overridden, missing)
            } else {
                let mut parsed = last
                    .as_ref()
                    .and_then(|t| string(h, t).ok())
                    .and_then(|s| h.parse_strict_units(&s).ok());
                if let Some(object) = parsed
                    .as_ref()
                    .filter(|v| h.kind(v).is_ok_and(|k| k == ValueKind::Object))
                {
                    if h.has(object, &"data".into(), runtime)? {
                        parsed = Some(get(h, object, "data", runtime)?);
                    }
                }
                let built = schema::build(config.output_schema.as_ref());
                if built.error.is_some() {
                    parsed = None;
                }
                if parsed.is_none() && built.normalized.is_none() && built.error.is_none() {
                    if let Some(raw) = &last {
                        let trim = get(h, raw, "trim", runtime)?;
                        let trimmed = h.call(&trim, raw, &[], runtime)?;
                        if !string(h, &trimmed)?.units().is_empty() {
                            parsed = Some(raw.clone());
                        }
                    }
                }
                (parsed, false, false)
            };
        if missing || data.is_none() {
            return Ok(result(
                ChildResultStatus::Invalid,
                data,
                Some("Subagent yield contained no usable data".into()),
                overridden,
            ));
        }
        let schema = get(h, &self.owner.graph, "schema", runtime)?;
        let mut value = data.take().unwrap();
        if h.kind(&value)? == ValueKind::String {
            let trim = get(h, &value, "trim", runtime)?;
            let trimmed = h.call(&trim, &value, &[], runtime)?;
            let text = string(h, &trimmed)?;
            if text.units().first().is_some_and(|u| matches!(*u, 123 | 91)) {
                if let Ok(parsed) = h.parse_strict_units(&text) {
                    value = parsed
                }
            }
        }
        if config.output_schema_mode == "strict" {
            value = normalize_graph(h, &declared, &value, runtime)?
        }
        if let Some(error) = validate(h, &schema, &value, runtime)? {
            if config.output_schema_mode == "strict" || !overridden {
                return Ok(result(
                    ChildResultStatus::Invalid,
                    Some(value),
                    Some(error),
                    overridden,
                ));
            }
        }
        let built = schema::build(config.output_schema.as_ref());
        let warning = if overridden {
            Some("Yield schema validation was overridden after consecutive failures".into())
        } else {
            built.error.clone()
        };
        if config.output_schema_mode == "strict" && warning.is_some() {
            return Ok(result(
                ChildResultStatus::Invalid,
                Some(value),
                warning,
                overridden,
            ));
        }
        Ok(result(
            ChildResultStatus::Success,
            Some(value),
            warning,
            overridden,
        ))
    }
}
fn section_shape(
    h: &Arc<ara_rpc::value_heap::ValueHeap>,
    schema: &ValueHandle,
    label: &WireString,
    r: &dyn ara_rpc::value_heap::ValueRuntime,
) -> Result<Option<bool>, ValueError> {
    if h.kind(schema)? != ValueKind::Object {
        return Ok(None);
    }
    let props = get(h, schema, "properties", r)?;
    let mut shapes = Vec::new();
    if h.kind(&props)? == ValueKind::Object && h.has(&props, &label.clone().into(), r)? {
        shapes.push(array_schema(
            h,
            &h.get(&props, &label.clone().into(), r)?,
            r,
        )?);
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        let variants = get(h, schema, key, r)?;
        if h.kind(&variants)? == ValueKind::Array {
            for branch in members(h, &variants, r)? {
                if let Some(shape) = section_shape(h, &branch, label, r)? {
                    shapes.push(shape)
                }
            }
        }
    }
    Ok(shapes
        .first()
        .copied()
        .filter(|shape| shapes.iter().all(|s| s == shape)))
}
fn array_schema(
    h: &Arc<ara_rpc::value_heap::ValueHeap>,
    schema: &ValueHandle,
    r: &dyn ara_rpc::value_heap::ValueRuntime,
) -> Result<bool, ValueError> {
    if h.kind(schema)? != ValueKind::Object {
        return Ok(false);
    }
    let ty = get(h, schema, "type", r)?;
    if matches!(h.snapshot(&ty)?,ValueSnapshot::String(s) if s.equals_ascii("array")) {
        return Ok(true);
    }
    if h.kind(&ty)? == ValueKind::Array {
        for t in members(h, &ty, r)? {
            if matches!(h.snapshot(&t)?,ValueSnapshot::String(s) if s.equals_ascii("array")) {
                return Ok(true);
            }
        }
    }
    for k in ["oneOf", "anyOf", "allOf"] {
        let a = get(h, schema, k, r)?;
        if h.kind(&a)? == ValueKind::Array {
            for s in members(h, &a, r)? {
                if array_schema(h, &s, r)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
