//! Fixed OMP 596f2da stateful ordinary Responses consumer (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Graph handles stay in their Session lineage. Full exotic Bun.deepEquals
//! parity remains WIP; ordinary nested values use the existing selected port.
use super::super::{ChainLease, ChainSnapshot, MAX_CHAIN_BASELINE_BYTES, StreamOptions, chain_key};
use super::{callbacks, history};
use crate::actual_provider::{GraphBuilder, GraphProviderContext};
use crate::{Model, graph_values::GraphValueFactory};
use ara_rpc::{
    WireString,
    value_heap::{PropertyKey, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

#[derive(Debug)]
pub(crate) struct NativeChainBaseline {
    request: ValueHandle,
    response_items: ValueHandle,
    response_id: WireString,
}

pub(crate) struct NativeChainRequest {
    session: Arc<super::super::ProviderSessionState>,
    lease: Option<ChainLease>,
    full_request: ValueHandle,
    history_request: ValueHandle,
    active_request: ValueHandle,
    sent_previous: bool,
    pending_baseline: Option<Option<ChainSnapshot>>,
}

pub(super) fn scaffolding_key(b: &GraphBuilder) -> Result<PropertyKey> {
    b.0.heap().module_symbol("ara-ai.responses", "trailingScaffoldingItems")
}

fn length(b: &GraphBuilder, array: &ValueHandle) -> Result<u32> {
    b.0.heap().array_length(array, b.0.runtime())
}
fn slice(b: &GraphBuilder, array: &ValueHandle, start: u32, end: Option<u32>) -> Result<ValueHandle> {
    let mut args = vec![b.0.heap().number(f64::from(start))?];
    if let Some(end) = end {
        args.push(b.0.heap().number(f64::from(end))?);
    }
    callbacks::execution(b).method(array, "slice", &args)
}
fn nested_equal(b: &GraphBuilder, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
    // Same-graph comparison, never serde or JSON.stringify equality. This is
    // the existing selected Bun non-strict ordinary object/array port, not a
    // claim of Date/Map/Set/typed-array/exotic prototype parity.
    b.0.heap().mcp_deep_equals(left, right, b.0.runtime())
}
fn equal_without(b: &GraphBuilder, left: &ValueHandle, right: &ValueHandle, omit: &[&str]) -> Result<bool> {
    let heap = b.0.heap();
    let runtime = b.0.runtime();
    if !heap.truthy(left)?
        || !heap.truthy(right)?
        || !matches!(heap.kind(left)?, ValueKind::Object | ValueKind::Array)
        || !matches!(heap.kind(right)?, ValueKind::Object | ValueKind::Array)
    {
        return nested_equal(b, left, right);
    }
    for key in heap.for_in_string_keys(left, runtime)? {
        if omit.iter().any(|omitted| key.equals_ascii(omitted)) {
            continue;
        }
        let av = heap.get(left, &PropertyKey::String(key.clone()), runtime)?;
        let bv = heap.get(right, &PropertyKey::String(key), runtime)?;
        if !nested_equal(b, &av, &bv)? {
            return Ok(false);
        }
    }
    for key in heap.for_in_string_keys(right, runtime)? {
        if omit.iter().any(|omitted| key.equals_ascii(omitted)) {
            continue;
        }
        let property = PropertyKey::String(key);
        let bv = heap.get(right, &property, runtime)?;
        if heap.kind(&bv)? != ValueKind::Undefined && !heap.has(left, &property, runtime)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn delta(b: &GraphBuilder, baseline: &NativeChainBaseline, current: &ValueHandle) -> Result<Option<ValueHandle>> {
    let previous = b.get(&baseline.request, "input")?;
    let now = b.get(current, "input")?;
    if b.0.heap().kind(&previous)? != ValueKind::Array || b.0.heap().kind(&now)? != ValueKind::Array {
        return Ok(None);
    }
    if !equal_without(b, &baseline.request, current, &["input", "client_metadata"])? {
        return Ok(None);
    }
    let Some(prefix_len) = length(b, &previous)?.checked_add(length(b, &baseline.response_items)?) else {
        return Ok(None);
    };
    if length(b, &now)? <= prefix_len {
        return Ok(None);
    }
    let mut index = 0_u32;
    for series in [&previous, &baseline.response_items] {
        for item in b.members(series)? {
            let kind = b.get(&item, "type")?;
            let omit_id = matches!(b.0.heap().snapshot(&kind)?, ValueSnapshot::String(kind)
                if ["message", "function_call", "custom_tool_call"].iter().any(|expected| kind.equals_ascii(expected)));
            let actual = b.0.heap().get(&now, &PropertyKey::String(index.to_string().into()), b.0.runtime())?;
            let omit = if omit_id { &["status", "id"][..] } else { &["status"][..] };
            if !equal_without(b, &item, &actual, omit)? {
                return Ok(None);
            }
            index += 1;
        }
    }
    Ok(Some(slice(b, &now, index, None)?))
}

impl NativeChainRequest {
    pub(crate) fn begin(b: &GraphBuilder, model: &Model, options: &StreamOptions, body: &ValueHandle) -> Result<Self> {
        let session = options
            .session_state
            .as_ref()
            .ok_or(ValueError::UnsupportedOperation("Actual Responses stateful Session owner is absent"))?;
        let lease = ChainLease::begin(session.clone(), chain_key(model, options));
        // A disabled circuit keeps normal full requests and its original store
        // value; it does not create a separate Native owner or reset the host.
        let full_request = if lease.is_some() {
            let full = b.0.heap().spread(body, b.0.runtime())?;
            b.define(&full, "store", b.0.heap().boolean(true)?)?;
            full
        } else {
            body.clone()
        };
        let count = b.0.heap().get(&full_request, &scaffolding_key(b)?, b.0.runtime())?;
        let count = match b.0.heap().snapshot(&count)? {
            ValueSnapshot::Undefined => 0,
            ValueSnapshot::Number(value) if value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX) => {
                value as u32
            }
            _ => return Err(ValueError::TypeError("Invalid Responses scaffolding metadata".into())),
        };
        let history_request = if count > 0 {
            let input = b.get(&full_request, "input")?;
            let keep = length(b, &input)?
                .checked_sub(count)
                .ok_or(ValueError::TypeError("Responses scaffolding exceeds input".into()))?;
            let history = b.0.heap().spread(&full_request, b.0.runtime())?;
            b.define(&history, "input", slice(b, &input, 0, Some(keep))?)?;
            history
        } else {
            full_request.clone()
        };
        let mut active_request = full_request.clone();
        let mut sent_previous = false;
        if let Some(lease) = &lease
            && let Some(ChainSnapshot::Native(baseline)) = &lease.baseline
            && let Some(delta) = delta(b, baseline, &history_request)?
        {
            let input = if count > 0 {
                let full_input = b.get(&full_request, "input")?;
                let tail = slice(b, &full_input, length(b, &full_input)? - count, None)?;
                // Fixed [...delta, ...scaffolding] uses each original iterator.
                let mut items = b.members(&delta)?;
                items.extend(b.members(&tail)?);
                history::array(b, items)?
            } else {
                delta
            };
            active_request = b.0.heap().spread(&full_request, b.0.runtime())?;
            b.define(&active_request, "input", input)?;
            b.define(&active_request, "previous_response_id", b.text(baseline.response_id.clone())?)?;
            sent_previous = true;
        }
        Ok(Self {
            session: session.clone(),
            lease,
            full_request,
            history_request,
            active_request,
            sent_previous,
            pending_baseline: None,
        })
    }
    pub(crate) fn body(&self) -> &ValueHandle {
        &self.active_request
    }
    pub(crate) fn sent_previous(&self) -> bool {
        self.sent_previous
    }
    pub(crate) fn stale_rejection(&mut self, b: &GraphBuilder, zero_data_retention: bool) -> Result<()> {
        if let Some(lease) = &mut self.lease {
            lease.stale_rejection(zero_data_retention);
        }
        self.active_request = self.full_request.clone();
        if zero_data_retention {
            b.define(&self.active_request, "store", b.0.heap().boolean(false)?)?;
        }
        self.sent_previous = false;
        Ok(())
    }
    pub(crate) fn complete(
        &mut self,
        model: &Model,
        output: &ValueHandle,
        items: &ValueHandle,
        factory: Arc<GraphValueFactory>,
        hash: Option<Arc<dyn crate::actual_provider::HistoryIdHashPort>>,
    ) -> Result<()> {
        let b = GraphBuilder(factory);
        let context = GraphProviderContext {
            original_context: None,
            system_prompt: Vec::new(),
            messages: Vec::new(),
            tools: None,
            history_demotion_dialect: None,
            history_id_hash: hash,
            history_id_hash_function: None,
        };
        // This is the same admission used by replay warmup; run it only once.
        let response_items = history::replayable_native_items(&b, &context, items)?;
        if response_items.is_some() {
            self.session.warm(&model.provider);
        }
        if self.lease.is_none() {
            return Ok(());
        }
        let response_id = b.optional_string(output, "responseId")?.filter(|id| !id.is_empty());
        let baseline = match (response_id, response_items) {
            (Some(response_id), Some(response_items)) => {
                let response_items = history::array(&b, response_items)?;
                let request = b.0.heap().structured_clone_json(&self.history_request, b.0.runtime())?;
                // Bounded storage is ARA's existing shared-owner contract. The
                // measurement sees the detached successful clone, not Context.
                let bytes = b
                    .plain_json(&request)?
                    .to_utf8()
                    .map_err(|_| ValueError::UnsupportedOperation("JSON.stringify baseline must contain scalar UTF-8"))?
                    .len();
                if bytes > MAX_CHAIN_BASELINE_BYTES {
                    None
                } else {
                    Some(ChainSnapshot::Native(Arc::new(NativeChainBaseline { request, response_items, response_id })))
                }
            }
            _ => None,
        };
        // Keep fresh state provisional while the original output's duration
        // assignment and terminal event construction may still throw.
        self.pending_baseline = Some(baseline);
        Ok(())
    }
    pub(crate) fn publish(&mut self) {
        if let Some(lease) = &mut self.lease
            && let Some(baseline) = self.pending_baseline.take()
        {
            lease.publish_baseline(baseline, self.sent_previous, true);
        }
    }
    pub(crate) fn fail_completion(&mut self) {
        self.pending_baseline = None;
        if let Some(lease) = &mut self.lease {
            lease.fail_completion();
        }
    }
}
