//! Native siblings for fixed OMP 596f2da Session recovery.
//! Original Live sources remain canonical; observations admit a transaction,
//! never construct a replacement assistant or serialize unrelated arguments.
use super::*;
use ara_rpc::WireValue;

#[derive(Debug)]
pub struct NativeFailedAssistantRecovery {
    transaction: FailedAssistantRecovery,
}

impl NativeFailedAssistantRecovery {
    pub fn local_history_rewritten(&self) -> bool {
        self.transaction.local_history_rewritten()
    }
    pub fn failed_entry_source(&self) -> AgentEntrySource {
        self.transaction.failed_entry_source()
    }
    pub fn failed_message_body(&self) -> Result<AgentMessageBody> {
        self.transaction.failed_message_body()
    }
    /// Existing local-reduction transactions use only the original branch owner.
    pub fn transaction_mut(&mut self) -> &mut FailedAssistantRecovery {
        &mut self.transaction
    }
}

#[derive(Debug, PartialEq)]
pub enum NativeFailedAssistantRecoveryOutcome {
    Committed,
    Restored { body: AgentMessageBody, source: AgentEntrySource, appended_entry_id: Option<String> },
}

fn field_text(source: &NativeMessageSource, path: &[&str], optional: bool) -> Result<Option<String>> {
    match source.observe_field(path)? {
        None | Some(WireValue::Null) if optional => Ok(None),
        Some(WireValue::String(ref value)) => value
            .to_utf8()
            .map(Some)
            .map_err(|_| failed_recovery_error("native recovery diagnostic String is not UTF-8")),
        _ => Err(failed_recovery_error("native recovery requires a current String field")),
    }
}

fn assistant_reason(source: &NativeMessageSource) -> Result<String> {
    if !matches!(source, NativeMessageSource::Live { .. }) {
        return Err(failed_recovery_error("native recovery requires the original Live source"));
    }
    if field_text(source, &["role"], false)?.as_deref() != Some("assistant") {
        return Err(failed_recovery_error("native recovery requires an assistant source"));
    }
    field_text(source, &["stopReason"], false)?
        .ok_or_else(|| failed_recovery_error("native recovery has no stop reason"))
}

fn no_tools(source: &NativeMessageSource) -> Result<()> {
    // The Host's narrow content.length observation requires a current Array.
    // A non-array content value must not look tool-free merely because the
    // existing tool-call observer has no calls to enumerate.
    content_length(source)?;
    if !source.observe_tool_calls()?.is_empty() {
        return Err(failed_recovery_error("native recovery requires a tool-free assistant"));
    }
    Ok(())
}

fn failed_reason(source: &NativeMessageSource) -> Result<()> {
    if !matches!(assistant_reason(source)?.as_str(), "error" | "length") {
        return Err(failed_recovery_error("native recovery requires Error or Length"));
    }
    no_tools(source)
}

fn content_length(source: &NativeMessageSource) -> Result<usize> {
    let Some(WireValue::Number(length)) = source.observe_field(&["content", "length"])? else {
        return Err(failed_recovery_error("native recovery content has no current array length"));
    };
    if !length.is_finite() || length < 0.0 || length.fract() != 0.0 || length > f64::from(u32::MAX) {
        return Err(failed_recovery_error("native recovery content has invalid array length"));
    }
    Ok(length as usize)
}

fn nonempty(source: &NativeMessageSource, path: &[&str], optional: bool) -> Result<bool> {
    match source.observe_field(path)? {
        None | Some(WireValue::Null) if optional => Ok(false),
        // JS whitespace is entirely in the BMP. Unpaired surrogates are
        // non-whitespace content and retain their original source units.
        Some(WireValue::String(ref text)) => Ok(text
            .units()
            .iter()
            .any(|unit| char::from_u32(u32::from(*unit)).is_none_or(|character| !ara_prompt::js::is_space(character)))),
        _ => Err(failed_recovery_error("native recovery content field is not a String")),
    }
}

enum ContentPredicate {
    EmptyError,
    EmptyStop,
    OrphanToolUse,
}

// Corresponding existing Session predicates have deliberately different block
// rules. Do not substitute a single text-only or replay-policy projection.
fn content_is_empty(source: &NativeMessageSource, predicate: ContentPredicate) -> Result<bool> {
    for at in 0..content_length(source)? {
        let at = at.to_string();
        let kind = field_text(source, &["content", &at, "type"], false)?
            .ok_or_else(|| failed_recovery_error("native recovery block has no type"))?;
        let contentful = match (&predicate, kind.as_str()) {
            (_, "text") => nonempty(source, &["content", &at, "text"], false)?,
            (ContentPredicate::EmptyError, "thinking") => {
                nonempty(source, &["content", &at, "thinking"], false)?
                    || nonempty(source, &["content", &at, "thinkingSignature"], true)?
            }
            (ContentPredicate::EmptyStop, "thinking") => {
                nonempty(source, &["content", &at, "thinkingSignature"], true)?
            }
            (ContentPredicate::EmptyError, "redactedThinking") => nonempty(source, &["content", &at, "data"], false)?,
            (ContentPredicate::OrphanToolUse, "toolCall") => true,
            (
                ContentPredicate::OrphanToolUse,
                "thinking" | "redactedThinking" | "image" | "audio" | "anthropicServerTool",
            ) => false,
            (_, "toolCall" | "redactedThinking" | "image" | "audio" | "anthropicServerTool") => true,
            _ => return Err(failed_recovery_error("native recovery block type is unsupported")),
        };
        if contentful {
            return Ok(false);
        }
    }
    Ok(true)
}

fn empty_terminal(source: &NativeMessageSource) -> Result<()> {
    let reason = assistant_reason(source)?;
    let predicate = match reason.as_str() {
        "stop" => ContentPredicate::EmptyStop,
        "toolUse" => ContentPredicate::OrphanToolUse,
        _ => return Err(failed_recovery_error("native terminal discard requires Stop or orphan ToolUse")),
    };
    if !content_is_empty(source, predicate)? {
        return Err(failed_recovery_error("native terminal has actionable content"));
    }
    no_tools(source)?;
    if assistant_reason(source)? != reason {
        return Err(failed_recovery_error("native terminal stop reason changed during observation"));
    }
    Ok(())
}

impl SessionJournal {
    // Identity compares both original root and encoder lease. The Host also
    // rechecks its Agent record, active Session and runtime around this call.
    fn native_recovery_entry(&self, entry_id: &str, source: &NativeMessageSource) -> Result<Entry> {
        if !matches!(source, NativeMessageSource::Live { .. }) {
            return Err(failed_recovery_error("native recovery source is not Live"));
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| failed_recovery_error("native recovery entry is missing"))?;
        if entry.kind != "message" || entry.native_message.as_ref() != Some(source) {
            return Err(failed_recovery_error("native recovery original source changed"));
        }
        Ok(entry.clone())
    }

    /// Same branch transaction as the legacy Gemini reminder, admitted through
    /// the current Live source without reconstructing an AssistantMessage.
    pub fn append_native_gemini_reminder_after_aborted(
        &mut self,
        notice: &LoopGuardNotice,
        aborted_entry_id: &str,
        source: &NativeMessageSource,
    ) -> Result<String> {
        if notice.custom_type() != GEMINI_TOOL_CALL_REMINDER_TYPE || self.leaf_id() != Some(aborted_entry_id) {
            return Err(failed_recovery_error("Gemini reminder requires the exact aborted native leaf"));
        }
        if assistant_reason(source)? != "aborted" {
            return Err(failed_recovery_error("Gemini reminder native assistant is not aborted"));
        }
        for at in 0..content_length(source)? {
            if field_text(source, &["content", &at.to_string(), "type"], false)?.as_deref() != Some("thinking") {
                return Err(failed_recovery_error("Gemini reminder native assistant is not thinking-only"));
            }
        }
        if assistant_reason(source)? != "aborted" {
            return Err(failed_recovery_error("Gemini reminder native stop reason changed during observation"));
        }
        let target = self.native_recovery_entry(aborted_entry_id, source)?;
        let previous_leaf = self.leaf.clone();
        self.leaf = target.parent_id;
        let result = self.append_loop_guard_notice(notice);
        if result.is_err() {
            self.leaf = previous_leaf;
        }
        result
    }

    pub fn begin_native_failed_assistant_recovery(
        &mut self,
        expected_leaf: &str,
        entry_id: &str,
        source: &NativeMessageSource,
    ) -> Result<NativeFailedAssistantRecovery> {
        if self.leaf_id() != Some(expected_leaf) {
            return Err(failed_recovery_error("native failed assistant branch changed"));
        }
        failed_reason(source)?;
        let failed_entry = self.native_recovery_entry(entry_id, source)?;
        let branch = self.strict_compaction_branch().map_err(|error| failed_recovery_error(error.to_string()))?;
        let index = branch
            .iter()
            .position(|entry| entry.id == entry_id)
            .ok_or_else(|| failed_recovery_error("native failed assistant is outside the current branch"))?;
        if branch[index + 1..].iter().any(|entry| !entry.is_service_tier_change()) {
            return Err(failed_recovery_error("native failed assistant metadata tail changed"));
        }
        let parent_branch = branch[..index].iter().map(|entry| (**entry).clone()).collect::<Vec<_>>();
        let parent_context = &branch[..index];
        let previous_compaction_id = parent_context[active_context_start(parent_context)..]
            .iter()
            .rev()
            .find(|entry| entry.kind == "compaction")
            .map(|entry| entry.id.clone());
        failed_reason(source)?;
        let failed_entry = self.native_recovery_entry(&failed_entry.id, source)?;
        let transaction = FailedAssistantRecovery {
            session_id: self.session_id().to_owned(),
            parent_branch,
            failed_entry,
            assistant: FailedAssistantRecoveryMessage::Native,
            previous_compaction_id,
            local_history_rewritten: false,
        };
        self.leaf = transaction.failed_entry.parent_id.clone();
        Ok(NativeFailedAssistantRecovery { transaction })
    }

    pub fn discard_native_entry_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        source: &NativeMessageSource,
    ) -> Result<Option<String>> {
        failed_reason(source)?;
        let target = self.native_recovery_entry(entry_id, source)?;
        self.discard_admitted_assistant_durably(expected_leaf, entry_id, target, None)
    }

    pub fn discard_native_empty_assistant_terminal_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        source: &NativeMessageSource,
    ) -> Result<Option<String>> {
        empty_terminal(source)?;
        let target = self.native_recovery_entry(entry_id, source)?;
        self.discard_admitted_assistant_durably(expected_leaf, entry_id, target, None)
    }

    pub fn discard_native_failed_assistant_for_promotion(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        source: &NativeMessageSource,
        target_model: &str,
    ) -> Result<Option<String>> {
        failed_reason(source)?;
        let target = self.native_recovery_entry(entry_id, source)?;
        self.discard_admitted_assistant_durably(expected_leaf, entry_id, target, Some(target_model))
    }

    pub fn finish_native_failed_assistant_recovery(
        &mut self,
        token: NativeFailedAssistantRecovery,
        history_rewritten: bool,
    ) -> Result<NativeFailedAssistantRecoveryOutcome> {
        if !matches!(token.transaction.assistant, FailedAssistantRecoveryMessage::Native) {
            return Err(failed_recovery_error("native recovery transaction mode changed"));
        }
        let source = token
            .transaction
            .failed_entry
            .native_message
            .as_ref()
            .ok_or_else(|| failed_recovery_error("native recovery lost its original source"))?;
        failed_reason(source)?;
        let empty_error =
            assistant_reason(source)? == "error" && content_is_empty(source, ContentPredicate::EmptyError)?;
        failed_reason(source)?;
        let Some(appended_entry_id) =
            self.finish_failed_assistant_recovery_transaction(&token.transaction, history_rewritten, empty_error)?
        else {
            return Ok(NativeFailedAssistantRecoveryOutcome::Committed);
        };
        Ok(NativeFailedAssistantRecoveryOutcome::Restored {
            body: token.failed_message_body()?,
            source: token.failed_entry_source(),
            appended_entry_id,
        })
    }

    /// The Host has assigned only retryRecovery in the original Graph and must
    /// invalidate derived views. A failed rewrite cannot roll back Native
    /// setters or Live slots by cloning Entries; it is a fail-stop receipt.
    pub fn persist_native_retry_recovery(&mut self, updates: &[(String, NativeMessageSource)]) -> Result<()> {
        let branch_ids: HashSet<_> = self.branch().iter().map(|entry| entry.id.clone()).collect();
        let mut seen = HashSet::new();
        for (id, source) in updates {
            if !seen.insert(id) || !branch_ids.contains(id) {
                return Err(failed_recovery_error("native retry entry is repeated or outside the active branch"));
            }
            self.native_recovery_entry(id, source)?;
            if !matches!(assistant_reason(source)?.as_str(), "error" | "aborted")
                || field_text(source, &["retryRecovery", "kind"], false)?.as_deref() != Some("auto-retry")
                || !matches!(
                    field_text(source, &["retryRecovery", "status"], false)?.as_deref(),
                    Some("recovered" | "superseded")
                )
            {
                return Err(failed_recovery_error("native retry metadata requires an actual failed assistant"));
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.rewrite() {
            self.rewrite_required = true;
            return Err(error);
        }
        Ok(())
    }
}
