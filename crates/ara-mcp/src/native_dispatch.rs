//! Native JSON network boundary on the original explicitly granted MCP owner.
//! Fixed OMP596f2da client.ts callTool / transports/stdio.ts request (MIT).
//! No original graph value is sent to the actor or reconstructed as typed args.

use super::*;

/// Prepared from source JSON.stringify while still on the original graph driver.
/// Private fields prevent replacing the exact request/owner association.
pub struct NativeMcpRequest {
    owner: Arc<Owner>,
    prepared: PreparedRequest,
}

#[derive(Debug)]
pub enum NativeMcpOutcome {
    Complete(Value),
    NotDispatched(String),
    UnknownEffect(String),
    Aborted { published: bool },
}

impl NativeMcpOutcome {
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::UnknownEffect(_) | Self::Aborted { published: true })
    }
}

impl McpTool {
    pub fn server_name(&self) -> &str {
        &self.server
    }
    pub fn original_tool_name(&self) -> &str {
        &self.original_name
    }

    /// Native-only reservation: the caller constructs the whole original RPC
    /// object and calls the original JSON.stringify. Allocation is observable
    /// before send, as in the fixed stdio.request. This is not dispatch.
    pub fn prepare_native_request<E>(
        &self,
        stringify: impl FnOnce(u64, &str) -> Result<Vec<u8>, E>,
    ) -> Result<NativeMcpRequest, E> {
        let id = self.owner.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = stringify(id, &self.original_name)?;
        Ok(NativeMcpRequest { owner: self.owner.clone(), prepared: PreparedRequest { id, frame } })
    }
}

impl NativeMcpRequest {
    /// Same gate, sender, connection actor and cleanup as the typed MCP path.
    /// Only native bytes, remote JSON and a CancellationToken cross this await.
    pub async fn dispatch(self, cancel: CancellationToken) -> NativeMcpOutcome {
        if self.prepared.frame.len() > FRAME_MAX || self.prepared.frame.last() != Some(&b'\n') {
            return NativeMcpOutcome::NotDispatched("MCP outbound frame is invalid or exceeds 1 MiB".into());
        }
        if cancel.is_cancelled() {
            return NativeMcpOutcome::Aborted { published: false };
        }
        let permit = tokio::select! {
            _ = cancel.cancelled() => return NativeMcpOutcome::Aborted { published: false },
            permit = self.owner.gate.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => return NativeMcpOutcome::NotDispatched("MCP server unavailable".into()),
            },
        };
        if !self.owner.alive.load(Ordering::Acquire) {
            return NativeMcpOutcome::NotDispatched("MCP server unavailable".into());
        }
        let (reply, received) = oneshot::channel();
        let slot = tokio::select! {
            _ = cancel.cancelled() => return NativeMcpOutcome::Aborted { published: false },
            slot = self.owner.sender.reserve() => match slot {
                Ok(slot) => slot,
                Err(_) => return NativeMcpOutcome::NotDispatched("MCP server unavailable".into()),
            },
        };
        if !self.owner.alive.load(Ordering::Acquire) {
            return NativeMcpOutcome::NotDispatched("MCP server unavailable".into());
        }
        slot.send(ActorCommand { prepared: self.prepared, reply });
        // A lost caller must not leave the original owner alive with an unknown
        // in-flight command. The actor also observes reply.closed().
        struct PublishedGuard {
            owner: Arc<Owner>,
            complete: bool,
        }
        impl Drop for PublishedGuard {
            fn drop(&mut self) {
                if !self.complete {
                    self.owner.stop();
                }
            }
        }
        let mut guard = PublishedGuard { owner: self.owner.clone(), complete: false };
        let outcome = tokio::select! {
            _ = cancel.cancelled() => NativeMcpOutcome::Aborted { published: true },
            received = tokio::time::timeout(CALL_TIMEOUT, received) => match received {
                Ok(Ok(Ok(value))) => match serde_json::to_vec(&value) {
                    Ok(bytes) if bytes.len() <= RESULT_MAX => NativeMcpOutcome::Complete(value),
                    _ => NativeMcpOutcome::UnknownEffect("MCP tool result exceeds 256 KiB".into()),
                },
                Ok(Ok(Err(error))) => NativeMcpOutcome::UnknownEffect(error),
                Ok(Err(_)) => NativeMcpOutcome::UnknownEffect("MCP server closed after dispatch".into()),
                Err(_) => NativeMcpOutcome::UnknownEffect("MCP tool timed out after dispatch".into()),
            },
        };
        guard.complete = matches!(outcome, NativeMcpOutcome::Complete(_));
        drop(permit);
        outcome
    }
}
