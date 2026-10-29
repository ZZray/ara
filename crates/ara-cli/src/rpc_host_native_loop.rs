//! Fixed OMP596f2da stream-guards.ts:177-238 Native RPC turn-end consumer.
//! MIT; Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! See THIRD_PARTY_NOTICES.md. Core owns transcript/journal input insertion.
use super::*;
use ara_agent::{ActualProviderError, AgentMessageBody};
use ara_ai::StopReason;
use ara_rpc::value_heap::{ValueHandle, ValueSnapshot};

#[derive(Clone, Copy)]
pub(super) enum StopObservation {
    Observed(Option<StopReason>),
    CurrentGraph,
}

/// The direct Native consumer preserves the original custom Graph notice.
/// The contextual production RPC consumer must admit its public projection
/// before Core inserts/persists the input or starts the next model call.
#[derive(Clone, Copy)]
pub(super) enum NoticeBoundary {
    Native,
    PublicRpc,
}

fn current_root(message: &AgentMessage, values: &SessionValues) -> Result<ValueHandle, ActualProviderError> {
    let AgentMessageBody::Graph(root) = message.body_snapshot() else {
        return Err(ActualProviderError::Protocol("Native RPC loop guard requires current Graph message bodies"));
    };
    values.check_handle(&root)?;
    Ok(root)
}

impl ToolLoopHooks {
    #[allow(clippy::too_many_arguments)] // Original stop observation and explicit Host public boundary.
    pub(super) fn append_loop_actual(
        &self,
        mut inputs: Vec<AgentMessage>,
        message: &AgentMessage,
        results: &[AgentMessage],
        cancel: &CancellationToken,
        values: &SessionValues,
        stop: StopObservation,
        notice_boundary: NoticeBoundary,
    ) -> Result<Vec<AgentMessage>, ActualProviderError> {
        if cancel.is_cancelled() {
            return Ok(inputs);
        }
        if !self.settings.enabled {
            self.state.lock().unwrap().configure(self.settings.clone());
            return Ok(inputs);
        }
        if matches!(stop, StopObservation::Observed(Some(StopReason::Error | StopReason::Aborted))) {
            return Ok(inputs);
        }
        // The base hook already completed. These are its current original
        // bodies; neither typed snapshots nor reconstructed projections enter.
        let root = current_root(message, values)?;
        let heap = values.heap();
        let runtime = values.runtime();
        if matches!(stop, StopObservation::CurrentGraph) {
            let reason = heap.get(&root, &"stopReason".into(), runtime)?;
            if matches!(heap.snapshot(&reason)?, ValueSnapshot::String(reason)
                if reason.equals_ascii("error") || reason.equals_ascii("aborted"))
            {
                return Ok(inputs);
            }
        }
        let role = heap.get(&root, &"role".into(), runtime)?;
        if !matches!(heap.snapshot(&role)?, ValueSnapshot::String(role) if role.equals_ascii("assistant")) {
            return Ok(inputs);
        }
        let native = self
            .native_session
            .as_ref()
            .ok_or(ActualProviderError::Unsupported("RPC Native Session prompt/logger owner"))?;
        if !Arc::ptr_eq(native.values().factory(), values.factory()) {
            return Err(ActualProviderError::Protocol("RPC Native loop owner/current values factory mismatch"));
        }
        let logger = native.logger().ok_or(ActualProviderError::Unsupported("RPC original Native logger binding"))?;
        values.check_handle(logger)?;
        let result_roots = results.iter().map(|result| current_root(result, values)).collect::<Result<Vec<_>, _>>()?;
        let result_array = heap.array(result_roots)?;
        let turn = heap.object()?;
        heap.define_own_data(&turn, "message".into(), root, runtime)?;
        heap.define_own_data(&turn, "toolResults".into(), result_array, runtime)?;
        let factory = values.factory();
        let mut lease = {
            let mut state = self.state.lock().unwrap();
            state.begin_actual(self.settings.clone(), factory)
        }
        .expect("enabled Native guard");
        if lease.exempt_set.is_none() {
            // Constructors and callbacks run without the Host state mutex.
            let set = ara_ai::tool_call_loop_guard::actual::create_exempt_set(factory, &lease.exempt_tools)?;
            lease = self.state.lock().unwrap().install_actual_exempt_set(&lease, set)?;
        }
        let prepared = ara_ai::tool_call_loop_guard::actual::prepare_turn(
            factory,
            &turn,
            lease.exempt_set.as_ref().expect("installed Native Set"),
        )?;
        let hit = self.state.lock().unwrap().commit_actual_turn(&lease, &prepared)?;
        let Some(count) = hit else { return Ok(inputs) };
        let detection = ara_ai::tool_call_loop_guard::actual::finish_turn(factory, &prepared, count)?;
        // Original callee Get precedes object argument field Gets, then the
        // original logger Call precedes render. A Throw keeps committed count.
        let warn = heap.get(logger, &"warn".into(), runtime)?;
        let warning = heap.string("cross-turn tool-call loop detected".into())?;
        let fields = heap.object()?;
        for key in ["toolName", "count"] {
            let value = heap.get(&detection, &key.into(), runtime)?;
            heap.define_own_data(&fields, key.into(), value, runtime)?;
        }
        heap.call(&warn, logger, &[warning, fields], runtime)?;
        let custom = ara_session::actual_loop_guard_notice::tool_call_loop(factory, native.prompt(), &detection)?;
        if matches!(notice_boundary, NoticeBoundary::PublicRpc)
            && let Some(details) = ara_cli::actual_host::loop_notice_public_utf16_refusal(&custom, values)?
        {
            // Same Host refusal as the legacy hook: preserve exact UTF-16
            // details and both Run cancellation tokens, never the connection.
            let mut receipt = wire(json!({"type":"notice","level":"error","source":"loop-guard",
                "sessionId":self.session_id,
                "message":"tool-call loop notice contains UTF-16 that cannot be projected to current UTF-8 public text"}));
            receipt.insert("details", details);
            self.output.send(OutputItem::Frame(receipt));
            cancel.cancel();
            self.run_cancel.cancel();
            return Ok(inputs);
        }
        inputs.push(AgentMessage::from_graph(custom, None)?);
        Ok(inputs)
    }
}
