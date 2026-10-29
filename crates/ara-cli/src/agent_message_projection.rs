//! Fixed coding-agent messages.ts:1142-1320 and Core compaction/messages.ts.
//! OMP 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; repository notices).
//! A Host owns this weak cache. Core owns canonical raw records, never Session
//! paths. Rust prepared DTOs are owned snapshots; no Native array shortcut.

use ara_agent::{AgentMessage, AgentMessageIdentity, RawContextError};
use ara_ai::{AssistantBlock, AssistantMessage, Message, Model, StopReason};
use ara_session::{AgentEntrySource, AgentMessageBody, Entry, NativeMessageSource, SessionJournal};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[path = "actual_message_projection.rs"]
mod actual;
use actual::ActualMemo;
#[path = "native_budget_observation.rs"]
mod native_budget_observation;

/// Reuse the existing model DTO observer at the Session compaction boundary.
/// Only GraphJournalEncoder's assistant/tool-result guard calls this;
/// it is neither a canonical-message conversion nor a general raw fallback.
pub(crate) fn compaction_message_observation(
    root: &ara_rpc::value_heap::ValueHandle,
    values: &ara_agent::SessionValues,
) -> Result<Value, ara_rpc::value_heap::ValueError> {
    native_budget_observation::compaction_body(root, values)
}

enum Fragment {
    AliasedRecord(AgentMessageIdentity),
    DerivedSnapshot(Vec<Message>),
    /// Disposable exact model projection from a verified current archive source.
    DerivedNativeSnapshot(NativeMessageSource),
}
struct Memo {
    generation: u64,
    interrupted_next: bool,
    route: Route,
    /// Checked native proof is valid only for this exact CURRENT body. Store a
    /// digest, never a second canonical body/transcript or a source restore.
    checked_body_sha256: Option<[u8; 32]>,
    fragment: Fragment,
}
type Route = (String, String, String, String, bool, bool, String);
#[derive(Clone)]
struct DonorRoute {
    model: Model,
    metadata: Option<Value>,
    settings: ara_agent::remote::RemoteSettings,
    supports_images: bool,
}
#[derive(Default)]
pub struct AgentMessageProjector {
    cache: Mutex<HashMap<AgentMessageIdentity, Memo>>,
    actual_cache: Mutex<HashMap<AgentMessageIdentity, ActualMemo>>,
    entry_ids: Mutex<HashMap<AgentMessageIdentity, String>>,
    checked_route: Mutex<Option<(Model, bool, bool)>>,
    donor: Mutex<Option<DonorRoute>>,
    native_encoder: Mutex<Option<Arc<crate::actual_session::GraphJournalEncoder>>>,
}

fn error(error: impl std::fmt::Display) -> RawContextError {
    RawContextError(error.to_string())
}

fn route(model: &Model, native_replay_available: bool, supports_images: bool, policy: String) -> Route {
    (
        model.api.clone(),
        model.provider.clone(),
        model.id.clone(),
        model.base_url.clone(),
        native_replay_available,
        supports_images,
        policy,
    )
}
fn body_sha256(body: &Value) -> [u8; 32] {
    ring::digest::digest(&ring::digest::SHA256, &serde_json::to_vec(body).expect("raw JSON serializes"))
        .as_ref()
        .try_into()
        .expect("SHA256 digest length")
}

pub fn source_entry_id(message: &AgentMessage) -> Option<String> {
    message.source()?.get("id").and_then(Value::as_str).map(str::to_owned)
}

impl AgentMessageProjector {
    fn snapcompact_route_policy(
        &self,
        model: &Model,
    ) -> Result<(ara_snapcompact::DeploymentTarget, ara_snapcompact::HistoryBlockOptions, bool), RawContextError> {
        let donor = self.donor.lock().unwrap().as_ref().filter(|donor| &donor.model == model).cloned();
        let deployment = crate::snapcompact::deployment_target(model, donor.as_ref().and_then(|d| d.metadata.as_ref()))
            .map_err(error)?;
        let bytes = ara_snapcompact::frame_data_bytes_budget(Some(&deployment)).map_err(error)?;
        let include_thinking = ara_snapcompact::reader_includes_thinking(&deployment).map_err(error)?;
        Ok((
            deployment,
            ara_snapcompact::HistoryBlockOptions { max_frame_data_bytes: Some(bytes), ..Default::default() },
            include_thinking,
        ))
    }
    fn route(
        &self,
        model: &Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Route, RawContextError> {
        let (deployment, options, include_thinking) = self.snapcompact_route_policy(model)?;
        // The captured built identity, providerType, wire-id fallback and actual
        // endpoint policy all invalidate derived views when their result changes.
        Ok(route(
            model,
            native_replay_available,
            supports_images,
            format!("{deployment:?}:{options:?}:{include_thinking}"),
        ))
    }
    /// Bind the one existing journal before adopting its Graph context. Current
    /// Live bodies retain their handles; durable bodies import once into the
    /// Host's current realm. Failed candidate admission leaves this binding unset.
    pub fn bind_native_session(
        &self,
        journal: &mut SessionJournal,
        encoder: Arc<crate::actual_session::GraphJournalEncoder>,
    ) -> Result<(), RawContextError> {
        if self
            .native_encoder
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|current| !Arc::ptr_eq(current.values(), encoder.values()))
        {
            return Err(error("actual projector cannot replace the current Session realm"));
        }
        journal.bind_native_realm(encoder.values().heap(), encoder.clone()).map_err(error)?;
        *self.native_encoder.lock().unwrap() = Some(encoder);
        Ok(())
    }

    pub fn message_from_body(
        &self,
        body: AgentMessageBody,
        source: AgentEntrySource,
    ) -> Result<AgentMessage, RawContextError> {
        match body {
            AgentMessageBody::Legacy(body) => {
                self.new_legacy_message(body, Some(Arc::new(source.checked_legacy().map_err(error)?)))
            }
            AgentMessageBody::Native(native) => {
                let encoder = self.native_encoder.lock().unwrap().clone();
                if let Some(encoder) = encoder {
                    // Import belongs to bind_native_session. Reimporting here
                    // would create a second canonical body for the same entry.
                    if matches!(native, NativeMessageSource::Persisted { .. })
                        && !matches!(&source, AgentEntrySource::Snapcompact { .. })
                    {
                        return Err(error("native journal must bind the current realm before context adoption"));
                    }
                    let envelope = match source {
                        AgentEntrySource::Native { envelope, .. } | AgentEntrySource::Snapcompact { envelope, .. } => {
                            envelope
                        }
                        AgentEntrySource::Legacy(source) => source,
                    };
                    encoder.message_from_source(&native, Some(Arc::new(envelope))).map_err(error)
                } else {
                    // The existing Host's explicit legacy mode has no actual
                    // values binding. Its checked bridge propagates failures;
                    // it never substitutes for a configured Graph consumer.
                    AgentMessage::from_raw(
                        native.checked_body().map_err(error)?,
                        Some(Arc::new(source.checked_legacy().map_err(error)?)),
                    )
                }
            }
        }
    }

    fn current_role(&self, message: &AgentMessage) -> Result<bool, RawContextError> {
        match message.body_snapshot() {
            ara_agent::AgentMessageBody::Model(_) | ara_agent::AgentMessageBody::RawJson(_) => {
                Ok(message.json()?["role"] == "compactionSummary")
            }
            ara_agent::AgentMessageBody::Graph(root) => {
                let encoder = self
                    .native_encoder
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| error("actual journal encoder is unavailable"))?;
                encoder.values().check_handle(&root).map_err(error)?;
                let role =
                    encoder.values().heap().get(&root, &"role".into(), encoder.values().runtime()).map_err(error)?;
                Ok(matches!(encoder.values().heap().snapshot(&role).map_err(error)?,
                    ara_rpc::value_heap::ValueSnapshot::String(role) if role.equals_ascii("compactionSummary")))
            }
        }
    }

    /// Owned display observation of the current original message body.
    /// Native observations retain the bound encoder and original Graph root;
    /// this DTO does not import, persist, or grant execution authority.
    pub fn current_wire_body(&self, message: &AgentMessage) -> Result<Arc<ara_rpc::WireValue>, RawContextError> {
        match message.body_snapshot() {
            ara_agent::AgentMessageBody::Graph(root) => {
                let encoder = self
                    .native_encoder
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| error("actual Host journal binding is unavailable"))?;
                NativeMessageSource::live(root, encoder).observe().map_err(error)
            }
            _ => ara_session::native_wire_from_serde(&message.json()?).map(Arc::new).map_err(error),
        }
    }
    fn wire_digest(body: &ara_rpc::WireValue) -> [u8; 32] {
        use ara_rpc::{WireString, WireValue};
        enum Part<'a> {
            Value(&'a WireValue),
            Key(&'a WireString),
        }
        fn text(hash: &mut ring::digest::Context, text: &WireString) {
            hash.update(&(text.units().len() as u64).to_le_bytes());
            for unit in text.units() {
                hash.update(&unit.to_le_bytes());
            }
        }
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut pending = vec![Part::Value(body)];
        while let Some(part) = pending.pop() {
            match part {
                Part::Key(key) => {
                    hash.update(b"k");
                    text(&mut hash, key);
                }
                Part::Value(value) => match value {
                    WireValue::Null => hash.update(b"n"),
                    WireValue::Bool(value) => hash.update(if *value { b"t" } else { b"f" }),
                    WireValue::Number(value) => {
                        hash.update(b"d");
                        let bits = if value.is_nan() { f64::NAN.to_bits() } else { value.to_bits() };
                        hash.update(&bits.to_le_bytes());
                    }
                    WireValue::String(value) => {
                        hash.update(b"s");
                        text(&mut hash, value);
                    }
                    WireValue::Array(values) => {
                        hash.update(b"a");
                        hash.update(&(values.len() as u64).to_le_bytes());
                        pending.extend(values.iter().rev().map(Part::Value));
                    }
                    WireValue::Object(entries) => {
                        hash.update(b"o");
                        hash.update(&(entries.len() as u64).to_le_bytes());
                        let mut entries: Vec<_> = entries.iter().collect();
                        entries.sort_by(|(left, _), (right, _)| left.units().cmp(right.units()));
                        for (key, value) in entries.into_iter().rev() {
                            pending.push(Part::Value(value));
                            pending.push(Part::Key(key));
                        }
                    }
                },
            }
        }
        hash.finish().as_ref().try_into().expect("SHA256 digest length")
    }
    /// Comparison receipt only, never a message projection or restore source.
    fn wire_receipt(body: &ara_rpc::WireValue) -> Value {
        let digest = Self::wire_digest(body);
        let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        json!({"domain":"current-wire-body-receipt","sha256":digest})
    }
    pub fn current_body_receipts(&self, messages: &[AgentMessage]) -> Result<Vec<Value>, RawContextError> {
        messages
            .iter()
            .map(|message| match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(_) => {
                    Ok(Self::wire_receipt(self.current_wire_body(message)?.as_ref()))
                }
                _ => message.json(),
            })
            .collect()
    }
    /// Locate the observed provider anchor using its complete current body.
    /// Entry identity narrows ownership elsewhere; it is not equality proof.
    pub fn budget_anchor_position(
        &self,
        messages: &[AgentMessage],
        entry: &Entry,
    ) -> Result<Option<usize>, RawContextError> {
        if let Some(source) = &entry.native_message {
            let expected = source.observe().map_err(error)?;
            for (index, message) in messages.iter().enumerate().rev() {
                if self.current_wire_body(message)?.deep_equal(&expected) {
                    return Ok(Some(index));
                }
            }
            return Ok(None);
        }
        let Some(expected) = entry.message().map_err(error)? else { return Ok(None) };
        for (index, message) in messages.iter().enumerate().rev() {
            if self.current_role(message)? {
                continue;
            }
            let body = match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(root) => {
                    let values = self.actual_values().ok_or_else(|| error("actual Host values are not bound"))?;
                    native_budget_observation::budget_body(&root, &values).map_err(error)?
                }
                _ => message.json()?,
            };
            if body.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let current = serde_json::from_value::<Message>(body).map_err(error)?;
            if current == expected {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }
    fn native_projection_for_current(
        &self,
        message: &AgentMessage,
        selected_route: &Route,
    ) -> Result<Option<NativeMessageSource>, RawContextError> {
        let candidate = self.cache.lock().unwrap().get(&message.weak_identity()).and_then(|memo| {
            if memo.generation != message.derivation_generation() || &memo.route != selected_route {
                return None;
            }
            match &memo.fragment {
                Fragment::DerivedNativeSnapshot(source) => Some((memo.checked_body_sha256, source.clone())),
                _ => None,
            }
        });
        let Some((digest, source)) = candidate else { return Ok(None) };
        let current = self.current_wire_body(message)?;
        Ok((digest == Some(Self::wire_digest(&current))).then_some(source))
    }
    /// Typed callers retain a checked adapter; it never replaces exact history.
    fn checked_native_projection(source: &NativeMessageSource) -> Result<Vec<Message>, RawContextError> {
        let mut message: Message = serde_json::from_value(source.checked_body().map_err(error)?).map_err(error)?;
        if let Message::User(user) = &mut message
            && let ara_ai::UserContent::Blocks(blocks) = &mut user.content
        {
            for block in blocks {
                if let ara_ai::UserBlock::Image(image) = block {
                    image.compaction_frame = true;
                }
            }
        }
        Ok(vec![message])
    }
    /// Count exact archive projections without fabricating a typed transcript.
    pub fn count_current(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        options: ara_agent::tokenizer::MessageCountOptions,
    ) -> Result<usize, RawContextError> {
        let (available, images) = self.settings_for(model);
        let selected_route = self.route(model, available, images)?;
        let (deployment, _, _) = self.snapcompact_route_policy(model)?;
        let billing = ara_snapcompact::frame_billing(Some(&deployment.model)).map_err(error)?;
        let tokenizer = crate::context_budget::tokenizer(model);
        let mut total = 0usize;
        for message in messages {
            let count = match self.native_projection_for_current(message, &selected_route)? {
                Some(source) => crate::snapcompact::model_source_tokens_with_billing(&tokenizer, &source, &billing)
                    .map_err(error)?,
                None => tokenizer.count_messages(
                    &self.estimate_messages(std::slice::from_ref(message), model, available, images)?,
                    options,
                ),
            };
            total = total.saturating_add(count);
        }
        Ok(total)
    }

    fn checked_current_body(&self, message: &AgentMessage) -> Result<Value, RawContextError> {
        match message.body_snapshot() {
            ara_agent::AgentMessageBody::Model(_) | ara_agent::AgentMessageBody::RawJson(_) => message.json(),
            ara_agent::AgentMessageBody::Graph(root) => {
                use ara_session::NativeJournalEncoder;
                let encoder = self
                    .native_encoder
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| error("actual journal encoder is unavailable"))?;
                ara_session::native_checked_serde(&encoder.observe_body(&root).map_err(error)?).map_err(error)
            }
        }
    }

    pub fn set_donor(
        &self,
        model: &Model,
        metadata: Option<&Value>,
        settings: &ara_agent::remote::RemoteSettings,
        supports_images: bool,
    ) {
        *self.donor.lock().unwrap() = Some(DonorRoute {
            model: model.clone(),
            metadata: metadata.cloned(),
            settings: settings.clone(),
            supports_images,
        });
    }
    pub fn clear_donor(&self) {
        *self.donor.lock().unwrap() = None;
        *self.checked_route.lock().unwrap() = None;
    }
    /// Awaited Host converter calls this from its retained Session owner using
    /// the final admitted route. This updates weak typed projections only; a
    /// busy Agent's canonical records/array are never replaced by fragments.
    pub fn refresh_checked(
        &self,
        journal: &SessionJournal,
        messages: &[AgentMessage],
        model: &Model,
    ) -> Result<(), RawContextError> {
        let donor = self.donor.lock().unwrap().as_ref().filter(|donor| &donor.model == model).cloned();
        let Some(donor) = donor else {
            return Ok(());
        };
        let available = crate::remote_compaction::native_replay_available(
            journal,
            model,
            donor.metadata.as_ref(),
            &donor.settings,
            donor.supports_images,
        )
        .map_err(error)?;
        let route = self.route(model, available, donor.supports_images)?;
        let (_, history_options, include_thinking) = self.snapcompact_route_policy(model)?;
        *self.checked_route.lock().unwrap() = Some((model.clone(), available, donor.supports_images));
        for message in messages {
            if !self.current_role(message)? {
                continue;
            }
            if let Some(source_id) = self.entry_id(message) {
                let current = self.current_wire_body(message)?;
                if let Some(projection) = journal
                    .checked_snapcompact_source_for_route_with_options(
                        &current,
                        &source_id,
                        model,
                        available,
                        donor.supports_images,
                        history_options,
                        include_thinking,
                    )
                    .map_err(error)?
                {
                    self.cache.lock().unwrap().insert(
                        message.weak_identity(),
                        Memo {
                            generation: message.derivation_generation(),
                            interrupted_next: false,
                            route: route.clone(),
                            checked_body_sha256: Some(Self::wire_digest(&current)),
                            fragment: Fragment::DerivedNativeSnapshot(projection),
                        },
                    );
                    continue;
                }
            }
            let body = self.checked_current_body(message)?;
            let identity = message.weak_identity();
            let digest = body_sha256(&body);
            let generation = message.derivation_generation();
            if self.cache.lock().unwrap().get(&identity).is_some_and(|memo| {
                memo.generation == generation && memo.route == route && memo.checked_body_sha256 == Some(digest)
            }) {
                continue;
            }
            // Ordinary from_raw source JSON cannot register this origin.
            let source = self.entry_ids.lock().unwrap().get(&identity).cloned();
            let Some(source) = source else {
                continue;
            };
            if let Some(projection) = journal
                .checked_agent_message_projection_for_route(&body, &source, model, available, donor.supports_images)
                .map_err(error)?
            {
                self.cache.lock().unwrap().insert(
                    identity,
                    Memo {
                        generation,
                        interrupted_next: false,
                        route: route.clone(),
                        checked_body_sha256: Some(digest),
                        fragment: Fragment::DerivedSnapshot(projection),
                    },
                );
            }
        }
        Ok(())
    }
    /// Reopen/adopt checked Session source once; only this path can retain the
    /// temporary typed native proof. Ordinary AgentMessage::from_raw has none.
    pub fn raw_context(
        &self,
        journal: &SessionJournal,
        model: &Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Vec<AgentMessage>, RawContextError> {
        let route = self.route(model, native_replay_available, supports_images)?;
        let (_, history_options, include_thinking) = self.snapcompact_route_policy(model)?;
        let records = journal
            .raw_agent_context_for_route_with_images(model, native_replay_available, supports_images)
            .map_err(error)?;
        let mut output = Vec::with_capacity(records.len());
        self.cache.lock().unwrap().retain(|identity, _| identity.upgrade().is_some());
        for record in records {
            let exact = match (&record.body, &record.source) {
                (AgentMessageBody::Native(body), AgentEntrySource::Snapcompact { envelope, .. }) => {
                    let current = body.observe().map_err(error)?;
                    let source_id = envelope
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| error("archive source has no entry id"))?;
                    journal
                        .checked_snapcompact_source_for_route_with_options(
                            &current,
                            source_id,
                            model,
                            native_replay_available,
                            supports_images,
                            history_options,
                            include_thinking,
                        )
                        .map_err(error)?
                        .map(|source| (Self::wire_digest(&current), source))
                }
                _ => None,
            };
            let digest = if record.checked_projection.is_some() {
                Some(body_sha256(&record.body.checked_legacy().map_err(error)?))
            } else {
                None
            };
            let raw = self.message_from_body(record.body, record.source)?;
            if let Some(id) = source_entry_id(&raw) {
                self.record_entry_id(&raw, id);
            }
            let identity = raw.weak_identity();
            let generation = raw.derivation_generation();
            if let Some((digest, projection)) = exact {
                self.cache.lock().unwrap().insert(
                    identity.clone(),
                    Memo {
                        generation,
                        interrupted_next: false,
                        route: route.clone(),
                        checked_body_sha256: Some(digest),
                        fragment: Fragment::DerivedNativeSnapshot(projection),
                    },
                );
            } else if let Some(projection) = record.checked_projection {
                self.cache.lock().unwrap().insert(
                    identity,
                    Memo {
                        generation,
                        interrupted_next: false,
                        route: route.clone(),
                        checked_body_sha256: digest,
                        fragment: Fragment::DerivedSnapshot(projection),
                    },
                );
            }
            output.push(raw);
        }
        *self.checked_route.lock().unwrap() = Some((model.clone(), native_replay_available, supports_images));
        Ok(output)
    }
    fn settings_for(&self, model: &Model) -> (bool, bool) {
        self.checked_route
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(checked, _, _)| checked == model)
            .map(|(_, available, images)| (*available, *images))
            .unwrap_or((false, false))
    }
    pub fn convert_current(&self, messages: &[AgentMessage], model: &Model) -> Result<Vec<Message>, RawContextError> {
        let (available, images) = self.settings_for(model);
        self.convert(messages, model, available, images)
    }
    pub fn estimate_current(&self, messages: &[AgentMessage], model: &Model) -> Result<Vec<Message>, RawContextError> {
        let (available, images) = self.settings_for(model);
        self.estimate_messages(messages, model, available, images)
    }
    /// Same-file reload compares original-domain provider replay views.
    /// Legacy-only callers keep their existing checked comparison. No canonical
    /// body is exported, sanitized, rebuilt or admitted by this observation.
    pub fn raw_messages_equal_current(
        &self,
        left: &[AgentMessage],
        right: &[AgentMessage],
    ) -> Result<bool, RawContextError> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            match (left.body_snapshot(), right.body_snapshot()) {
                (ara_agent::AgentMessageBody::Graph(left), ara_agent::AgentMessageBody::Graph(right)) => {
                    let values = self.actual_values().ok_or_else(|| error("actual Host values are not bound"))?;
                    if !native_budget_observation::replay_equal(&left, &right, &values).map_err(error)? {
                        return Ok(false);
                    }
                }
                (ara_agent::AgentMessageBody::Graph(_), _) | (_, ara_agent::AgentMessageBody::Graph(_)) => {
                    return Err(error("Provider replay comparison cannot mix Graph and legacy owners"));
                }
                _ => {
                    if !raw_messages_equal(std::slice::from_ref(left), std::slice::from_ref(right))? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
    /// Explicit Session owner publication/reopen replacement. Retained entry
    /// IDs reuse their record; a changed owner body invalidates derived views.
    /// Provider fragments never become the replacement authority.
    pub fn reconcile_owned_context(
        &self,
        current: &[AgentMessage],
        selected: Vec<AgentMessage>,
    ) -> Result<Vec<AgentMessage>, RawContextError> {
        let retained: HashMap<_, _> = current.iter().filter_map(|raw| self.entry_id(raw).map(|id| (id, raw))).collect();
        let mut output = Vec::with_capacity(selected.len());
        for fresh in selected {
            let Some(previous) = self.entry_id(&fresh).and_then(|id| retained.get(&id).copied()) else {
                output.push(fresh);
                continue;
            };
            let changed = match (previous.body_snapshot(), fresh.body_snapshot()) {
                (ara_agent::AgentMessageBody::Graph(previous), ara_agent::AgentMessageBody::Graph(fresh)) => {
                    let values = self.actual_values().ok_or_else(|| error("actual Host values are not bound"))?;
                    values.check_handle(&previous).map_err(error)?;
                    values.check_handle(&fresh).map_err(error)?;
                    !previous.same_identity(&fresh)
                }
                (ara_agent::AgentMessageBody::Graph(_), _) | (_, ara_agent::AgentMessageBody::Graph(_)) => {
                    return Err(error("Session context reconciliation cannot mix Graph and legacy owners"));
                }
                _ => previous.json()? != fresh.json()?,
            };
            if changed {
                previous.replace_body(fresh.body_snapshot())?;
                previous.invalidate_derived();
            }
            let generation = previous.derivation_generation();
            let mut cache = self.cache.lock().unwrap();
            if let Some(mut checked) = cache.remove(&fresh.weak_identity()) {
                checked.generation = generation;
                cache.insert(previous.weak_identity(), checked);
            }
            output.push(previous.clone());
        }
        Ok(output)
    }
    pub fn record_entry_id(&self, message: &AgentMessage, id: String) {
        self.entry_ids.lock().unwrap().insert(message.weak_identity(), id);
    }
    pub fn entry_id(&self, message: &AgentMessage) -> Option<String> {
        self.entry_ids.lock().unwrap().get(&message.weak_identity()).cloned()
    }
    pub fn append_current_body(
        &self,
        journal: &mut SessionJournal,
        message: &AgentMessage,
    ) -> ara_session::Result<String> {
        let id = match message.body_snapshot() {
            ara_agent::AgentMessageBody::Model(_) | ara_agent::AgentMessageBody::RawJson(_) => {
                let body = message.json().map_err(|error| {
                    ara_session::SessionError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
                })?;
                journal.append_agent_message_body(&body)?
            }
            ara_agent::AgentMessageBody::Graph(body) => {
                let encoder = self.native_encoder.lock().unwrap().clone().ok_or_else(|| {
                    ara_session::SessionError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "actual journal encoder is unavailable",
                    ))
                })?;
                journal.append_native_message(NativeMessageSource::live(body, encoder))?
            }
        };
        self.record_entry_id(message, id.clone());
        Ok(id)
    }

    /// Separate Host budgeting projection; this never calls programmable Core
    /// transform/convert hooks or consumes queues. Usage admission has a raw
    /// transcript and explicitly asks this estimator for its typed fragments.
    pub fn estimate_messages(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Vec<Message>, RawContextError> {
        let mut output = Vec::new();
        let route = self.route(model, native_replay_available, supports_images)?;
        for message in messages {
            if let Some(source) = self.native_projection_for_current(message, &route)? {
                output.extend(Self::checked_native_projection(&source)?);
                continue;
            }
            let body = match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(root) => {
                    let values = self.actual_values().ok_or_else(|| error("actual Host values are not bound"))?;
                    native_budget_observation::budget_body(&root, &values).map_err(error)?
                }
                _ => message.json()?,
            };
            let generation = message.derivation_generation();
            // Checked summary carriers are admitted against their complete
            // CURRENT source, never the disposable budget field projection.
            let digest = if self.current_role(message)? {
                body_sha256(&self.checked_current_body(message)?)
            } else {
                body_sha256(&body)
            };
            let checked = self
                .cache
                .lock()
                .unwrap()
                .get(&message.weak_identity())
                .filter(|memo| {
                    memo.generation == generation && memo.route == route && memo.checked_body_sha256 == Some(digest)
                })
                .and_then(|memo| match &memo.fragment {
                    Fragment::DerivedSnapshot(messages) => Some(messages.clone()),
                    Fragment::DerivedNativeSnapshot(_) => None,
                    _ => None,
                });
            output.extend(match checked {
                Some(checked) => checked,
                None => {
                    Entry::project_agent_message_body_for_route(&body, model, native_replay_available).map_err(error)?
                }
            });
        }
        Ok(output)
    }

    pub fn convert(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Vec<Message>, RawContextError> {
        let route = self.route(model, native_replay_available, supports_images)?;
        self.cache.lock().unwrap().retain(|identity, _| identity.upgrade().is_some());
        let mut output = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if let Some(source) = self.native_projection_for_current(message, &route)? {
                output.extend(Self::checked_native_projection(&source)?);
                continue;
            }
            let identity = message.weak_identity();
            let generation = message.derivation_generation();
            let body = message.json()?;
            let digest = body_sha256(&body);
            let interrupted_next = if let Some(next) = messages.get(index + 1) {
                let next_body = next.json()?;
                next_body["role"] == "custom" && next_body["customType"] == "interrupted-thinking"
            } else {
                false
            };
            let cached = {
                let cache = self.cache.lock().unwrap();
                cache
                    .get(&identity)
                    .filter(|memo| {
                        memo.generation == generation
                            && memo.interrupted_next == interrupted_next
                            && memo.route == route
                            && memo.checked_body_sha256.is_none_or(|checked| checked == digest)
                    })
                    .map(|memo| match &memo.fragment {
                        Fragment::AliasedRecord(raw) => Cached::Alias(raw.clone()),
                        Fragment::DerivedSnapshot(fragment) => Cached::Snapshot(fragment.clone()),
                        Fragment::DerivedNativeSnapshot(source) => Cached::Native(source.clone()),
                    })
            };
            if let Some(cached) = cached {
                match cached {
                    Cached::Alias(raw) => {
                        if let Some(raw) = raw.upgrade() {
                            let Some(Message::Assistant(message)) = raw.checked_model_snapshot()? else {
                                return Err(RawContextError("aliased owned Assistant changed role".into()));
                            };
                            output.push(Message::Assistant(message));
                            continue;
                        }
                    }
                    Cached::Native(source) => {
                        output.extend(Self::checked_native_projection(&source)?);
                        continue;
                    }
                    Cached::Snapshot(fragment) => {
                        output.extend(fragment);
                        continue;
                    }
                }
            }
            let (fragment, aliases) = if let Some(Message::Assistant(mut assistant)) =
                message.checked_model_snapshot()?
            {
                // Fixed messages.ts:474 plus error/flags.ts:36,320. Imported
                // raw errorId is an extension until AI materializes that field.
                let error_id = body
                    .get("errorId")
                    .and_then(|value| value.as_i64().map(|id| id as u32).or_else(|| value.as_u64().map(|id| id as u32)))
                    .unwrap_or(0);
                let user_interrupted = assistant.stop_reason == StopReason::Aborted
                    && (error_id & 0x0400_0000 != 0
                        || assistant.error_message.as_deref() == Some("Interrupted by user"));
                let demoted = (interrupted_next || user_interrupted) && strip_demoted_thinking(&mut assistant);
                if assistant.content.is_empty()
                    && ((user_interrupted && !interrupted_next)
                        || (assistant.stop_reason == StopReason::Error && assistant.tool_calls().next().is_none()))
                {
                    (Vec::new(), false)
                } else {
                    (vec![Message::Assistant(assistant)], !demoted)
                }
            } else {
                (
                    Entry::project_agent_message_body_for_route(&body, model, native_replay_available)
                        .map_err(error)?,
                    false,
                )
            };
            let memo_fragment = if aliases {
                Fragment::AliasedRecord(identity.clone())
            } else {
                Fragment::DerivedSnapshot(fragment.clone())
            };
            {
                let mut cache = self.cache.lock().unwrap();
                cache.insert(
                    identity,
                    Memo {
                        generation,
                        interrupted_next,
                        route: route.clone(),
                        checked_body_sha256: None,
                        fragment: memo_fragment,
                    },
                );
            }
            output.extend(fragment);
        }
        self.entry_ids.lock().unwrap().retain(|identity, _| identity.upgrade().is_some());
        Ok(output)
    }
}

enum Cached {
    Native(NativeMessageSource),
    Alias(AgentMessageIdentity),
    Snapshot(Vec<Message>),
}

/// messages.ts demoteInterruptedThinking: skip empty trailing text, then strip
/// only the contiguous nonempty unsigned thinking run. No demotion keeps alias.
fn strip_demoted_thinking(message: &mut AssistantMessage) -> bool {
    let mut end = message.content.len();
    while end > 0
        && matches!(&message.content[end - 1], AssistantBlock::Text(text) if ara_prompt::js::trim(&text.text).is_empty())
    {
        end -= 1;
    }
    let mut start = end;
    while start > 0
        && matches!(&message.content[start - 1], AssistantBlock::Thinking(thinking)
        if !ara_prompt::js::trim(&thinking.thinking).is_empty() && thinking.thinking_signature.as_deref().is_none_or(str::is_empty))
    {
        start -= 1;
    }
    if start == end {
        return false;
    }
    message.content.truncate(start);
    true
}

/// Fixed messages.ts:135-143,194-314 compares provider-replay projections.
/// These disposable JSON values never replace canonical bodies or source cells.
pub fn raw_messages_equal(left: &[AgentMessage], right: &[AgentMessage]) -> Result<bool, RawContextError> {
    if left.len() != right.len() {
        return Ok(false);
    }
    for (left, right) in left.iter().zip(right) {
        if replay_message_value(left.json()?) != replay_message_value(right.json()?) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn replay_fields(body: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields.iter().filter_map(|field| body.get(*field).map(|value| ((*field).into(), value.clone()))).collect(),
    )
}

fn replay_message_value(body: Value) -> Value {
    match body.get("role").and_then(Value::as_str) {
        Some("user" | "developer") => replay_fields(&body, &["role", "content", "providerPayload"]),
        Some("assistant") => {
            let responses =
                matches!(body.get("api").and_then(Value::as_str), Some("openai-responses" | "openai-codex-responses"));
            let mut result =
                replay_fields(&body, &["role", "content", "api", "provider", "model", "stopReason", "errorMessage"]);
            if responses {
                if let Some(Value::Array(content)) = body.get("content") {
                    result["content"] = Value::Array(
                        content
                            .iter()
                            .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                                Some("thinking") => None,
                                Some("toolCall") => Some(replay_fields(block, &["type", "id", "name", "arguments"])),
                                Some("text") => Some(replay_fields(block, &["type", "text", "textSignature"])),
                                _ => Some(block.clone()),
                            })
                            .collect(),
                    );
                }
            } else if let Some(payload) = body.get("providerPayload") {
                result["providerPayload"] = payload.clone();
            }
            result
        }
        Some("toolResult") => replay_fields(&body, &["role", "toolName", "toolCallId", "isError", "content"]),
        Some("bashExecution" | "pythonExecution") => {
            let mut result = replay_fields(
                &body,
                &["role", "command", "code", "output", "exitCode", "cancelled", "excludeFromContext"],
            );
            // Native picks command for Bash and code for Python, including when
            // the raw record carries additional extension fields.
            result.as_object_mut().unwrap().remove(if body["role"] == "bashExecution" { "code" } else { "command" });
            if let Some(meta) = body.get("meta").filter(|value| ara_prompt::js::truthy(value)) {
                let mut normalized = replay_fields(meta, &["truncation", "limits"]);
                if let Some(diagnostics) = meta.get("diagnostics").filter(|value| ara_prompt::js::truthy(value)) {
                    normalized["diagnostics"] = replay_fields(diagnostics, &["summary", "messages"]);
                }
                result["meta"] = normalized;
            }
            result
        }
        Some("custom" | "hookMessage") => replay_fields(&body, &["role", "customType", "content"]),
        Some("branchSummary") => replay_fields(&body, &["role", "summary"]),
        Some("compactionSummary") => replay_fields(&body, &["role", "summary", "providerPayload"]),
        Some("fileMention") => {
            let mut result = replay_fields(&body, &["role", "files"]);
            if let Some(Value::Array(files)) = body.get("files") {
                result["files"] =
                    Value::Array(files.iter().map(|file| replay_fields(file, &["path", "content", "image"])).collect());
            }
            result
        }
        _ => body,
    }
}

/// True raw ingress for the existing Skill/guard sources. Source only records
/// origin; append and conversion always inspect the editable current body.
pub fn raw_custom_input(body: Value) -> Result<AgentMessage, RawContextError> {
    Entry::project_agent_message_body(&body).map_err(error)?;
    AgentMessage::from_raw(
        body.clone(),
        Some(Arc::new(json!({"origin":body.get("customType"), "attribution":body.get("attribution")}))),
    )
}

/// Owned receipts survive shared-record edits. They are comparison evidence,
/// never the authority for publishing a replacement transcript.
pub fn raw_body_receipts(messages: &[AgentMessage]) -> Result<Vec<Value>, RawContextError> {
    messages.iter().map(AgentMessage::json).collect()
}
pub fn raw_last_assistant(messages: &[AgentMessage]) -> Result<Option<AssistantMessage>, RawContextError> {
    let Some(last) = messages.last() else {
        return Ok(None);
    };
    Ok(last.checked_model_snapshot()?.as_ref().and_then(Message::as_assistant).cloned())
}
/// Retry's safety decision starts at the exact terminal record and includes
/// every following raw record. An unprojectable application record vetoes it.
pub fn raw_retry_tail(
    messages: &[AgentMessage],
    failed: &AssistantMessage,
) -> Result<Option<Vec<Message>>, RawContextError> {
    let mut start = None;
    for (index, raw) in messages.iter().enumerate().rev() {
        if raw.checked_model_snapshot()?.as_ref().and_then(Message::as_assistant) == Some(failed) {
            start = Some(index);
            break;
        }
    }
    let Some(start) = start else {
        return Ok(None);
    };
    let mut tail = Vec::with_capacity(messages.len() - start);
    for raw in &messages[start..] {
        let Some(message) = raw.checked_model_snapshot()? else {
            return Ok(None);
        };
        tail.push(message);
    }
    Ok(Some(tail))
}

#[cfg(test)]
mod replay_comparison_tests {
    use super::*;

    #[test]
    fn raw_provider_replay_comparison_preserves_native_roles_and_live_cells() {
        let usage = |input| serde_json::to_value(ara_ai::Usage { input: Some(input), ..Default::default() }).unwrap();
        let metadata = |item: &str| {
            serde_json::to_value(ara_ai::ToolCallProviderMetadata::Computer(ara_ai::ComputerToolCallMetadata {
                provider_item_id: item.into(),
                actions: vec![ara_ai::ComputerAction::try_from(json!({"type":"screenshot"})).unwrap()],
                pending_safety_checks: Vec::new(),
            }))
            .unwrap()
        };
        let base = json!({"role":"assistant","api":"openai-responses","provider":"fixture","model":"fixture",
            "stopReason":"stop","timestamp":1,"usage":usage(1),"providerPayload":{"items":[]},
            "content":[{"type":"text","text":"same","textSignature":"sig","extension":"old"},
                {"type":"thinking","thinking":"old"},
                {"type":"toolCall","id":"call","name":"read","arguments":{"path":"a"},"providerMetadata":metadata("old-item")}]});
        let source = Arc::new(json!({"entryId":"replay-source"}));
        let left = AgentMessage::from_raw(base.clone(), Some(source.clone())).unwrap();
        let mut changed = base.clone();
        changed["timestamp"] = json!(2);
        changed["usage"] = usage(9999);
        changed["providerPayload"] = json!({"items":[{"type":"reasoning"}]});
        changed["content"][0]["extension"] = json!("new");
        changed["content"][1]["thinking"] = json!("new");
        changed["content"][2]["providerMetadata"] = metadata("new-item");
        let right = AgentMessage::from_raw(changed.clone(), None).unwrap();
        assert!(raw_messages_equal(std::slice::from_ref(&left), std::slice::from_ref(&right)).unwrap());
        assert_eq!(left.json().unwrap(), base);
        assert_eq!(right.json().unwrap(), changed);
        assert!(Arc::ptr_eq(&left.source().unwrap(), &source));
        for path in [
            "/content/0/text",
            "/content/0/textSignature",
            "/content/2/id",
            "/content/2/arguments/path",
            "/model",
            "/stopReason",
            "/errorMessage",
        ] {
            let mut meaningful = changed.clone();
            if path == "/errorMessage" {
                meaningful["errorMessage"] = json!("new");
            } else {
                *meaningful.pointer_mut(path).unwrap() = json!("different");
            }
            assert_ne!(replay_message_value(base.clone()), replay_message_value(meaningful), "{path}");
        }
        for (role, retained, ignored) in [
            ("user", "providerPayload", "synthetic"),
            ("developer", "content", "timestamp"),
            ("toolResult", "toolCallId", "details"),
            ("bashExecution", "output", "extension"),
            ("pythonExecution", "code", "extension"),
            ("custom", "customType", "details"),
            ("hookMessage", "content", "timestamp"),
            ("branchSummary", "summary", "timestamp"),
            ("compactionSummary", "providerPayload", "timestamp"),
            ("unknown-fixture", "extension", "ignored-fixture"),
        ] {
            let original = json!({"role":role, (retained):"original", (ignored):"original"});
            let mut meaningful = original.clone();
            meaningful[retained] = json!("different");
            assert_ne!(replay_message_value(original.clone()), replay_message_value(meaningful), "{role}/{retained}");
            let mut runtime_only = original.clone();
            runtime_only[ignored] = json!("different");
            assert_eq!(
                replay_message_value(original.clone()) == replay_message_value(runtime_only),
                role != "unknown-fixture",
                "{role}/{ignored}"
            );
        }
        let files = json!({"role":"fileMention","files":[{"path":"a","content":"b","image":"c","extension":"old"}]});
        let mut changed_files = files.clone();
        changed_files["files"][0]["extension"] = json!("new");
        assert_eq!(replay_message_value(files.clone()), replay_message_value(changed_files.clone()));
        changed_files["files"][0]["path"] = json!("different");
        assert_ne!(replay_message_value(files), replay_message_value(changed_files));
        let meta = json!({"role":"bashExecution","meta":{"truncation":{"lines":1},"limits":{"bytes":10},
            "diagnostics":{"summary":"s","messages":["m"],"extension":"old"},"extension":"old"}});
        let mut changed_meta = meta.clone();
        changed_meta["meta"]["extension"] = json!("new");
        changed_meta["meta"]["diagnostics"]["extension"] = json!("new");
        assert_eq!(replay_message_value(meta.clone()), replay_message_value(changed_meta.clone()));
        changed_meta["meta"]["diagnostics"]["messages"] = json!(["different"]);
        assert_ne!(replay_message_value(meta), replay_message_value(changed_meta));
        let mut other = base.clone();
        other["api"] = json!("anthropic-messages");
        let mut other_changed = other.clone();
        other_changed["providerPayload"] = json!({"different":true});
        assert_ne!(replay_message_value(other), replay_message_value(other_changed));
    }
}
