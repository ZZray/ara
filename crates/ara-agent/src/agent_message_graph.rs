//! Graph profile/slot operations on the existing AgentMessageCell.
//! No Host lineage, provider transport or second message owner is introduced.

use super::*;
use ara_rpc::{
    WireString,
    value_heap::{
        CaptureOwnControls, CaptureOwnRecords, CapturedOwnScalar, GuardedFacts, GuardedPropertyError,
        GuardedPropertyOp, GuardedPropertyStep, GuardedPropertyValue, OrderedOwnRecords, OwnScalar, OwnScalarFact,
        PropertyKey, ValueError, ValueHandle, ValueRuntime,
    },
};

#[derive(Debug, Clone)]
pub enum GraphAdmissionError {
    StaleOwner,
    StaleRoot,
    MissingMember,
    BoundaryChanged,
    ForeignHeap,
    UnsupportedStructure(&'static str),
    Value(ValueError),
}

impl From<GuardedPropertyError> for GraphAdmissionError {
    fn from(error: GuardedPropertyError) -> Self {
        match error {
            GuardedPropertyError::MissingMember => Self::MissingMember,
            GuardedPropertyError::BoundaryChanged => Self::BoundaryChanged,
            GuardedPropertyError::UnsupportedStructure(reason) => Self::UnsupportedStructure(reason),
            GuardedPropertyError::Value(ValueError::ForeignHeap)
            | GuardedPropertyError::Operation(ValueError::ForeignHeap) => Self::ForeignHeap,
            GuardedPropertyError::Value(error) | GuardedPropertyError::Operation(error) => Self::Value(error),
        }
    }
}
impl std::fmt::Display for GraphAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for GraphAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Value(error) => Some(error),
            _ => None,
        }
    }
}

/// Callback-only failures retain their actual ValueError classifier. Admission
/// and compound failures are non-native, with the callback receipt kept intact.
#[derive(Debug, Clone)]
pub enum GraphOwnerError {
    Native(ValueError),
    Admission { reason: GraphAdmissionError, callback_failure: Option<ValueError> },
}
impl From<GraphAdmissionError> for GraphOwnerError {
    fn from(reason: GraphAdmissionError) -> Self {
        Self::Admission { reason, callback_failure: None }
    }
}
impl GraphOwnerError {
    pub fn is_native_exception(&self) -> bool {
        match self {
            Self::Native(error) => error.is_native_exception(),
            Self::Admission { .. } => false,
        }
    }
}
impl std::fmt::Display for GraphOwnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native(error) => write!(formatter, "Graph callback failed: {error}"),
            Self::Admission { reason, callback_failure } => {
                write!(formatter, "Graph current admission failed: {reason}")?;
                if let Some(error) = callback_failure {
                    write!(formatter, "; callback observation: {error}")?;
                }
                Ok(())
            }
        }
    }
}
impl std::error::Error for GraphOwnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Native(error) => Some(error),
            Self::Admission { reason, .. } => Some(reason),
        }
    }
}

fn required_text(
    controls: &[(PropertyKey, CapturedOwnScalar)],
    key: &'static str,
) -> Result<WireString, GraphAdmissionError> {
    match controls.iter().find(|(candidate, _)| candidate == &PropertyKey::from(key)).map(|(_, value)| value) {
        Some(CapturedOwnScalar::Text(value)) => Ok(value.clone()),
        _ => Err(GraphAdmissionError::UnsupportedStructure("Required Graph control must be own Text")),
    }
}

/// Caller holds the existing record guard. The final finite profile capture
/// observes role, selected nodes and conditional stopReason in one arena guard.
pub(super) fn capture_boundary(
    root: &ValueHandle,
) -> Result<(OwnedExecutionBoundary, Vec<ValueHandle>), GraphAdmissionError> {
    let heap = root.heap();
    let role_capture = heap
        .capture_own_controls(
            root,
            &CaptureOwnControls {
                root_scalar_keys: vec!["role".into()],
                selected_records_root_scalar_keys: Vec::new(),
                records: None,
            },
        )
        .map_err(GraphAdmissionError::from)?;
    let role = required_text(&role_capture.controls, "role")?;
    if role.units().is_empty() {
        return Err(GraphAdmissionError::UnsupportedStructure("Graph role must be nonempty Text"));
    }
    let assistant = role == WireString::from("assistant");
    let tool_result = role == WireString::from("toolResult");
    let request = CaptureOwnControls {
        root_scalar_keys: if tool_result {
            vec!["role".into(), "toolCallId".into(), "toolName".into()]
        } else {
            vec!["role".into()]
        },
        selected_records_root_scalar_keys: if assistant { vec!["stopReason".into()] } else { Vec::new() },
        records: assistant.then(|| CaptureOwnRecords {
            array_key: "content".into(),
            discriminator_key: "type".into(),
            discriminator: "toolCall".into(),
            scalar_keys: vec!["id".into(), "name".into()],
        }),
    };
    let captured = heap.capture_own_controls(root, &request).map_err(GraphAdmissionError::from)?;
    if required_text(&captured.controls, "role")? != role {
        return Err(GraphAdmissionError::BoundaryChanged);
    }
    let mut boundary = OwnedExecutionBoundary { role, calls: Vec::new(), runnable: None, result: None };
    let mut members = Vec::new();
    for record in captured.records {
        boundary.calls.push((required_text(&record.controls, "id")?, required_text(&record.controls, "name")?));
        members.push(record.member);
    }
    if !boundary.calls.is_empty() {
        let reason = required_text(&captured.controls, "stopReason")?;
        let reason =
            [StopReason::Stop, StopReason::Length, StopReason::ToolUse, StopReason::Error, StopReason::Aborted]
                .into_iter()
                .find(|candidate| reason == WireString::from(candidate.as_str()))
                .ok_or(GraphAdmissionError::UnsupportedStructure("Unknown Graph stopReason Text"))?;
        boundary.runnable = Some(matches!(reason, StopReason::ToolUse | StopReason::Stop));
    }
    if tool_result {
        boundary.result =
            Some((required_text(&captured.controls, "toolCallId")?, required_text(&captured.controls, "toolName")?));
    }
    Ok((boundary, members))
}

pub(super) fn admit_record(live: &mut AgentMessageRecord) -> Result<(), GraphAdmissionError> {
    let AgentMessageBody::Graph(root) = &live.body else {
        return Err(GraphAdmissionError::UnsupportedStructure("Current body is not Graph"));
    };
    let (boundary, _) = capture_boundary(root)?;
    if live.owned_boundary.as_ref().is_some_and(|pinned| pinned != &boundary) {
        return Err(GraphAdmissionError::BoundaryChanged);
    }
    if live.owned_boundary.is_none() {
        live.owned_boundary = Some(boundary);
    }
    Ok(())
}

fn slot_facts(boundary: &OwnedExecutionBoundary, member: &ValueHandle) -> GuardedFacts {
    let mut root = vec![OwnScalarFact { key: "role".into(), expected: OwnScalar::Text(boundary.role.clone()) }];
    if let Some(runnable) = boundary.runnable {
        let reasons: &[StopReason] = if runnable {
            &[StopReason::ToolUse, StopReason::Stop]
        } else {
            &[StopReason::Length, StopReason::Error, StopReason::Aborted]
        };
        root.push(OwnScalarFact {
            key: "stopReason".into(),
            expected: OwnScalar::TextOneOf(reasons.iter().map(|reason| WireString::from(reason.as_str())).collect()),
        });
    }
    GuardedFacts {
        root,
        ordered: Some(OrderedOwnRecords {
            array_key: "content".into(),
            discriminator_key: "type".into(),
            discriminator: "toolCall".into(),
            records: boundary
                .calls
                .iter()
                .map(|(id, name)| {
                    vec![
                        OwnScalarFact { key: "id".into(), expected: OwnScalar::Text(id.clone()) },
                        OwnScalarFact { key: "name".into(), expected: OwnScalar::Text(name.clone()) },
                    ]
                })
                .collect(),
        }),
        array_key: "content".into(),
        member: member.clone(),
    }
}

#[derive(Clone, Debug)]
struct StagedGraphProfile {
    boundary: OwnedExecutionBoundary,
    reason: StopReason,
    reason_text: WireString,
    members: Vec<ValueHandle>,
}

impl StagedGraphProfile {
    fn matches(&self, current: &Self) -> bool {
        self.boundary == current.boundary
            && self.reason_text == current.reason_text
            && self.members.len() == current.members.len()
            && self.members.iter().zip(&current.members).all(|(before, after)| before.same_identity(after))
    }
}

fn capture_final_profile(root: &ValueHandle) -> Result<StagedGraphProfile, GraphAdmissionError> {
    // The stream's terminal status and execution controls are observed in
    // one finite arena capture. No callback runs under either guard.
    let captured = root
        .heap()
        .capture_own_controls(
            root,
            &CaptureOwnControls {
                root_scalar_keys: vec!["role".into(), "stopReason".into()],
                selected_records_root_scalar_keys: Vec::new(),
                records: Some(CaptureOwnRecords {
                    array_key: "content".into(),
                    discriminator_key: "type".into(),
                    discriminator: "toolCall".into(),
                    scalar_keys: vec!["id".into(), "name".into()],
                }),
            },
        )
        .map_err(GraphAdmissionError::from)?;
    let role = required_text(&captured.controls, "role")?;
    if role != WireString::from("assistant") {
        return Err(GraphAdmissionError::UnsupportedStructure("Actual stream final root must be an assistant"));
    }
    let reason_text = required_text(&captured.controls, "stopReason")?;
    let reason = [StopReason::Stop, StopReason::Length, StopReason::ToolUse, StopReason::Error, StopReason::Aborted]
        .into_iter()
        .find(|reason| reason_text == WireString::from(reason.as_str()))
        .ok_or(GraphAdmissionError::UnsupportedStructure("Actual stream final stopReason must be known own Text"))?;
    let calls = captured
        .records
        .iter()
        .map(|record| Ok((required_text(&record.controls, "id")?, required_text(&record.controls, "name")?)))
        .collect::<Result<Vec<_>, GraphAdmissionError>>()?;
    let has_calls = !calls.is_empty();
    let boundary = OwnedExecutionBoundary {
        role,
        calls,
        runnable: has_calls.then_some(matches!(reason, StopReason::Stop | StopReason::ToolUse)),
        result: None,
    };
    Ok(StagedGraphProfile {
        boundary,
        reason,
        reason_text,
        members: captured.records.into_iter().map(|record| record.member).collect(),
    })
}

/// Same canonical owner before its final pin. This is a transient finite
/// observation, not an adopted boundary, registry or derived-cache generation.
#[derive(Clone, Debug)]
pub(crate) struct StagedGraphPreparation {
    owner: Weak<AgentMessageCell>,
    root: ValueHandle,
    profile: Arc<StagedGraphProfile>,
    slots: Vec<CurrentArgumentsSlot>,
    owned: bool,
}

impl StagedGraphPreparation {
    /// No Get(arguments), no callback under the owner guard. Source operations
    /// on non-call members/iterators also require both-outcome readmission.
    pub(crate) fn recheck_current(&self) -> Result<(), GraphOwnerError> {
        let owner = self.owner.upgrade().ok_or(GraphAdmissionError::StaleOwner)?;
        let live = owner.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::StaleRoot.into());
        };
        if !root.same_identity(&self.root) {
            return Err(GraphAdmissionError::StaleRoot.into());
        }
        let pinned = if self.owned {
            live.owned_boundary.as_ref() == Some(&self.profile.boundary)
        } else {
            live.owned_boundary.is_none()
        };
        if !pinned || !self.profile.matches(&capture_final_profile(root)?) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        Ok(())
    }
    #[allow(dead_code)] // The complete R2-B preparation driver is still unavailable.
    pub(crate) fn current_arguments_slot(&self, member: &ValueHandle) -> Result<CurrentArgumentsSlot, GraphOwnerError> {
        if self.root.heap().heap_token() != member.heap().heap_token() {
            return Err(GraphAdmissionError::ForeignHeap.into());
        }
        self.slots
            .iter()
            .find(|slot| slot.facts.member.same_identity(member))
            .cloned()
            .ok_or_else(|| GraphAdmissionError::MissingMember.into())
    }
}

#[derive(Clone, Copy, Debug)]
enum ArgumentsSlotMode {
    Owned,
    Staged,
}

/// Weak canonical-owner capability, not a tool id/index or cached argument map.
#[derive(Clone)]
pub struct CurrentArgumentsSlot {
    owner: Weak<AgentMessageCell>,
    root: ValueHandle,
    boundary: OwnedExecutionBoundary,
    mode: ArgumentsSlotMode,
    // Local receipt lineage only: ordinary Owned constructors have no origin.
    // A fresh binding must come from this exact expected-profile final pin.
    prepared_origin: Option<Arc<StagedGraphProfile>>,
    facts: GuardedFacts,
}
impl std::fmt::Debug for CurrentArgumentsSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CurrentArgumentsSlot")
            .field("root", &self.root)
            .field("member", &self.facts.member)
            .field("owner_alive", &self.owner.strong_count().ne(&0))
            .finish()
    }
}

impl AgentMessage {
    /// The existing Core stream owner records the received terminal before
    /// its unfinished-tool retention. No Host constructor can sign an origin.
    pub(crate) fn remember_stream_terminal_origin(
        &self,
        source: ValueHandle,
        values: &crate::provider_context::SessionValues,
    ) -> Result<(), GraphOwnerError> {
        values.check_handle(&source).map_err(GraphAdmissionError::Value)?;
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        if live.owned_boundary.is_some() || live.stream_terminal_origin.is_some() {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        let AgentMessageBody::Graph(current) = &live.body else {
            return Err(GraphAdmissionError::StaleRoot.into());
        };
        if !current.same_identity(&source) {
            return Err(GraphAdmissionError::StaleRoot.into());
        }
        live.stream_terminal_origin = Some(super::StreamTerminalOrigin { retained: source.clone(), source });
        Ok(())
    }

    /// Read-only, live terminal provenance for the exact finalized record.
    /// This is neither a journal identity nor replay/admission authority.
    /// Original Graph handles remain live; no typed or JSON body is rebuilt.
    pub fn stream_terminal_origin(
        &self,
        values: &crate::provider_context::SessionValues,
    ) -> Result<Option<ValueHandle>, GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let Some(origin) = &live.stream_terminal_origin else { return Ok(None) };
        let AgentMessageBody::Graph(current) = &live.body else {
            return Err(GraphAdmissionError::StaleRoot.into());
        };
        values.check_handle(current).map_err(GraphAdmissionError::Value)?;
        values.check_handle(&origin.source).map_err(GraphAdmissionError::Value)?;
        if !current.same_identity(&origin.retained) {
            return Err(GraphAdmissionError::StaleRoot.into());
        }
        let (boundary, _) = capture_boundary(current)?;
        if live.owned_boundary.as_ref() != Some(&boundary) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        Ok(Some(origin.source.clone()))
    }

    /// Finite identity controls for actual tail/result consumers. This never
    /// exports the canonical body, reads arguments, grants or prepares effects.
    #[allow(clippy::type_complexity)] // Existing finite controls, no projected message or additional owner.
    pub(crate) fn graph_identity_controls(
        &self,
    ) -> Result<(WireString, bool, Vec<(WireString, WireString)>, Option<(WireString, WireString)>), GraphOwnerError>
    {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::UnsupportedStructure("actual identity controls require Graph body").into());
        };
        let (boundary, _) = capture_boundary(root)?;
        if live.owned_boundary.as_ref().is_some_and(|pinned| pinned != &boundary) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        Ok((boundary.role, boundary.runnable == Some(true), boundary.calls, boundary.result))
    }
    /// Callback-free Host readmission of this exact canonical Graph root.
    /// This checks the existing finite execution boundary; it neither signs
    /// terminal provenance nor admits replay, tools or a replacement record.
    pub fn recheck_graph_root(&self, expected: &ValueHandle) -> Result<(), GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::StaleRoot.into());
        };
        if !root.same_identity(expected) {
            return Err(GraphAdmissionError::StaleRoot.into());
        }
        let (current, _) = capture_boundary(root)?;
        if live.owned_boundary.as_ref() != Some(&current) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        Ok(())
    }

    /// Read-only observation of Core's current unpinned Assistant stream root.
    /// This grants no execution/adoption or finalized terminal authority; Host
    /// streaming observers recheck around original Gets before cancellation.
    pub fn recheck_stream_graph_root(&self, expected: &ValueHandle) -> Result<(), GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::StaleRoot.into());
        };
        if !root.same_identity(expected) {
            return Err(GraphAdmissionError::StaleRoot.into());
        }
        if live.owned_boundary.is_some() {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        let captured = root
            .heap()
            .capture_own_controls(
                root,
                &CaptureOwnControls {
                    root_scalar_keys: vec!["role".into()],
                    selected_records_root_scalar_keys: Vec::new(),
                    records: None,
                },
            )
            .map_err(GraphAdmissionError::from)?;
        if !required_text(&captured.controls, "role")?.equals_ascii("assistant") {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        Ok(())
    }

    /// Explicit known-never-executed resume prepares this already adopted
    /// owner. The pinned controls stay fixed; argument callbacks use Owned
    /// slots throughout and cannot unpin/reconstruct this same record.
    pub(crate) fn capture_owned_graph_preparation(
        &self,
        values: &crate::provider_context::SessionValues,
    ) -> Result<StagedGraphPreparation, GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::UnsupportedStructure("owned actual prepare requires Graph body").into());
        };
        values.check_handle(root).map_err(GraphAdmissionError::Value)?;
        let profile = Arc::new(capture_final_profile(root)?);
        if live.owned_boundary.as_ref() != Some(&profile.boundary) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        let slots = profile
            .members
            .iter()
            .map(|member| CurrentArgumentsSlot {
                owner: Arc::downgrade(&self.0),
                root: root.clone(),
                boundary: profile.boundary.clone(),
                mode: ArgumentsSlotMode::Owned,
                prepared_origin: None,
                facts: slot_facts(&profile.boundary, member),
            })
            .collect();
        Ok(StagedGraphPreparation { owner: Arc::downgrade(&self.0), root: root.clone(), profile, slots, owned: true })
    }
    /// A non-assistant Graph tail is not a pending typed tool turn. Observe
    /// only its own role and verify that an adopted role has not been changed.
    pub(crate) fn graph_stream_tail_role(&self) -> Result<WireString, GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::UnsupportedStructure("Graph tail role requires a Graph body").into());
        };
        let captured = root
            .heap()
            .capture_own_controls(
                root,
                &CaptureOwnControls {
                    root_scalar_keys: vec!["role".into()],
                    selected_records_root_scalar_keys: Vec::new(),
                    records: None,
                },
            )
            .map_err(GraphAdmissionError::from)?;
        let role = required_text(&captured.controls, "role")?;
        if live.owned_boundary.as_ref().is_some_and(|boundary| boundary.role != role) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        Ok(role)
    }

    /// Core's same-cell stream staging. Unlike public from_graph, controls are
    /// deliberately not pinned before names/content finish streaming.
    pub(crate) fn from_stream_graph(
        root: ValueHandle,
        values: &crate::provider_context::SessionValues,
    ) -> Result<Self, GraphOwnerError> {
        values.check_handle(&root).map_err(GraphAdmissionError::Value)?;
        Ok(Self::new(AgentMessageBody::Graph(root), None))
    }

    pub(crate) fn replace_stream_graph(
        &self,
        root: ValueHandle,
        values: &crate::provider_context::SessionValues,
    ) -> Result<(), GraphOwnerError> {
        values.check_handle(&root).map_err(GraphAdmissionError::Value)?;
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        if live.owned_boundary.is_some() {
            return Err(
                GraphAdmissionError::UnsupportedStructure("Cannot restage an adopted Graph stream record").into()
            );
        }
        match &live.body {
            AgentMessageBody::Graph(current) => {
                values.check_handle(current).map_err(GraphAdmissionError::Value)?;
            }
            _ => {
                return Err(GraphAdmissionError::UnsupportedStructure(
                    "Graph stream staging requires its same Graph cell",
                )
                .into());
            }
        }
        // Only this Core staging method can carry a remembered terminal across
        // Core's retention/abort roots. Public replace_body revokes provenance.
        if let Some(origin) = &mut live.stream_terminal_origin {
            origin.retained = root.clone();
        }
        live.body = AgentMessageBody::Graph(root);
        live.derivation_generation = live.derivation_generation.wrapping_add(1);
        Ok(())
    }

    /// Captures exact terminal Text even for zero calls; never reads arguments
    /// and never adopts or changes the existing canonical record.
    pub(crate) fn capture_staged_graph_preparation(
        &self,
        values: &crate::provider_context::SessionValues,
    ) -> Result<StagedGraphPreparation, GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::UnsupportedStructure("Staged preparation requires a Graph body").into());
        };
        values.check_handle(root).map_err(GraphAdmissionError::Value)?;
        if live.owned_boundary.is_some() {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        let profile = Arc::new(capture_final_profile(root)?);
        let slots = profile
            .members
            .iter()
            .map(|member| {
                let mut facts = slot_facts(&profile.boundary, member);
                facts.root.retain(|fact| fact.key != PropertyKey::from("stopReason"));
                facts.root.push(OwnScalarFact {
                    key: "stopReason".into(),
                    expected: OwnScalar::Text(profile.reason_text.clone()),
                });
                CurrentArgumentsSlot {
                    owner: Arc::downgrade(&self.0),
                    root: root.clone(),
                    boundary: profile.boundary.clone(),
                    mode: ArgumentsSlotMode::Staged,
                    prepared_origin: Some(profile.clone()),
                    facts,
                }
            })
            .collect();
        Ok(StagedGraphPreparation { owner: Arc::downgrade(&self.0), root: root.clone(), profile, slots, owned: false })
    }

    /// Final transform/preparation observation precedes this one boundary pin.
    pub(crate) fn install_final_graph(
        &self,
        root: ValueHandle,
        values: &crate::provider_context::SessionValues,
    ) -> Result<(StopReason, bool), GraphOwnerError> {
        self.install_final_graph_profile(root, values, None).map(|(reason, calls, _)| (reason, calls))
    }

    /// Compares the preparation profile in the same finite final capture before
    /// the single pin. Fresh Owned slots do not perform Get(arguments).
    pub(crate) fn install_prepared_final_graph(
        &self,
        root: ValueHandle,
        values: &crate::provider_context::SessionValues,
        prepared: &StagedGraphPreparation,
    ) -> Result<(StopReason, bool, Vec<CurrentArgumentsSlot>), GraphOwnerError> {
        self.install_final_graph_profile(root, values, Some(prepared))
    }

    fn install_final_graph_profile(
        &self,
        root: ValueHandle,
        values: &crate::provider_context::SessionValues,
        prepared: Option<&StagedGraphPreparation>,
    ) -> Result<(StopReason, bool, Vec<CurrentArgumentsSlot>), GraphOwnerError> {
        values.check_handle(&root).map_err(GraphAdmissionError::Value)?;
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(current) = &live.body else {
            return Err(
                GraphAdmissionError::UnsupportedStructure("Graph finalization requires its same Graph cell").into()
            );
        };
        values.check_handle(current).map_err(GraphAdmissionError::Value)?;
        if let Some(prepared) = prepared {
            if !Weak::ptr_eq(&prepared.owner, &Arc::downgrade(&self.0)) {
                return Err(GraphAdmissionError::StaleOwner.into());
            }
            if !current.same_identity(&prepared.root) || !root.same_identity(&prepared.root) {
                return Err(GraphAdmissionError::StaleRoot.into());
            }
            if live.owned_boundary.is_some() {
                return Err(GraphAdmissionError::BoundaryChanged.into());
            }
        }
        let profile = capture_final_profile(&root)?;
        if prepared.is_some_and(|prepared| !prepared.profile.matches(&profile)) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        if live.owned_boundary.as_ref().is_some_and(|pinned| pinned != &profile.boundary) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        let has_calls = !profile.members.is_empty();
        let slots = if prepared.is_some() {
            profile
                .members
                .iter()
                .map(|member| CurrentArgumentsSlot {
                    owner: Arc::downgrade(&self.0),
                    root: root.clone(),
                    boundary: profile.boundary.clone(),
                    mode: ArgumentsSlotMode::Owned,
                    prepared_origin: prepared.map(|prepared| prepared.profile.clone()),
                    facts: slot_facts(&profile.boundary, member),
                })
                .collect()
        } else {
            Vec::new()
        };
        live.body = AgentMessageBody::Graph(root);
        live.owned_boundary = Some(profile.boundary);
        live.derivation_generation = live.derivation_generation.wrapping_add(1);
        Ok((profile.reason, has_calls, slots))
    }
    pub fn from_graph(root: ValueHandle, source: Option<Arc<Value>>) -> Result<Self, GraphOwnerError> {
        let message = Self::new(AgentMessageBody::Graph(root), source);
        {
            let mut live = message.0.record.write().unwrap_or_else(|poison| poison.into_inner());
            admit_record(&mut live)?;
        }
        Ok(message)
    }

    pub fn current_arguments_slot(&self, member: &ValueHandle) -> Result<CurrentArgumentsSlot, GraphOwnerError> {
        let live = self.0.record.read().unwrap_or_else(|poison| poison.into_inner());
        let AgentMessageBody::Graph(root) = &live.body else {
            return Err(GraphAdmissionError::UnsupportedStructure("Arguments slot requires current Graph body").into());
        };
        if root.heap().heap_token() != member.heap().heap_token() {
            return Err(GraphAdmissionError::ForeignHeap.into());
        }
        let pinned = live
            .owned_boundary
            .as_ref()
            .ok_or(GraphAdmissionError::UnsupportedStructure("Arguments slot requires owned Graph boundary"))?;
        let (current, members) = capture_boundary(root)?;
        if &current != pinned {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        if !members.iter().any(|candidate| candidate.same_identity(member)) {
            return Err(GraphAdmissionError::MissingMember.into());
        }
        Ok(CurrentArgumentsSlot {
            owner: Arc::downgrade(&self.0),
            root: root.clone(),
            boundary: pinned.clone(),
            mode: ArgumentsSlotMode::Owned,
            prepared_origin: None,
            facts: slot_facts(pinned, member),
        })
    }
}

impl CurrentArgumentsSlot {
    pub(crate) fn require_owned_for(&self, owner: &AgentMessage, member: &ValueHandle) -> Result<(), GraphOwnerError> {
        if !matches!(self.mode, ArgumentsSlotMode::Owned) {
            return Err(
                GraphAdmissionError::UnsupportedStructure("actual invoke requires an Owned arguments slot").into()
            );
        }
        if !Weak::ptr_eq(&self.owner, &Arc::downgrade(&owner.0)) {
            return Err(GraphAdmissionError::StaleOwner.into());
        }
        if !self.matches_member(member) {
            return Err(GraphAdmissionError::MissingMember.into());
        }
        self.recheck_current()
    }
    pub(crate) fn matches_member(&self, member: &ValueHandle) -> bool {
        self.facts.member.same_identity(member)
    }
    fn check_record(&self, live: &AgentMessageRecord) -> Result<(), GraphAdmissionError> {
        match &live.body {
            AgentMessageBody::Graph(root) if root.same_identity(&self.root) => {}
            _ => return Err(GraphAdmissionError::StaleRoot),
        }
        match self.mode {
            ArgumentsSlotMode::Owned if live.owned_boundary.as_ref() != Some(&self.boundary) => {
                return Err(GraphAdmissionError::BoundaryChanged);
            }
            ArgumentsSlotMode::Staged if live.owned_boundary.is_some() => {
                return Err(GraphAdmissionError::BoundaryChanged);
            }
            _ => {}
        }
        Ok(())
    }

    #[allow(dead_code)] // Used by the transient foundation, not dispatch admission.
    pub(crate) fn require_owned_rebinding(&self, fresh: &Self) -> Result<(), GraphOwnerError> {
        if !matches!(self.mode, ArgumentsSlotMode::Staged) || !matches!(fresh.mode, ArgumentsSlotMode::Owned) {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        if !self
            .prepared_origin
            .as_ref()
            .zip(fresh.prepared_origin.as_ref())
            .is_some_and(|(before, after)| Arc::ptr_eq(before, after))
            || self.boundary != fresh.boundary
        {
            return Err(GraphAdmissionError::BoundaryChanged.into());
        }
        if !Weak::ptr_eq(&self.owner, &fresh.owner) {
            return Err(GraphAdmissionError::StaleOwner.into());
        }
        if !self.root.same_identity(&fresh.root) {
            return Err(GraphAdmissionError::StaleRoot.into());
        }
        if !self.facts.member.same_identity(&fresh.facts.member) {
            return Err(GraphAdmissionError::MissingMember.into());
        }
        fresh.recheck_current()
    }

    fn operate(
        &self,
        op: GuardedPropertyOp<'_>,
        runtime: &dyn ValueRuntime,
    ) -> Result<GuardedPropertyValue, GraphOwnerError> {
        let owner = self.owner.upgrade().ok_or(GraphAdmissionError::StaleOwner)?;
        let heap = self.root.heap();
        let key = PropertyKey::from("arguments");
        let step = {
            let live = owner.record.read().unwrap_or_else(|poison| poison.into_inner());
            self.check_record(&live)?;
            heap.guarded_property(&self.root, &self.facts, &key, op).map_err(|error| match error {
                GuardedPropertyError::Operation(error) => GraphOwnerError::Native(error),
                error => GraphAdmissionError::from(error).into(),
            })?
        };
        let plan = match step {
            GuardedPropertyStep::Done(value) => return Ok(value),
            GuardedPropertyStep::Deferred(plan) => plan,
        };
        // All owner/heap guards are gone before the one selected callback.
        let callback = heap.run_deferred(plan, runtime);
        let admission = {
            let live = owner.record.read().unwrap_or_else(|poison| poison.into_inner());
            self.check_record(&live).and_then(|()| {
                heap.guarded_property(&self.root, &self.facts, &key, GuardedPropertyOp::Recheck)
                    .map(|_| ())
                    .map_err(GraphAdmissionError::from)
            })
        };
        match (callback, admission) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(GraphOwnerError::Native(error)),
            (callback, Err(reason)) => Err(GraphOwnerError::Admission { reason, callback_failure: callback.err() }),
        }
    }

    /// Re-admit the same owner, root and selected member without reading
    /// `arguments` again. Validation callbacks may have changed controls.
    pub fn recheck_current(&self) -> Result<(), GraphOwnerError> {
        match self.operate(GuardedPropertyOp::Recheck, &ara_rpc::value_heap::NoRuntime)? {
            GuardedPropertyValue::Rechecked => Ok(()),
            _ => Err(GraphAdmissionError::UnsupportedStructure("Arguments recheck did not complete").into()),
        }
    }

    pub fn read(&self, runtime: &dyn ValueRuntime) -> Result<ValueHandle, GraphOwnerError> {
        match self.operate(GuardedPropertyOp::Read, runtime)? {
            GuardedPropertyValue::Read(value) => Ok(value),
            _ => Err(GraphAdmissionError::UnsupportedStructure("Arguments read did not return a value").into()),
        }
    }

    pub fn assign(&self, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<(), GraphOwnerError> {
        match self.operate(GuardedPropertyOp::Assign(value), runtime)? {
            GuardedPropertyValue::Assigned => Ok(()),
            _ => Err(GraphAdmissionError::UnsupportedStructure("Arguments assignment did not complete").into()),
        }
    }
}
