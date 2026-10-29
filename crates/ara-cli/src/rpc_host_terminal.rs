//! CURRENT terminal consumers over canonical records, fixed OMP596f2da.
//! MIT; THIRD_PARTY_NOTICES.md. Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
use super::super::rpc_host_retry::{
    NativeRetryClassifierFunctions, NativeRetryClassifierPort, NativeRetryHintFunctions, RetryDisposition,
    disposition_actual, effective_max_retries_actual, replay_disposition_actual, same_route_class_actual,
    session_retry_hint_actual,
};
use super::*;
use ara_ai::{
    StopReason,
    retry_classification::{RetryClass, flag},
};
use ara_rpc::value_heap::{ValueHandle, ValueSnapshot};
use ara_session::{
    FailedAssistantRecovery, FailedAssistantRecoveryOutcome, NativeFailedAssistantRecovery,
    NativeFailedAssistantRecoveryOutcome, NativeMessageSource,
};

/// Prepared once with the logical Session, never a callback runtime snapshot.
pub(super) struct NativeTerminalImports {
    classifier: NativeRetryClassifierFunctions,
    hint: NativeRetryHintFunctions,
    _owner: ara_ai::native_error::NativeErrorFunctionOwner,
}
impl NativeTerminalImports {
    pub fn prepare(owner: &ara_cli::native_factory::PreparedNativeSession) -> Result<Self> {
        use ara_ai::{
            native_error::{NativeErrorFunctionOwner, NativeErrorImports},
            retry::native_retry_hint::NativeRetryHintModule,
        };
        let values = owner.values();
        ara_cli::retry_host::prepare_hint_dependencies(owner.realm())?;
        let hints = NativeRetryHintModule::prepare(values.factory(), owner.realm().globals.environment.clone())?;
        let errors = NativeErrorFunctionOwner::prepare(
            values.factory(),
            NativeErrorImports {
                globals: owner.realm().globals.environment.clone(),
                extract_retry_hint: Some(hints.extract_retry_hint.clone()),
            },
        )?;
        let functions = errors.functions(values.factory())?;
        Ok(Self {
            classifier: NativeRetryClassifierFunctions {
                receiver: functions.receiver,
                classify_message: functions.classify_message,
                is_context_overflow: functions.is_context_overflow,
            },
            hint: NativeRetryHintFunctions {
                date_receiver: owner.realm().globals.date.clone(),
                extract_retry_hint: hints.extract_retry_hint,
            },
            _owner: errors,
        })
    }
}

#[derive(Clone)]
pub(super) enum RecoveryTerminal {
    Legacy(AssistantMessage),
    Native {
        message: AgentMessage,
        root: ValueHandle,
        session: Arc<Session>,
        generation: u64,
        source_model: ara_ai::Model,
    },
}
#[derive(Clone, Copy)]
pub(super) enum FactsPurpose {
    Empty,
    Unexpected,
    Overflow,
    Served,
}
pub(super) struct TerminalFacts {
    pub stop_reason: StopReason,
    pub current_route: bool,
    pub timestamp: f64,
    pub has_tools: bool,
    pub has_text: bool,
    pub signed_thinking: bool,
    pub empty_terminal: bool,
    pub provider_empty_content: bool,
    pub produced_output: bool,
    pub safe_success: bool,
}
impl RecoveryTerminal {
    pub fn native(message: AgentMessage, host: &Host) -> Result<Self> {
        let ara_agent::AgentMessageBody::Graph(root) = message.body_snapshot() else {
            bail!("native terminal is not Graph")
        };
        let values = host.session.values.as_ref().context("Native terminal Session values unavailable")?;
        values.check_handle(&root)?;
        message.recheck_graph_root(&root)?;
        Ok(Self::Native {
            message,
            root,
            session: host.session.clone(),
            generation: host.prompt_generation,
            source_model: host.config.model.clone(),
        })
    }
    pub fn legacy(&self) -> Option<&AssistantMessage> {
        match self {
            Self::Legacy(message) => Some(message),
            _ => None,
        }
    }
    pub fn actual(&self) -> Option<&AgentMessage> {
        match self {
            Self::Native { message, .. } => Some(message),
            _ => None,
        }
    }
    fn binding(&self) -> Result<(&AgentMessage, &ValueHandle, &SessionValues, &NativeTerminalImports)> {
        let Self::Native { message, root, session, .. } = self else { bail!("legacy terminal has no Native binding") };
        let values = session.values.as_deref().context("Native terminal values missing")?;
        let imports = session.terminal_imports.as_ref().context("Native terminal imports missing")?;
        values.check_handle(root)?;
        message.recheck_graph_root(root)?;
        Ok((message, root, values, imports))
    }
    /// Native errors remain primary even if a getter also changed the root.
    fn read<T>(
        &self,
        action: impl FnOnce(&ValueHandle, &SessionValues, &NativeTerminalImports) -> Result<T>,
    ) -> Result<T> {
        let (message, root, values, imports) = self.binding()?;
        let result = action(root, values, imports);
        let admission = message.recheck_graph_root(root);
        match (result, admission) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(primary), Err(secondary)) => {
                let primary = if let Some(error) = primary.downcast_ref::<ara_agent::ActualProviderError>() {
                    error.clone()
                } else if let Some(error) = primary.downcast_ref::<ara_rpc::value_heap::ValueError>() {
                    error.clone().into()
                } else {
                    ara_agent::ActualProviderError::Callback(primary.to_string())
                };
                Err(ara_agent::ActualProviderError::Compound {
                    primary: Arc::new(primary),
                    secondary: Arc::new(secondary.into()),
                }
                .into())
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }
    pub fn check_host(&self, host: &Host) -> Result<()> {
        if let Self::Native { session, generation, .. } = self {
            if !Arc::ptr_eq(session, &host.session) || *generation != host.prompt_generation {
                bail!("Native terminal Session generation changed")
            }
            self.binding()?;
        }
        Ok(())
    }
    pub fn exact_tail(&self, messages: &[AgentMessage]) -> Result<bool> {
        match self {
            Self::Legacy(message) => Ok(raw_last_assistant(messages)?.as_ref() == Some(message)),
            Self::Native { message, root, .. } => {
                self.binding()?;
                let Some(tail) = messages.last().filter(|tail| tail.same_record(message)) else {
                    return Ok(false);
                };
                tail.recheck_graph_root(root)?;
                Ok(true)
            }
        }
    }
    pub fn persisted_id(&self, active: &ActiveRun) -> Result<String> {
        match self {
            Self::Legacy(message) => active
                .sink
                .entries
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find_map(|(id, candidate)| (candidate.as_assistant() == Some(message)).then(|| id.clone()))
                .context("recovery assistant has no persisted entry ID"),
            Self::Native { message, session, .. } => {
                if !Arc::ptr_eq(session, &active.sink.session) {
                    bail!("Native terminal Run Session changed")
                }
                self.binding()?;
                session.projector.entry_id(message).context("Native terminal has no persisted entry ID")
            }
        }
    }
    pub async fn quota_replay_admission(&self, host: &Host, active: &ActiveRun) -> Result<bool> {
        self.check_host(host)?;
        let Some(mut class) = self.retry_class(&host.config.model)? else { return Ok(false) };
        if !class.usage_limit || !self.usage_admission()? {
            return Ok(false);
        }
        // Only independent effect safety is supplied here. Private quota proof
        // matching and durable feedback remain owned by session_quota_outcome_actual.
        class.replay_blocked = false;
        let messages = host.agent.raw_messages().await;
        let id = self.persisted_id(active)?;
        let journal = host.session.journal.lock().await;
        self.source(&journal, &id)?;
        self.check_host(host)?;
        let Some(mut current_class) = self.retry_class(&host.config.model)? else { return Ok(false) };
        if !current_class.usage_limit || !self.usage_admission()? {
            return Ok(false);
        }
        current_class.replay_blocked = false;
        Ok(self.retry_dispositions(&messages, &current_class)?.is_some())
    }
    pub fn source(&self, journal: &SessionJournal, entry_id: &str) -> Result<NativeMessageSource> {
        let Self::Native { root, .. } = self else { bail!("legacy terminal has no Native source") };
        self.binding()?;
        let source = journal
            .entries()
            .iter()
            .find(|entry| entry.id == entry_id)
            .and_then(|entry| entry.native_message.as_ref())
            .context("Native terminal durable source missing")?;
        if !source.live_body().is_some_and(|body| body.same_identity(root)) {
            bail!("Native terminal durable source changed")
        }
        Ok(source.clone())
    }
    pub fn facts(&self, model: &ara_ai::Model) -> Result<TerminalFacts> {
        self.facts_for(model, FactsPurpose::Served)
    }
    pub fn facts_for(&self, model: &ara_ai::Model, purpose: FactsPurpose) -> Result<TerminalFacts> {
        if let Self::Legacy(message) = self {
            let has_text = message.content.iter().any(|block| matches!(block, ara_ai::AssistantBlock::Text(text) if !ara_prompt::js::trim(&text.text).is_empty()));
            let signed_thinking = message.content.iter().any(|block| matches!(block, ara_ai::AssistantBlock::Thinking(thinking)
                if !ara_prompt::js::trim(&thinking.thinking).is_empty() && thinking.thinking_signature.as_deref().is_some_and(|signature| !ara_prompt::js::trim(signature).is_empty())));
            return Ok(TerminalFacts {
                stop_reason: message.stop_reason,
                current_route: message.provider == model.provider && message.model == model.id,
                timestamp: message.timestamp as f64,
                has_tools: message.tool_calls().next().is_some(),
                has_text,
                signed_thinking,
                empty_terminal: ara_session::is_empty_assistant_terminal(message),
                provider_empty_content: message.content.iter().all(|block| {
                    matches!(block, ara_ai::AssistantBlock::Thinking(_))
                        || matches!(block, ara_ai::AssistantBlock::Text(text) if text.text.trim().is_empty())
                }),
                produced_output: ara_session::assistant_turn_produced_output(message),
                safe_success: !matches!(model.api.as_str(), "openai-responses" | "openai-codex-responses")
                    || message.terminal_context_recovery == Some(ara_ai::ContextRecoveryEvidence::ContentOnly),
            });
        }
        self.read(|root, values, _| {
            let is = |key, expected| -> Result<bool> { Ok(matches!(values.heap().snapshot(&get(values, root, key)?)?, ValueSnapshot::String(text) if text.equals_ascii(expected))) };
            if !is("role", "assistant")? { bail!("Native terminal CURRENT role is not assistant") }
            let stop_reason = match string(values, &get(values, root, "stopReason")?)?.units() {
                units if units == ara_rpc::WireString::from("stop").units() => StopReason::Stop,
                units if units == ara_rpc::WireString::from("length").units() => StopReason::Length,
                units if units == ara_rpc::WireString::from("toolUse").units() => StopReason::ToolUse,
                units if units == ara_rpc::WireString::from("error").units() => StopReason::Error,
                units if units == ara_rpc::WireString::from("aborted").units() => StopReason::Aborted,
                _ => bail!("Native terminal stopReason is unsupported"),
            };
            let current_route = string(values, &get(values, root, "provider")?)? == ara_rpc::WireString::from(model.provider.as_str())
                && string(values, &get(values, root, "model")?)? == ara_rpc::WireString::from(model.id.as_str());
            let timestamp = match values.heap().snapshot(&get(values, root, "timestamp")?)? { ValueSnapshot::Number(value) => value, _ => bail!("Native terminal timestamp is not Number") };
            let content = get(values, root, "content")?;
            if !matches!(values.heap().snapshot(&content)?, ValueSnapshot::Array) { bail!("Native terminal content is not Array") }
            let length = values.heap().array_length(&content, values.runtime())?;
            let mut has_text = false;
            let mut has_tools = false;
            let mut signed_thinking = false;
            let mut actionable = false;
            let mut provider_empty_content = true;
            for index in 0..length {
                let block = get(values, &content, &index.to_string())?;
                let kind = string(values, &get(values, &block, "type")?)?;
                if kind.equals_ascii("text") {
                    let nonblank = nonblank(values, &get(values, &block, "text")?)?;
                    has_text |= nonblank;
                    actionable |= nonblank;
                    provider_empty_content &= !nonblank;
                } else if kind.equals_ascii("thinking") {
                    // Error/Aborted empty-response evidence consumes only
                    // type/text. Signature/thinking getters cannot veto it.
                    if !matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                        if matches!(purpose, FactsPurpose::Unexpected) {
                            // Fixed signed-thinking predicate: thinking before
                            // signature, with the original short circuit.
                            if nonblank(values, &get(values, &block, "thinking")?)? {
                                let signature = get(values, &block, "thinkingSignature")?;
                                signed_thinking |= !nullish(values, &signature)? && nonblank(values, &signature)?;
                            }
                        } else {
                            let signature = get(values, &block, "thinkingSignature")?;
                            actionable |= !nullish(values, &signature)? && nonblank(values, &signature)?;
                        }
                    }
                } else {
                    actionable = true;
                    provider_empty_content = false;
                    has_tools |= kind.equals_ascii("toolCall");
                }
            }
            let empty_terminal = match stop_reason { StopReason::Stop => !actionable,
                StopReason::ToolUse => !has_tools && !has_text, _ => false };
            let safe_success = !matches!(model.api.as_str(), "openai-responses" | "openai-codex-responses") || is("terminalContextRecovery", "contentOnly")?;
            Ok(TerminalFacts { stop_reason, current_route, timestamp, has_tools, has_text, signed_thinking,
                empty_terminal, provider_empty_content, produced_output: !matches!(stop_reason, StopReason::Error | StopReason::Aborted) && !empty_terminal && actionable,
                safe_success })
        })
    }
    pub fn class(&self, model: &ara_ai::Model) -> Result<RetryClass> {
        match self {
            Self::Legacy(message) => Ok(ara_ai::retry_classification::classify_retry(message, &model.api)),
            Self::Native { message, .. } => self.read(|root, values, imports| {
                if let Some(metadata) = same_route_class_actual(model, message, values, &imports.classifier)? { return Ok(metadata.class) }
                // Successful/Length/ToolUse maintenance may inspect existing
                // flags but must not classify and Set errorId on the original.
                let stop = get(values, root, "stopReason")?;
                let nonerror = matches!(values.heap().snapshot(&stop)?, ValueSnapshot::String(text)
                    if text.equals_ascii("stop") || text.equals_ascii("length") || text.equals_ascii("toolUse"));
                let id = if nonerror { get(values, root, "errorId")? }
                    else { imports.classifier.classify_message(root, values)? };
                let id = values.heap().to_number(&id, values.runtime())?;
                let id = if id.is_finite() { id.trunc().rem_euclid(4_294_967_296.0) as u32 } else { 0 };
                let mut class = RetryClass { error_id: id, overflow: imports.classifier.is_context_overflow(root, model.context_window.unwrap_or(0.0), values)?, usage_limit: id & flag::USAGE_LIMIT != 0, ..Default::default() };
                let evidence = get(values, root, "failureEvidence")?;
                if !nullish(values, &evidence)? {
                    let context = get(values, &evidence, "contextRecovery")?;
                    class.context_recovery_blocked = !nullish(values, &context)? && !matches!(values.heap().snapshot(&context)?, ValueSnapshot::String(text) if text.equals_ascii("contentOnly") || text.equals_ascii("usageAdmission"));
                }
                class.context_recovery_blocked |= class.usage_limit;
                Ok(class)
            }),
        }
    }
    pub fn usage_admission(&self) -> Result<bool> {
        match self { Self::Legacy(message) => Ok(message.failure_evidence.as_ref().is_some_and(|e| e.context_recovery == Some(ara_ai::ContextRecoveryEvidence::UsageAdmission))),
            _ => self.read(|root, values, _| { let evidence = get(values, root, "failureEvidence")?;
                if nullish(values, &evidence)? { return Ok(false) }
                Ok(matches!(values.heap().snapshot(&get(values, &evidence, "contextRecovery")?)?, ValueSnapshot::String(text) if text.equals_ascii("usageAdmission"))) }) }
    }
    pub fn retry_dispositions(
        &self,
        messages: &[AgentMessage],
        class: &RetryClass,
    ) -> Result<Option<(bool, RetryDisposition)>> {
        match self {
            Self::Legacy(message) => {
                let Some(tail) = raw_retry_tail(messages, message)? else {
                    return Ok(None);
                };
                Ok(super::super::rpc_host_retry::replay_disposition(message, &tail, class)
                    .map(|replay| (super::super::rpc_host_retry::disposition(message, &tail, class).is_some(), replay)))
            }
            Self::Native { message, .. } => self
                .read(|_, values, _| {
                    Ok(replay_disposition_actual(message, messages, class, values, true)?.map(|replay| (false, replay)))
                })
                .and_then(|result| match result {
                    None => Ok(None),
                    Some((_, replay)) => self.read(|_, values, _| {
                        Ok(Some((disposition_actual(message, messages, class, values, true)?.is_some(), replay)))
                    }),
                }),
        }
    }
    pub fn max_retries(&self, configured: f64) -> Result<f64> {
        match self {
            Self::Legacy(message) => Ok(super::super::rpc_host_retry::effective_max_retries(message, configured)),
            Self::Native { message, .. } => {
                self.read(|_, values, _| Ok(effective_max_retries_actual(message, configured, values)?))
            }
        }
    }
    pub fn retry_hint(&self, now: f64) -> Result<Option<f64>> {
        match self {
            Self::Legacy(message) => Ok(super::super::rpc_host_retry::session_retry_hint(message, now)),
            Self::Native { message, .. } => {
                self.read(|_, values, imports| Ok(session_retry_hint_actual(message, now, values, &imports.hint)?))
            }
        }
    }
    pub fn error_message(&self) -> Result<Option<String>> {
        match self {
            Self::Legacy(message) => Ok(message.error_message.clone()),
            _ => self.read(|root, values, _| {
                let error = get(values, root, "errorMessage")?;
                if nullish(values, &error)? {
                    return Ok(None);
                }
                Ok(Some(
                    values
                        .heap()
                        .to_string(&error, values.runtime())?
                        .to_utf8()
                        .context("Native terminal error is not scalar display text")?,
                ))
            }),
        }
    }
    pub fn fallback_cooldown(&self, hint: Option<f64>, now: f64, random: f64) -> Result<f64> {
        match self {
            Self::Legacy(message) => {
                Ok(ara_ai::fallback_cooldown_ms(message.error_message.as_deref().unwrap_or(""), hint, now, random))
            }
            _ => self.read(|root, values, imports| {
                let error = get(values, root, "errorMessage")?;
                let text = if nullish(values, &error)? { values.heap().string("".into())? } else { error };
                Ok(imports._owner.fallback_cooldown_ms(values.factory(), &text, hint, random, values.runtime())?)
            }),
        }
    }
    pub fn input_usage(&self) -> Result<(u64, Option<u64>)> {
        match self {
            Self::Legacy(message) => {
                let buckets = [message.usage.input, message.usage.cache_read, message.usage.cache_write];
                let lower = buckets.into_iter().flatten().fold(0u64, u64::saturating_add);
                Ok((lower, buckets.iter().all(Option::is_some).then_some(lower)))
            }
            _ => self.read(|root, values, _| {
                let usage = get(values, root, "usage")?;
                let mut buckets = Vec::with_capacity(3);
                for key in ["input", "cacheRead", "cacheWrite"] {
                    buckets.push(match values.heap().snapshot(&get(values, &usage, key)?)? {
                        ValueSnapshot::Number(value)
                            if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value < u64::MAX as f64 =>
                        {
                            Some(value as u64)
                        }
                        ValueSnapshot::Undefined | ValueSnapshot::Null => None,
                        _ => bail!("Native terminal usage.{key} is not a known nonnegative integer or unknown"),
                    });
                }
                let lower = buckets.iter().flatten().copied().fold(0u64, u64::saturating_add);
                Ok((lower, buckets.iter().all(Option::is_some).then_some(lower)))
            }),
        }
    }
    pub fn context_receipt(
        &self,
        messages: &[AgentMessage],
    ) -> Result<Option<super::super::rpc_host_retry::NativeRetryContextReceipt>> {
        match self {
            Self::Legacy(_) => Ok(None),
            Self::Native { session, .. } => Ok(Some(super::super::rpc_host_retry::NativeRetryContextReceipt::capture(
                messages,
                session.values.clone().context("Native context values missing")?,
            )?)),
        }
    }
    pub fn require_current(&self, host: &Host, kind: ContinuationKind) -> Result<()> {
        let Self::Native { source_model, .. } = self else {
            return Ok(());
        };
        self.check_host(host)?;
        let purpose = match kind {
            ContinuationKind::Empty => FactsPurpose::Empty,
            ContinuationKind::Unexpected => FactsPurpose::Unexpected,
            ContinuationKind::Overflow => FactsPurpose::Overflow,
        };
        let facts = self.facts_for(source_model, purpose)?;
        let class = self.class(source_model)?;
        let eligible = facts.current_route
            && !class.context_recovery_blocked
            && match kind {
                ContinuationKind::Empty => {
                    let provider_empty = facts.stop_reason == StopReason::Error
                        && class.error_id & flag::EMPTY_RESPONSE != 0
                        && facts.provider_empty_content;
                    provider_empty || facts.empty_terminal && facts.safe_success
                }
                ContinuationKind::Unexpected => {
                    facts.stop_reason == StopReason::Stop
                        && !facts.has_tools
                        && !facts.has_text
                        && facts.signed_thinking
                        && facts.safe_success
                }
                ContinuationKind::Overflow => {
                    !facts.has_tools
                        && match facts.stop_reason {
                            StopReason::Length => facts.safe_success,
                            StopReason::Error => {
                                let (lower, _) = self.input_usage()?;
                                class.overflow
                                    || class.error_id & flag::PAYLOAD_REJECTED != 0
                                    || source_model
                                        .context_window
                                        .is_some_and(|window| window > 0.0 && lower as f64 > window)
                            }
                            _ => false,
                        }
                }
            };
        if !eligible {
            bail!("Native terminal CURRENT recovery predicate changed")
        }
        Ok(())
    }
    pub fn retry_class(&self, model: &ara_ai::Model) -> Result<Option<RetryClass>> {
        match self {
            Self::Legacy(message) => Ok(Some(ara_ai::retry_classification::classify_retry(message, &model.api))),
            Self::Native { message, .. } => self.read(|_, values, imports| {
                Ok(same_route_class_actual(model, message, values, &imports.classifier)?.map(|metadata| metadata.class))
            }),
        }
    }
    pub fn public_message(&self, session: &Arc<Session>) -> Result<WireValue> {
        match self {
            Self::Legacy(message) => Ok(wire(AgentEvent::MessageEnd { message: Message::Assistant(message.clone()).into() }
                .full()?["message"]
                .clone())),
            Self::Native { message, .. } => Ok(session
                .projector
                .public_event_wire(&AgentEvent::MessageEnd { message: message.clone() }, true)?.get("message")
                .cloned().unwrap_or(WireValue::Null)),
        }
    }
    pub fn public_persistence_key(&self) -> Result<String> {
        match self {
            Self::Legacy(message) => Ok(super::super::rpc_host_retry::persistence_key(message)),
            _ => self.read(|root, values, _| {
                let display = |key| -> Result<String> {
                    Ok(values
                        .heap()
                        .to_string(&get(values, root, key)?, values.runtime())?
                        .to_utf8()
                        .context("Native retry public key must be scalar text")?)
                };
                let response = get(values, root, "responseId")?;
                let response = if nullish(values, &response)? {
                    String::new()
                } else {
                    values
                        .heap()
                        .to_string(&response, values.runtime())?
                        .to_utf8()
                        .context("Native retry responseId is not scalar public text")?
                };
                Ok(format!(
                    "assistant:{}:{}:{}:{}:{}",
                    display("timestamp")?,
                    display("provider")?,
                    display("model")?,
                    response,
                    display("stopReason")?
                ))
            }),
        }
    }
    pub fn discard(&self, journal: &mut SessionJournal, leaf: Option<&str>, id: &str, empty: bool) -> Result<()> {
        match self {
            Self::Legacy(message) => {
                if empty {
                    journal.discard_empty_assistant_terminal_durably(leaf, id, message)?;
                } else {
                    journal.discard_entry_durably(leaf, id, message)?;
                }
            }
            _ => {
                let source = self.source(journal, id)?;
                if empty {
                    journal.discard_native_empty_assistant_terminal_durably(leaf, id, &source)?;
                } else {
                    journal.discard_native_entry_durably(leaf, id, &source)?;
                }
            }
        }
        Ok(())
    }
    pub fn discard_promotion(
        &self,
        journal: &mut SessionJournal,
        leaf: Option<&str>,
        id: &str,
        target: &str,
    ) -> Result<()> {
        match self {
            Self::Legacy(message) => {
                journal.discard_failed_assistant_for_promotion(leaf, id, message, target)?;
            }
            _ => {
                let source = self.source(journal, id)?;
                journal.discard_native_failed_assistant_for_promotion(leaf, id, &source, target)?;
            }
        }
        Ok(())
    }
    pub fn begin_recovery(&self, journal: &mut SessionJournal, leaf: &str, id: &str) -> Result<RecoveryToken> {
        Ok(match self {
            Self::Legacy(message) => RecoveryToken::Legacy(journal.begin_failed_assistant_recovery(leaf, id, message)?),
            _ => {
                let source = self.source(journal, id)?;
                RecoveryToken::Native(journal.begin_native_failed_assistant_recovery(leaf, id, &source)?)
            }
        })
    }
}
fn get(values: &SessionValues, root: &ValueHandle, key: &str) -> Result<ValueHandle> {
    Ok(values.heap().get(root, &key.into(), values.runtime())?)
}
fn string(values: &SessionValues, value: &ValueHandle) -> Result<ara_rpc::WireString> {
    match values.heap().snapshot(value)? {
        ValueSnapshot::String(value) => Ok(value),
        _ => bail!("Native terminal String field has wrong type"),
    }
}
fn nullish(values: &SessionValues, value: &ValueHandle) -> Result<bool> {
    Ok(matches!(values.heap().snapshot(value)?, ValueSnapshot::Null | ValueSnapshot::Undefined))
}
fn nonblank(values: &SessionValues, value: &ValueHandle) -> Result<bool> {
    let trim = get(values, value, "trim")?;
    let trimmed = values.heap().call(&trim, value, &[], values.runtime())?;
    Ok(values.heap().to_number(&get(values, &trimmed, "length")?, values.runtime())? > 0.0)
}

pub(super) enum RecoveryToken {
    Legacy(FailedAssistantRecovery),
    Native(NativeFailedAssistantRecovery),
}
impl From<FailedAssistantRecovery> for RecoveryToken {
    fn from(token: FailedAssistantRecovery) -> Self {
        Self::Legacy(token)
    }
}
impl RecoveryToken {
    pub fn transaction_mut(&mut self) -> &mut FailedAssistantRecovery {
        match self {
            Self::Legacy(token) => token,
            Self::Native(token) => token.transaction_mut(),
        }
    }
    pub fn rewritten(&self) -> bool {
        match self {
            Self::Legacy(token) => token.local_history_rewritten(),
            Self::Native(token) => token.local_history_rewritten(),
        }
    }
    pub fn finish(
        self,
        journal: &mut SessionJournal,
        rewritten: bool,
    ) -> Result<
        Option<(
            ara_session::AgentMessageBody,
            ara_session::AgentEntrySource,
            Option<String>,
            Option<Box<AssistantMessage>>,
        )>,
    > {
        match self {
            Self::Legacy(token) => {
                let source = token.failed_entry_source().clone();
                let body = token.failed_message_body()?;
                Ok(match journal.finish_failed_assistant_recovery(token, rewritten)? {
                    FailedAssistantRecoveryOutcome::Committed => None,
                    FailedAssistantRecoveryOutcome::Restored { message, appended_entry_id } => {
                        Some((body, source, appended_entry_id, Some(message)))
                    }
                })
            }
            Self::Native(token) => Ok(match journal.finish_native_failed_assistant_recovery(token, rewritten)? {
                NativeFailedAssistantRecoveryOutcome::Committed => None,
                NativeFailedAssistantRecoveryOutcome::Restored { body, source, appended_entry_id } => {
                    Some((body, source, appended_entry_id, None))
                }
            }),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ContinuationKind {
    Empty,
    Unexpected,
    Overflow,
}
#[derive(Clone)]
pub(super) struct NativeContinuationGuard {
    terminal: RecoveryTerminal,
    kind: ContinuationKind,
}
impl NativeContinuationGuard {
    pub fn for_terminal(terminal: &RecoveryTerminal, kind: ContinuationKind) -> Option<Self> {
        terminal.actual().map(|_| Self { terminal: terminal.clone(), kind })
    }
    pub fn recheck(&self, host: &Host) -> Result<()> {
        self.terminal.require_current(host, self.kind)
    }
}
/// Retains the pre-removal original replay receipts; they are observed again
/// after backoff. A root identity receipt never freezes these nested facts.
pub(super) struct NativeRetryWaitGuard {
    pub terminal: RecoveryTerminal,
    pub context: Vec<AgentMessage>,
    pub disposition: RetryDisposition,
    pub usage_admitted: bool,
    pub switched: bool,
    pub attempt: usize,
    pub wait_ceiling: f64,
}
impl NativeRetryWaitGuard {
    pub fn recheck(&self, host: &Host) -> Result<()> {
        self.terminal.check_host(host)?;
        let terminal = &self.terminal;
        let RecoveryTerminal::Native { source_model, .. } = terminal else { bail!("Native retry guard is legacy") };
        let Some(mut class) = terminal.retry_class(source_model)? else {
            bail!("Native retry CURRENT classification is no longer admitted")
        };
        if self.usage_admitted && class.usage_limit && terminal.usage_admission()? {
            class.replay_blocked = false;
        }
        let Some((same_route, disposition)) = terminal.retry_dispositions(&self.context, &class)? else {
            bail!("Native retry CURRENT replay effects no longer admit continuation")
        };
        if disposition != self.disposition || !same_route && !self.switched {
            bail!("Native retry CURRENT disposition changed")
        }
        if !self.switched && self.attempt as f64 > terminal.max_retries(host.retry_policy.max_retries())? {
            bail!("Native retry CURRENT budget exhausted")
        }
        if let Some(wait) = terminal.retry_hint(chrono::Utc::now().timestamp_millis() as f64)?
            && wait > self.wait_ceiling
            && !self.switched
            && !class.stale_responses
        {
            bail!("Native retry CURRENT provider wait increased")
        }
        Ok(())
    }
}

// Joined to the existing Native retry bank; no production mutation entrance.
#[cfg(test)]
pub(in super::super) fn observe_native_retry_wait_guard_family() {
    let (mut host, _output) = super::tests::fixture();
    let owner = ara_cli::native_factory::PreparedNativeSession::prepare().unwrap();
    let values = owner.values().clone();
    host.session = Session::new(
        None, host.session.header.clone(), &[], Some(values.clone()), Some(owner),
    ).unwrap();
    let mut body = serde_json::to_value(AssistantMessage::empty(
        &host.config.model.api, &host.config.model.provider, &host.config.model.id,
    )).unwrap();
    body["role"] = json!("assistant");
    body["stopReason"] = json!("error");
    body["errorId"] = json!(flag::CLASS | flag::TRANSIENT);
    body["errorMessage"] = json!("server_error: retry-after-ms=7200000");
    let root = values.heap().import_serde(&body).unwrap();
    let message = AgentMessage::from_graph(root.clone(), None).unwrap();
    let terminal = RecoveryTerminal::native(message.clone(), &host).unwrap();
    let original_wait = terminal.retry_hint(0.0).unwrap().unwrap();
    assert_eq!(original_wait, 7_200_000.0);
    let class = terminal.retry_class(&host.config.model).unwrap().unwrap();
    let context = vec![message];
    let (same_route, disposition) = terminal.retry_dispositions(&context, &class).unwrap().unwrap();
    assert!(same_route);
    let guard = NativeRetryWaitGuard {
        terminal, context, disposition, usage_admitted: false, switched: false,
        attempt: 1, wait_ceiling: original_wait.max(500.0),
    };
    guard.recheck(&host).expect("unchanged original provider wait stays admitted");
    values.heap().define_own_data(
        &root, "errorMessage".into(),
        values.heap().string("server_error: retry-after-ms=10800000".into()).unwrap(),
        values.runtime(),
    ).unwrap();
    let error = guard.recheck(&host).expect_err("a genuinely increased CURRENT raw hint must stop replay");
    assert_eq!(error.to_string(), "Native retry CURRENT provider wait increased");
    values.heap().define_own_data(
        &root, "errorMessage".into(),
        values.heap().string("server_error: retry-after-ms=7200000".into()).unwrap(),
        values.runtime(),
    ).unwrap();
    guard.recheck(&host).expect("restored same original hint remains admitted");
}

impl Host {
    pub(super) async fn finish_retry_native(
        &mut self,
        saga: RetrySaga,
        success: Option<&RecoveryTerminal>,
        error: Option<String>,
        supersede: bool,
    ) {
        use super::super::rpc_host_retry::annotate_retry_actual;
        saga.cancel.cancel();
        let mut final_error = error;
        let mut updates = Vec::new();
        let annotation_result: Result<()> = async {
            if success.is_none() && !supersede {
                return Ok(());
            }
            if !Arc::ptr_eq(&saga.session, &self.session) || saga.generation != self.prompt_generation {
                bail!("Native retry annotation Session generation changed")
            }
            let values = saga.session.values.as_ref().context("Native retry annotation values missing")?;
            let superseding = match success {
                Some(terminal) => {
                    terminal.check_host(self)?;
                    Some(terminal.actual().context("Native retry success is not original Graph")?)
                }
                None => None,
            };
            // Admit durable roots before any original Setter. No clone of the
            // journal can roll back externally observable Native mutation.
            let sources = {
                let journal = saga.session.journal.lock().await;
                saga.pending
                    .iter()
                    .map(|pending| {
                        pending.message.check_host(self)?;
                        Ok((pending.entry_id.clone(), pending.message.source(&journal, &pending.entry_id)?))
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            let failed_public = saga
                .pending
                .iter()
                .map(|pending| pending.message.public_message(&saga.session))
                .collect::<Result<Vec<_>>>()?;
            let recovered_at = success.map(|_| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
            let mut metadata = Vec::new();
            for pending in &saga.pending {
                let failed = pending.message.actual().context("Native retry failure is not original Graph")?;
                let annotation = annotate_retry_actual(
                    failed,
                    superseding,
                    pending.attempt,
                    &pending.recovery,
                    &pending.note,
                    recovered_at.as_deref(),
                    values,
                )?;
                failed.invalidate_derived();
                // Annotation metadata is an explicit public output boundary.
                metadata.push((values.heap().export_serde(&annotation)?, pending.message.public_persistence_key()?));
            }
            saga.session.journal.lock().await.persist_native_retry_recovery(&sources)?;
            let mut public = saga.session.messages.lock().unwrap();
            for ((pending, failed), (annotation, persistence_key)) in
                saga.pending.iter().zip(&failed_public).zip(metadata)
            {
                for candidate in public.iter_mut().filter(|candidate| candidate.deep_equal(failed)) {
                    candidate.insert("retryRecovery", wire(annotation.clone()));
                }
                updates.push(json!({"entryId":pending.entry_id,"persistenceKey":persistence_key,
                    "note":pending.note,"retryRecovery":annotation}));
            }
            Ok(())
        }
        .await;
        if let Err(error) = annotation_result {
            let reason = format!("Native retry recovery annotation/persistence failed: {error}");
            final_error.get_or_insert_with(|| reason.clone());
            saga.session.persistence_error.lock().unwrap().get_or_insert(reason);
            self.connection.cancel();
            self.drain_queues = false;
        }
        let mut event = json!({"type":"auto_retry_end","success":success.is_some() && final_error.is_none(),"attempt":saga.attempt});
        if let Some(error) = final_error {
            event["finalError"] = json!(error);
        }
        if success.is_some() || supersede {
            event["retryErrors"] = json!(updates);
        }
        self.output.frame(event);
    }
}

impl Host {
    pub(super) async fn begin_header_continue_native(
        &mut self,
        active: &ActiveRun,
        hit: HeaderInterruption,
        target: loop_guard::NativeHeaderTarget,
    ) -> Result<bool> {
        let mut messages = self.agent.raw_messages().await;
        let Some(message) = messages.last().filter(|message| message.same_record(&target.message)).cloned() else {
            return Ok(false);
        };
        // Cancellation finalizes this same record with a new shallow Graph root.
        // RecoveryTerminal pins and rechecks the current root below.
        let terminal = RecoveryTerminal::native(message.clone(), self)?;
        if !terminal.exact_tail(&messages)? {
            return Ok(false);
        }
        let eligible = terminal.read(|root, values, _| {
            let timestamp = match values.heap().snapshot(&get(values, root, "timestamp")?)? {
                ValueSnapshot::Number(timestamp) => timestamp,
                _ => bail!("Gemini native terminal timestamp is not Number"),
            };
            Ok(timestamp == target.timestamp
                && string(values, &get(values, root, "role")?)?.equals_ascii("assistant")
                && string(values, &get(values, root, "stopReason")?)?.equals_ascii("aborted"))
        })?;
        if !eligible {
            return Ok(false);
        }
        let class = RetryClass { abort: true, ..Default::default() };
        if !matches!(terminal.retry_dispositions(&messages, &class)?, Some((_, RetryDisposition::RemoveFailed))) {
            return Ok(false);
        }
        terminal.check_host(self)?;
        let entry_id = terminal.persisted_id(active)?;
        let notice = LoopGuardNotice::gemini_headers(hit.headers);
        let notice_id = {
            let mut journal = self.session.journal.lock().await;
            let source = terminal.source(&journal, &entry_id)?;
            journal.append_native_gemini_reminder_after_aborted(&notice, &entry_id, &source)?
        };
        terminal.check_host(self)?;
        messages.pop();
        let raw = self.session.projector.new_custom_input(notice.event_message())?;
        self.session.projector.record_entry_id(&raw, notice_id);
        messages.push(raw);
        self.agent.replace_idle_raw_messages(messages.clone())?;
        let failed = self.session.projector.public_event_wire(&AgentEvent::MessageEnd { message }, true)?.get("message").cloned().unwrap_or(WireValue::Null);
        let public = notice.event_message();
        {
            let mut transcript = self.session.messages.lock().unwrap();
            if let Some(index) = transcript.iter().rposition(|candidate| candidate.deep_equal(&failed)) {
                transcript.remove(index);
            }
            transcript.push(wire(public.clone()));
        }
        self.output.frame(json!({"type":"message_start","message":public}));
        self.output.frame(json!({"type":"message_end","message":public}));
        self.header_continue = Some(PendingHeaderContinue {
            session: self.session.clone(),
            generation: hit.generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            expected_messages: self.session.projector.current_body_receipts(&messages)?,
            deadline: Instant::now() + Duration::from_millis(1),
        });
        self.auto_compaction_pending = false;
        Ok(true)
    }
}
