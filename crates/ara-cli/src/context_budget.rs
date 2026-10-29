//! Host context accounting from fixed OMP 596f2da session-stats.ts.
//! Counts remain estimates where the selected native tokenizer is unavailable.
use ara_agent::{
    AgentMessage, AgentTool,
    tokenizer::{MessageCountOptions, TokenCountMode, Tokenizer, TokenizerPolicy, count_messages},
};
use ara_ai::{Message, Model};
use ara_rpc::WireValue;
use ara_session::{
    Entry, Result, SessionJournal, SessionReductionAction, SessionReductionEdit, SessionReductionSnapshot,
};
use serde_json::json;
use std::sync::Arc;

/// Capture Host policy once per counter; the reusable Core reads no environment.
pub fn tokenizer(model: &Model) -> Tokenizer {
    Tokenizer::with_policy(
        Some(model),
        TokenizerPolicy {
            test_environment: false,
            accurate_unknown: std::env::var("ARA_TOKENIZER_ACCURATE").is_ok_and(|value| value == "1"),
        },
    )
}

pub fn text_tokens(model: &Model, fragments: &[&str]) -> usize {
    tokenizer(model).count_fragments(fragments.iter().copied(), TokenCountMode::Approximate)
}

pub fn non_message_tokens(model: &Model, prompt: &[String], tools: &[Arc<dyn AgentTool>]) -> usize {
    let schemas: Vec<String> = tools
        .iter()
        .map(|tool| {
            let tool = tool.definition();
            serde_json::to_string(
                &json!({"name":tool.name,"description":tool.description,"parameters":tool.parameters}),
            )
            .unwrap_or_default()
        })
        .collect();
    let fragments: Vec<&str> = prompt.iter().chain(&schemas).map(String::as_str).collect();
    text_tokens(model, &fragments)
}

fn positive_number(value: Option<WireValue>) -> Option<usize> {
    value
        .as_ref()
        .and_then(WireValue::as_number)
        .filter(|number| number.is_finite() && *number >= 0.0)
        .map(|number| number.min(usize::MAX as f64) as usize)
}

/// A missing bucket remains unknown. Only observed positive prefix occupancy
/// can anchor the estimate; aborted/error responses never do so.
pub fn prompt_tokens(entry: &Entry) -> Result<Option<usize>> {
    if entry.kind != "message"
        || !entry
            .observed_message_field(&["role"])?
            .as_ref()
            .and_then(WireValue::as_string)
            .is_some_and(|role| role.equals_ascii("assistant"))
        || entry
            .observed_message_field(&["stopReason"])?
            .as_ref()
            .and_then(WireValue::as_string)
            .is_some_and(|reason| reason.equals_ascii("error") || reason.equals_ascii("aborted"))
    {
        return Ok(None);
    }
    if let Some(tokens) =
        positive_number(entry.observed_message_field(&["usage", "contextTokens"])?).filter(|tokens| *tokens > 0)
    {
        return Ok(Some(tokens));
    }
    let mut lower = 0usize;
    for key in ["input", "cacheRead", "cacheWrite"] {
        if let Some(tokens) = positive_number(entry.observed_message_field(&["usage", key])?) {
            lower = lower.saturating_add(tokens);
        }
    }
    if lower > 0 {
        return Ok(Some(lower));
    }
    let Some(total) = positive_number(entry.observed_message_field(&["usage", "totalTokens"])?) else {
        return Ok(None);
    };
    let Some(output) = positive_number(entry.observed_message_field(&["usage", "output"])?) else {
        return Ok(None);
    };
    Ok((total > output).then_some(total))
}

pub fn anchor(snapshot: &SessionReductionSnapshot) -> Result<Option<(usize, usize)>> {
    let boundary =
        snapshot.entries.iter().rposition(|entry| matches!(entry.kind.as_str(), "compaction" | "reset_boundary"));
    for (index, entry) in snapshot
        .entries
        .iter()
        .enumerate()
        .rev()
        .take_while(|(index, _)| boundary.is_none_or(|boundary| *index > boundary))
    {
        if let Some(tokens) = prompt_tokens(entry)? {
            return Ok(Some((index, tokens)));
        }
    }
    Ok(None)
}

pub fn anchor_edit(
    snapshot: &SessionReductionSnapshot,
    savings: &[(String, usize)],
    non_message: usize,
) -> Result<Option<SessionReductionEdit>> {
    let Some((anchor_index, prompt_tokens)) = anchor(snapshot)? else {
        return Ok(None);
    };
    let removed = snapshot.entries[..anchor_index].iter().fold(0usize, |sum, entry| {
        sum.saturating_add(
            savings.iter().filter(|(id, _)| id == &entry.id).fold(0usize, |n, (_, tokens)| n.saturating_add(*tokens)),
        )
    });
    if removed == 0 {
        return Ok(None);
    }
    Ok(Some(SessionReductionEdit {
        entry_id: snapshot.entries[anchor_index].id.clone(),
        action: SessionReductionAction::RecordAnchoredHistoryRewrite {
            tokens_removed: removed,
            snapshot_if_missing: Some(json!({"promptTokens":prompt_tokens,"nonMessageTokens":non_message,
            "compactionEpoch":snapshot.entries.iter().filter(|entry| entry.kind == "compaction").count()})),
        },
    }))
}

pub fn context_tokens(
    journal: &SessionJournal,
    messages: &[Message],
    pending: &[Message],
    non_message: usize,
) -> Result<usize> {
    context_tokens_with_counter(journal, messages, pending, non_message, count_messages)
}

pub fn context_tokens_for_model(
    model: &Model,
    journal: &SessionJournal,
    messages: &[Message],
    pending: &[Message],
    non_message: usize,
) -> Result<usize> {
    let tokenizer = tokenizer(model);
    context_tokens_with_counter(journal, messages, pending, non_message, |messages, options| {
        tokenizer.count_messages(messages, options)
    })
}

/// The actual Host counts checked current archive sources in their UTF16
/// domain. Provider occupancy still anchors the retained tail; no typed
/// transcript is constructed merely to locate that anchor.
pub fn context_tokens_for_current(
    model: &Model,
    journal: &SessionJournal,
    projector: &crate::agent_message_projection::AgentMessageProjector,
    messages: &[AgentMessage],
    pending: &[AgentMessage],
    non_message: usize,
) -> Result<usize> {
    context_tokens_with_observer(
        journal,
        messages,
        pending,
        non_message,
        |messages, options| {
            projector.count_current(messages, model, options).map_err(|error| std::io::Error::other(error).into())
        },
        |messages, entry| {
            projector.budget_anchor_position(messages, entry).map_err(|error| std::io::Error::other(error).into())
        },
    )
}

fn context_tokens_with_counter(
    journal: &SessionJournal,
    messages: &[Message],
    pending: &[Message],
    non_message: usize,
    count: impl Fn(&[Message], MessageCountOptions) -> usize,
) -> Result<usize> {
    context_tokens_with_observer(
        journal,
        messages,
        pending,
        non_message,
        |messages, options| Ok(count(messages, options)),
        |messages, entry| {
            Ok(entry.message()?.and_then(|projected| messages.iter().rposition(|message| message == &projected)))
        },
    )
}

/// Share the existing provider anchor arithmetic with callers that retain
/// exact message bodies. A failed current observation never becomes a zero
/// count or an omitted provider anchor.
pub fn context_tokens_with_observer<M>(
    journal: &SessionJournal,
    messages: &[M],
    pending: &[M],
    non_message: usize,
    count: impl Fn(&[M], MessageCountOptions) -> Result<usize>,
    anchor_position: impl Fn(&[M], &Entry) -> Result<Option<usize>>,
) -> Result<usize> {
    let pending_tokens = count(pending, MessageCountOptions { exclude_encrypted_reasoning: true })?;
    let stored = non_message
        .saturating_add(count(messages, MessageCountOptions { exclude_encrypted_reasoning: true })?)
        .saturating_add(pending_tokens);
    let snapshot = journal.raw_reduction_snapshot().map_err(std::io::Error::other)?;
    let Some((index, prompt)) = anchor(&snapshot)? else {
        return Ok(stored);
    };
    let entry = &snapshot.entries[index];
    let Some(position) = anchor_position(messages, entry)? else {
        return Ok(stored);
    };
    let tail = position.checked_add(1).and_then(|start| messages.get(start..)).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Current budget anchor is outside the transcript")
    })?;
    // Missing snapshot fields retain the existing estimate defaults. Failed
    // current observations return before these Option defaults are considered.
    let base = positive_number(entry.observed_message_field(&["contextSnapshot", "promptTokens"])?).unwrap_or(prompt);
    let removed = positive_number(entry.observed_message_field(&["contextSnapshot", "historyRewriteTokensRemoved"])?)
        .unwrap_or(0);
    let previous_non_message =
        positive_number(entry.observed_message_field(&["contextSnapshot", "nonMessageTokens"])?).unwrap_or(non_message);
    let provider = base
        .saturating_sub(removed)
        .saturating_add(non_message.saturating_sub(previous_non_message))
        .saturating_add(count(tail, MessageCountOptions::default())?)
        .saturating_add(pending_tokens);
    Ok(stored.max(provider))
}
