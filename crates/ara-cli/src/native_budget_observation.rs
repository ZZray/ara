//! Narrow Host budget observations and original-domain replay comparisons.
//! Fixed session/messages.ts:135-314, OMP 596f2da (MIT; THIRD_PARTY_NOTICES.md).
//! None of these derived values becomes a canonical AgentMessage body.
use super::*;
use crate::actual_host::HostGraph;
use ara_agent::SessionValues;
use ara_rpc::{
    WireString,
    value_heap::{NodeId, PropertyKey, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};

fn pick(
    graph: &HostGraph<'_>,
    root: &ValueHandle,
    fields: &[&str],
    omit_undefined: bool,
) -> Result<ValueHandle, ValueError> {
    let output = graph.0.heap().object()?;
    for field in fields {
        let value = graph.get(root, field)?;
        if !omit_undefined || !matches!(graph.0.heap().snapshot(&value)?, ValueSnapshot::Undefined) {
            graph.put(&output, field, value)?;
        }
    }
    Ok(output)
}

/// Explicit, disposable observation for the existing typed budget estimator.
/// Only optional Undefined fields are omitted. Native Number/UTF16 are retained
/// until the checked scalar boundary; unsupported domains still fail explicitly.
pub(super) fn budget_body(root: &ValueHandle, values: &SessionValues) -> Result<Value, ValueError> {
    values.check_handle(root)?;
    let graph = HostGraph(values);
    let role = graph.string(&graph.get(root, "role")?)?;
    let fields: &[&str] = if role.equals_ascii("assistant") {
        &[
            "role",
            "content",
            "timestamp",
            "api",
            "provider",
            "model",
            "usage",
            "stopReason",
            "providerPayload",
            "responseId",
            "upstreamProvider",
            "retryAccounting",
            "retryRecovery",
            "stopDetails",
            "errorMessage",
            "errorStatus",
            "failureEvidence",
            "terminalContextRecovery",
            "duration",
            "ttft",
        ]
    } else if role.equals_ascii("user") {
        &["role", "content", "timestamp", "providerPayload", "synthetic"]
    } else if role.equals_ascii("developer") {
        &["role", "content", "timestamp", "providerPayload"]
    } else if role.equals_ascii("toolResult") {
        &["role", "content", "timestamp", "toolCallId", "toolName", "isError", "providerMetadata", "prunedAt"]
    } else if role.equals_ascii("custom") || role.equals_ascii("hookMessage") {
        &["role", "content", "timestamp", "customType", "attribution", "display", "details"]
    } else if role.equals_ascii("branchSummary") {
        &["role", "summary", "timestamp", "fromId"]
    } else if role.equals_ascii("compactionSummary") {
        &[
            "role",
            "summary",
            "timestamp",
            "tokensBefore",
            "providerPayload",
            "method",
            "blocks",
            "images",
            "preserveData",
        ]
    } else if role.equals_ascii("bashExecution") {
        &["role", "command", "output", "timestamp", "exitCode", "cancelled", "truncated", "excludeFromContext", "meta"]
    } else if role.equals_ascii("pythonExecution") {
        &["role", "code", "output", "timestamp", "exitCode", "cancelled", "truncated", "excludeFromContext", "meta"]
    } else if role.equals_ascii("fileMention") {
        &["role", "files", "timestamp"]
    } else {
        &["role"]
    };
    let body = pick(&graph, root, fields, true)?;
    let content = graph.get(&body, "content")?;
    if values.heap().kind(&content)? == ValueKind::Array {
        let mut blocks = Vec::new();
        for block in graph.members(&content)? {
            let kind = graph.string(&graph.get(&block, "type")?)?;
            let fields: &[&str] = if kind.equals_ascii("text") {
                &["type", "text", "textSignature"]
            } else if kind.equals_ascii("thinking") {
                &["type", "thinking", "thinkingSignature"]
            } else if kind.equals_ascii("redactedThinking") {
                &["type", "data"]
            } else if kind.equals_ascii("toolCall") {
                &["type", "id", "name", "arguments", "intent", "thoughtSignature", "customWireName", "providerMetadata"]
            } else if kind.equals_ascii("image") {
                &["type", "data", "mimeType", "detail", "url"]
            } else {
                // Reserved malformed/unknown blocks remain a checked failure;
                // do not drop them or manufacture a zero-token history.
                return Err(ValueError::UnsupportedOperation(
                    "Budget content block type is not supported by the existing estimator",
                ));
            };
            blocks.push(pick(&graph, &block, fields, true)?);
        }
        graph.put(&body, "content", graph.array(blocks)?)?;
    }
    // export_serde does not invoke toJSON. The original root, block objects,
    // payload and argument handles are never rewritten by this observation.
    values.heap().export_serde(&body)
}

/// Compaction uses the same known model fields/blocks as the budget DTO.
/// Tool-result details and useless are additionally consumed by its existing
/// typed Message view. Keep those original handles under a strict bridge;
/// only absent optional top-level Undefined slots are omitted by pick.
pub(super) fn compaction_body(root: &ValueHandle, values: &SessionValues) -> Result<Value, ValueError> {
    let mut observed = budget_body(root, values)?;
    if observed.get("role").and_then(Value::as_str) == Some("toolResult") {
        let additions = pick(&HostGraph(values), root, &["details", "useless"], true)?;
        let additions = values.heap().export_serde(&additions)?;
        let Value::Object(additions) = additions else {
            return Err(ValueError::UnsupportedOperation("Compaction tool-result additions are not an object"));
        };
        let Value::Object(fields) = &mut observed else {
            return Err(ValueError::UnsupportedOperation("Compaction model observation is not an object"));
        };
        fields.extend(additions);
    }
    Ok(observed)
}

/// Object.entries/fromEntries normalization in the original value domain.
/// An iterative traversal retains Number, UTF16, Undefined and Function edges,
/// strips object prototypes/Symbol keys and preserves array holes. Circular
/// normalization is an explicit RangeError rather than Rust stack overflow.
fn normalize(graph: &HostGraph<'_>, root: &ValueHandle) -> Result<ValueHandle, ValueError> {
    enum Work {
        Enter { source: ValueHandle, slot: Option<(ValueHandle, PropertyKey)> },
        Array { source: ValueHandle, output: ValueHandle, index: u32, length: u32 },
        Object { output: ValueHandle, entries: Vec<(PropertyKey, ValueHandle)>, index: usize },
        Leave(NodeId),
    }
    let heap = graph.0.heap();
    let runtime = graph.0.runtime();
    let mut work = vec![Work::Enter { source: root.clone(), slot: None }];
    let mut active = Vec::new();
    let mut result = None;
    while let Some(frame) = work.pop() {
        match frame {
            Work::Enter { source, slot } => {
                let kind = heap.kind(&source)?;
                let value = if matches!(kind, ValueKind::Object | ValueKind::Array) {
                    if active.contains(&source.node_id()) {
                        return Err(ValueError::RangeError("Circular provider replay normalization".into()));
                    }
                    active.push(source.node_id());
                    work.push(Work::Leave(source.node_id()));
                    if kind == ValueKind::Array {
                        let length = heap.array_length(&source, runtime)?;
                        let output = heap.array_with_length(length)?;
                        work.push(Work::Array { source, output: output.clone(), index: 0, length });
                        output
                    } else {
                        let mut entries = Vec::new();
                        for key in heap.own_enumerable_keys(&source, runtime)? {
                            if matches!(&key, PropertyKey::String(_)) {
                                let value = heap.get(&source, &key, runtime)?;
                                entries.push((key, value));
                            }
                        }
                        let output = heap.object()?;
                        work.push(Work::Object { output: output.clone(), entries, index: 0 });
                        output
                    }
                } else {
                    source
                };
                match slot {
                    Some((parent, key)) => heap.define_own_data(&parent, key, value, runtime)?,
                    None => result = Some(value),
                }
            }
            Work::Array { source, output, mut index, length } => {
                while index < length {
                    let key = PropertyKey::from(WireString::from(index.to_string()));
                    index += 1;
                    if heap.has(&source, &key, runtime)? {
                        let child = heap.get(&source, &key, runtime)?;
                        work.push(Work::Array { source, output: output.clone(), index, length });
                        work.push(Work::Enter { source: child, slot: Some((output, key)) });
                        break;
                    }
                }
            }
            Work::Object { output, entries, index } => {
                if let Some((key, source)) = entries.get(index).cloned() {
                    work.push(Work::Object { output: output.clone(), entries, index: index + 1 });
                    work.push(Work::Enter { source, slot: Some((output, key)) });
                }
            }
            Work::Leave(node) => {
                let popped = active.pop();
                debug_assert_eq!(popped, Some(node));
            }
        }
    }
    result.ok_or(ValueError::UnsupportedOperation("Provider replay normalization produced no value"))
}

fn replay_value(graph: &HostGraph<'_>, root: &ValueHandle) -> Result<ValueHandle, ValueError> {
    let role = graph.string(&graph.get(root, "role")?)?;
    let fields: &[&str] = if role.equals_ascii("user") || role.equals_ascii("developer") {
        &["role", "content", "providerPayload"]
    } else if role.equals_ascii("assistant") {
        &["role", "content", "api", "provider", "model", "stopReason", "errorMessage"]
    } else if role.equals_ascii("toolResult") {
        &["role", "toolName", "toolCallId", "isError", "content"]
    } else if role.equals_ascii("bashExecution") {
        &["role", "command", "output", "exitCode", "cancelled", "meta", "excludeFromContext"]
    } else if role.equals_ascii("pythonExecution") {
        &["role", "code", "output", "exitCode", "cancelled", "meta", "excludeFromContext"]
    } else if role.equals_ascii("custom") || role.equals_ascii("hookMessage") {
        &["role", "customType", "content"]
    } else if role.equals_ascii("branchSummary") {
        &["role", "summary"]
    } else if role.equals_ascii("compactionSummary") {
        &["role", "summary", "providerPayload"]
    } else if role.equals_ascii("fileMention") {
        &["role", "files"]
    } else {
        return normalize(graph, root);
    };
    let output = pick(graph, root, fields, false)?;
    if role.equals_ascii("assistant") {
        let api = graph.get(root, "api")?;
        let responses = matches!(graph.0.heap().snapshot(&api)?, ValueSnapshot::String(api)
            if api.equals_ascii("openai-responses") || api.equals_ascii("openai-codex-responses"));
        let content = graph.get(root, "content")?;
        if responses {
            graph.put(&output, "providerPayload", graph.0.heap().undefined()?)?;
            if graph.0.heap().kind(&content)? == ValueKind::Array {
                let mut blocks = Vec::new();
                for block in graph.members(&content)? {
                    if graph.is(&block, "type", "thinking")? {
                        continue;
                    }
                    blocks.push(if graph.is(&block, "type", "toolCall")? {
                        pick(graph, &block, &["type", "id", "name", "arguments"], false)?
                    } else if graph.is(&block, "type", "text")? {
                        pick(graph, &block, &["type", "text", "textSignature"], false)?
                    } else {
                        normalize(graph, &block)?
                    });
                }
                graph.put(&output, "content", graph.array(blocks)?)?;
            } else {
                graph.put(&output, "content", normalize(graph, &content)?)?;
            }
        } else {
            graph.put(&output, "content", normalize(graph, &content)?)?;
            graph.put(&output, "providerPayload", graph.get(root, "providerPayload")?)?;
        }
    } else if role.equals_ascii("user")
        || role.equals_ascii("developer")
        || role.equals_ascii("toolResult")
        || role.equals_ascii("custom")
        || role.equals_ascii("hookMessage")
    {
        graph.put(&output, "content", normalize(graph, &graph.get(&output, "content")?)?)?;
    }
    if role.equals_ascii("bashExecution") || role.equals_ascii("pythonExecution") {
        let meta = graph.get(root, "meta")?;
        if graph.0.heap().truthy(&meta)? {
            let projected = pick(graph, &meta, &["truncation", "limits", "diagnostics"], false)?;
            for field in ["truncation", "limits"] {
                graph.put(&projected, field, normalize(graph, &graph.get(&projected, field)?)?)?;
            }
            let diagnostics = graph.get(&meta, "diagnostics")?;
            if graph.0.heap().truthy(&diagnostics)? {
                let diagnostics = pick(graph, &diagnostics, &["summary", "messages"], false)?;
                graph.put(&projected, "diagnostics", normalize(graph, &diagnostics)?)?;
            } else {
                graph.put(&projected, "diagnostics", graph.0.heap().undefined()?)?;
            }
            graph.put(&output, "meta", projected)?;
        } else {
            graph.put(&output, "meta", graph.0.heap().undefined()?)?;
        }
    }
    if role.equals_ascii("fileMention") {
        let mut files = Vec::new();
        for file in graph.members(&graph.get(root, "files")?)? {
            files.push(pick(graph, &file, &["path", "content", "image"], false)?);
        }
        graph.put(&output, "files", graph.array(files)?)?;
    }
    Ok(output)
}

pub(super) fn replay_equal(
    left: &ValueHandle,
    right: &ValueHandle,
    values: &SessionValues,
) -> Result<bool, ValueError> {
    values.check_handle(left)?;
    values.check_handle(right)?;
    let graph = HostGraph(values);
    let left = replay_value(&graph, left)?;
    let right = replay_value(&graph, right)?;
    values.heap().mcp_deep_equals(&left, &right, values.runtime())
}
