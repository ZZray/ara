//! Callback-free structural admission and shared ordinary property planning.
//! This carrier seam neither pins an Agent owner nor enables Graph messages.

use super::{
    property::{DeferredSelection, PropertyPlan},
    *,
};

#[derive(Debug, Clone)]
pub enum OwnScalar {
    Text(WireString),
    TextOneOf(Vec<WireString>),
    Bool(bool),
    Missing,
    Undefined,
    Null,
}

/// Finite own-control observations; these grant no parameter operation authority.
#[derive(Debug, Clone)]
pub enum CapturedOwnScalar {
    Missing,
    Text(WireString),
    Bool(bool),
    Undefined,
    Null,
}

#[derive(Debug, Clone)]
pub struct CaptureOwnRecords {
    pub array_key: PropertyKey,
    pub discriminator_key: PropertyKey,
    pub discriminator: WireString,
    pub scalar_keys: Vec<PropertyKey>,
}

#[derive(Debug, Clone)]
pub struct CaptureOwnControls {
    pub root_scalar_keys: Vec<PropertyKey>,
    /// Always admitted; observed only when the same guarded selection is nonempty.
    pub selected_records_root_scalar_keys: Vec<PropertyKey>,
    pub records: Option<CaptureOwnRecords>,
}

#[derive(Debug, Clone)]
pub struct CapturedOwnRecord {
    pub member: ValueHandle,
    pub controls: Vec<(PropertyKey, CapturedOwnScalar)>,
}

#[derive(Debug, Clone)]
pub struct CapturedOwnControls {
    pub root: ValueHandle,
    pub controls: Vec<(PropertyKey, CapturedOwnScalar)>,
    pub records: Vec<CapturedOwnRecord>,
}

#[derive(Debug, Clone)]
pub struct OwnScalarFact {
    pub key: PropertyKey,
    pub expected: OwnScalar,
}

#[derive(Debug, Clone)]
pub struct OrderedOwnRecords {
    pub array_key: PropertyKey,
    pub discriminator_key: PropertyKey,
    pub discriminator: WireString,
    pub records: Vec<Vec<OwnScalarFact>>,
}

#[derive(Debug, Clone)]
pub struct GuardedFacts {
    pub root: Vec<OwnScalarFact>,
    pub ordered: Option<OrderedOwnRecords>,
    pub array_key: PropertyKey,
    pub member: ValueHandle,
}

#[derive(Debug)]
pub enum GuardedPropertyOp<'a> {
    Read,
    Assign(&'a ValueHandle),
    Recheck,
}

#[derive(Debug)]
pub enum GuardedPropertyValue {
    Read(ValueHandle),
    Assigned,
    Rechecked,
}

#[derive(Debug)]
pub enum GuardedPropertyStep {
    Done(GuardedPropertyValue),
    Deferred(DeferredProperty),
}

#[derive(Debug)]
pub enum GuardedPropertyError {
    /// Heap/control admission failed before the requested property operation.
    Value(ValueError),
    /// Ordinary property read/write planning failed after successful admission.
    /// Native catch eligibility remains ValueError::is_native_exception().
    Operation(ValueError),
    MissingMember,
    BoundaryChanged,
    UnsupportedStructure(&'static str),
}

impl From<ValueError> for GuardedPropertyError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}
impl fmt::Display for GuardedPropertyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for GuardedPropertyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Value(error) | Self::Operation(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum DeferredOperation {
    GetCall(CallableTarget),
    SetCall { callable: CallableTarget, value: ValueHandle },
    OpaqueGet { capability: CapabilityId, target: ValueHandle },
    OpaqueSet { capability: CapabilityId, target: ValueHandle, value: ValueHandle },
    ArrayLength { value: ValueHandle, writable: bool },
}

/// Private construction and move-only consumption prevent replay of a captured
/// selected callback. Leases retain the graph, never an Agent/Host record.
#[derive(Debug)]
pub struct DeferredProperty {
    receiver: ValueHandle,
    key: PropertyKey,
    operation: DeferredOperation,
}

impl ValueHeap {
    pub(super) fn materialize_property_plan(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: &PropertyKey,
        plan: PropertyPlan,
    ) -> GuardedPropertyStep {
        match plan {
            PropertyPlan::Read(node) => GuardedPropertyStep::Done(GuardedPropertyValue::Read(self.handle(node))),
            PropertyPlan::Assigned => GuardedPropertyStep::Done(GuardedPropertyValue::Assigned),
            PropertyPlan::Deferred(selection) => {
                let operation = match selection {
                    DeferredSelection::GetCall(callable) => DeferredOperation::GetCall(callable),
                    DeferredSelection::SetCall { callable, value } => {
                        DeferredOperation::SetCall { callable, value: self.handle(value) }
                    }
                    DeferredSelection::OpaqueGet { capability, target } => {
                        DeferredOperation::OpaqueGet { capability, target: self.handle(target) }
                    }
                    DeferredSelection::OpaqueSet { capability, target, value } => DeferredOperation::OpaqueSet {
                        capability,
                        target: self.handle(target),
                        value: self.handle(value),
                    },
                    DeferredSelection::ArrayLength { value, writable } => {
                        DeferredOperation::ArrayLength { value: self.handle(value), writable }
                    }
                };
                GuardedPropertyStep::Deferred(DeferredProperty {
                    receiver: receiver.clone(),
                    key: key.clone(),
                    operation,
                })
            }
        }
    }

    pub(super) fn execute_property_plan(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: &PropertyKey,
        plan: PropertyPlan,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<GuardedPropertyValue> {
        match self.materialize_property_plan(receiver, key, plan) {
            GuardedPropertyStep::Done(value) => Ok(value),
            GuardedPropertyStep::Deferred(plan) => self.run_deferred(plan, runtime),
        }
    }

    /// Caller releases its record/cache/journal guards before invoking this.
    /// Runtime replies and throws enter the existing unique admission boundary.
    /// Caller must separately re-admit its current owner before publication.
    pub fn run_deferred(
        self: &Arc<Self>,
        plan: DeferredProperty,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<GuardedPropertyValue> {
        self.adopt(&plan.receiver)?;
        self.admit_key(&plan.key)?;
        match plan.operation {
            DeferredOperation::GetCall(callable) => {
                self.invoke(callable, &plan.receiver, &[], runtime).map(GuardedPropertyValue::Read)
            }
            DeferredOperation::SetCall { callable, value } => {
                self.adopt(&value)?;
                self.invoke(callable, &plan.receiver, &[value], runtime)?;
                Ok(GuardedPropertyValue::Assigned)
            }
            DeferredOperation::OpaqueGet { capability, target } => {
                self.adopt(&target)?;
                self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: &target,
                    receiver: &plan.receiver,
                    operation: OpaqueOperation::Get(&plan.key),
                }))
                .map(GuardedPropertyValue::Read)
            }
            DeferredOperation::OpaqueSet { capability, target, value } => {
                self.adopt(&target)?;
                self.adopt(&value)?;
                self.runtime_unit(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: &target,
                    receiver: &plan.receiver,
                    operation: OpaqueOperation::Set { key: &plan.key, value: &value },
                }))?;
                Ok(GuardedPropertyValue::Assigned)
            }
            DeferredOperation::ArrayLength { value, writable } => {
                // Only the existing normal Array assignment engine can create
                // this plan. Guarded structural members are ordinary Objects.
                self.adopt(&value)?;
                let length = property::valid_array_length(self.to_number(&value, runtime)?)?;
                self.set_array_length_descriptor(&plan.receiver, length, Some(writable))?;
                Ok(GuardedPropertyValue::Assigned)
            }
        }
    }

    /// Atomically observes declared own scalar controls and optionally the
    /// current ordered selected members. This neither pins an owner nor creates
    /// a parameter slot; callers bind the observations under their own policy.
    /// Missing means own absence only. No inherited lookup or callback occurs.
    pub fn capture_own_controls(
        self: &Arc<Self>,
        root: &ValueHandle,
        request: &CaptureOwnControls,
    ) -> Result<CapturedOwnControls, GuardedPropertyError> {
        let root_node = self.admit(root)?;
        let arena = self.lock()?;
        self.structural_object_locked(&arena, root_node)?;
        for key in &request.root_scalar_keys {
            self.admit_key_locked(&arena, key)?;
        }
        for key in &request.selected_records_root_scalar_keys {
            self.admit_key_locked(&arena, key)?;
        }
        if request.records.is_none() && !request.selected_records_root_scalar_keys.is_empty() {
            return Err(GuardedPropertyError::UnsupportedStructure("Conditional root controls require records"));
        }
        if let Some(records) = &request.records {
            self.admit_key_locked(&arena, &records.array_key)?;
            self.admit_key_locked(&arena, &records.discriminator_key)?;
            for key in &records.scalar_keys {
                self.admit_key_locked(&arena, key)?;
            }
        }
        let mut controls = self.capture_own_scalars_locked(&arena, root_node, &request.root_scalar_keys)?;
        let mut captured_records = Vec::new();
        if let Some(records) = &request.records {
            let content = self
                .structural_own_data_locked(&arena, root_node, &records.array_key)?
                .ok_or(GuardedPropertyError::UnsupportedStructure("Missing own content array"))?;
            let members = self.structural_array_members_locked(&arena, content)?;
            for member in members {
                let discriminator = self
                    .structural_own_data_locked(&arena, member, &records.discriminator_key)?
                    .ok_or(GuardedPropertyError::UnsupportedStructure("Missing own member discriminator"))?;
                let Node::String(text) = arena.get(discriminator)? else {
                    return Err(GuardedPropertyError::UnsupportedStructure(
                        "Member discriminator must be an own string",
                    ));
                };
                if text != &records.discriminator {
                    continue;
                }
                let controls = self.capture_own_scalars_locked(&arena, member, &records.scalar_keys)?;
                captured_records.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                captured_records.push(CapturedOwnRecord { member: self.handle(member), controls });
            }
        }
        if !captured_records.is_empty() {
            controls
                .try_reserve(request.selected_records_root_scalar_keys.len())
                .map_err(|_| ValueError::Allocation)?;
            controls.extend(self.capture_own_scalars_locked(
                &arena,
                root_node,
                &request.selected_records_root_scalar_keys,
            )?);
        }
        Ok(CapturedOwnControls { root: root.clone(), controls, records: captured_records })
    }

    /// Holds exactly one arena guard across structural admission and any
    /// ordinary read/write. No runtime, Host closure or callback is accepted.
    /// Facts must come from an already-authorized owner; this does not capture
    /// or adopt an initial root, especially one without a ToolCall.
    pub fn guarded_property(
        self: &Arc<Self>,
        root: &ValueHandle,
        facts: &GuardedFacts,
        key: &PropertyKey,
        op: GuardedPropertyOp<'_>,
    ) -> Result<GuardedPropertyStep, GuardedPropertyError> {
        let root_node = self.admit(root)?;
        let member = self.admit(&facts.member)?;
        let mut arena = self.lock()?;
        arena.get(root_node)?;
        arena.get(member)?;
        self.admit_key_locked(&arena, key)?;
        self.admit_key_locked(&arena, &facts.array_key)?;
        if let Some(ordered) = &facts.ordered {
            if ordered.array_key != facts.array_key {
                return Err(GuardedPropertyError::UnsupportedStructure("Inconsistent content array facts"));
            }
            self.admit_key_locked(&arena, &ordered.discriminator_key)?;
        }
        let assigned = match &op {
            GuardedPropertyOp::Assign(value) => {
                let node = self.admit(value)?;
                arena.get(node)?;
                Some(node)
            }
            _ => None,
        };
        self.check_own_facts_locked(&arena, root_node, &facts.root)?;
        let content = self
            .structural_own_data_locked(&arena, root_node, &facts.array_key)?
            .ok_or(GuardedPropertyError::UnsupportedStructure("Missing own content array"))?;
        let members = self.structural_array_members_locked(&arena, content)?;
        if !members.contains(&member) {
            return Err(GuardedPropertyError::MissingMember);
        }
        // ToolCall/member structural control domain is intentionally ordinary
        // Object only; Array, Function and opaque/prototype controls stay gaps.
        self.structural_object_locked(&arena, member)?;
        if let Some(ordered) = &facts.ordered {
            let mut selected = 0usize;
            let mut member_selected = false;
            for candidate in members {
                let discriminator = self
                    .structural_own_data_locked(&arena, candidate, &ordered.discriminator_key)?
                    .ok_or(GuardedPropertyError::UnsupportedStructure("Missing own member discriminator"))?;
                let Node::String(text) = arena.get(discriminator)? else {
                    return Err(GuardedPropertyError::UnsupportedStructure(
                        "Member discriminator must be an own string",
                    ));
                };
                if text != &ordered.discriminator {
                    continue;
                }
                let expected = ordered.records.get(selected).ok_or(GuardedPropertyError::BoundaryChanged)?;
                self.check_own_facts_locked(&arena, candidate, expected)?;
                member_selected |= candidate == member;
                selected += 1;
            }
            if selected != ordered.records.len() || !member_selected {
                return Err(GuardedPropertyError::BoundaryChanged);
            }
        }
        let plan = match op {
            GuardedPropertyOp::Recheck => {
                return Ok(GuardedPropertyStep::Done(GuardedPropertyValue::Rechecked));
            }
            GuardedPropertyOp::Read => {
                self.get_property_locked(&mut arena, member, key).map_err(GuardedPropertyError::Operation)?
            }
            GuardedPropertyOp::Assign(_) => {
                let assigned = assigned.ok_or(ValueError::InvalidNode)?;
                self.assign_property_locked(&mut arena, member, key, assigned)
                    .map_err(GuardedPropertyError::Operation)?
            }
        };
        Ok(self.materialize_property_plan(&facts.member, key, plan))
    }

    fn structural_object_locked<'a>(
        &self,
        arena: &'a Arena,
        node: NodeId,
    ) -> Result<&'a ObjectData, GuardedPropertyError> {
        match arena.get(node)? {
            Node::Object(object) => Ok(object),
            _ => Err(GuardedPropertyError::UnsupportedStructure("Structural record must be an ordinary Object")),
        }
    }

    fn structural_own_data_locked(
        &self,
        arena: &Arena,
        node: NodeId,
        key: &PropertyKey,
    ) -> Result<Option<NodeId>, GuardedPropertyError> {
        self.admit_key_locked(arena, key)?;
        let object = self.structural_object_locked(arena, node)?;
        match object.properties.iter().find(|entry| &entry.key == key).map(|entry| &entry.descriptor) {
            None => Ok(None),
            Some(StoredDescriptor::Data { value, .. }) => {
                arena.get(*value)?;
                Ok(Some(*value))
            }
            Some(StoredDescriptor::Accessor { .. }) => {
                Err(GuardedPropertyError::UnsupportedStructure("Structural accessor requires a separate source domain"))
            }
        }
    }

    // The existing fact comparison and initial observation share one own-data
    // scalar shape inspection. Equality borrows; capture alone owns the result.
    fn structural_own_scalar_locked<'a>(
        &self,
        arena: &'a Arena,
        node: NodeId,
        key: &PropertyKey,
    ) -> Result<Option<&'a Node>, GuardedPropertyError> {
        let Some(value) = self.structural_own_data_locked(arena, node, key)? else {
            return Ok(None);
        };
        let value = arena.get(value)?;
        if !matches!(value, Node::String(_) | Node::Boolean(_) | Node::Undefined | Node::Null) {
            return Err(GuardedPropertyError::UnsupportedStructure("Unsupported structural scalar"));
        }
        Ok(Some(value))
    }

    fn capture_own_scalars_locked(
        &self,
        arena: &Arena,
        node: NodeId,
        keys: &[PropertyKey],
    ) -> Result<Vec<(PropertyKey, CapturedOwnScalar)>, GuardedPropertyError> {
        let mut controls = Vec::new();
        controls.try_reserve(keys.len()).map_err(|_| ValueError::Allocation)?;
        for key in keys {
            let scalar = match self.structural_own_scalar_locked(arena, node, key)? {
                None => CapturedOwnScalar::Missing,
                Some(Node::String(text)) => CapturedOwnScalar::Text(text.clone()),
                Some(Node::Boolean(value)) => CapturedOwnScalar::Bool(*value),
                Some(Node::Undefined) => CapturedOwnScalar::Undefined,
                Some(Node::Null) => CapturedOwnScalar::Null,
                _ => return Err(GuardedPropertyError::UnsupportedStructure("Unsupported structural scalar")),
            };
            controls.push((key.clone(), scalar));
        }
        Ok(controls)
    }

    fn check_own_facts_locked(
        &self,
        arena: &Arena,
        node: NodeId,
        facts: &[OwnScalarFact],
    ) -> Result<(), GuardedPropertyError> {
        self.structural_object_locked(arena, node)?;
        for fact in facts {
            if matches!(&fact.expected, OwnScalar::TextOneOf(values) if values.is_empty()) {
                return Err(GuardedPropertyError::UnsupportedStructure("Empty TextOneOf fact"));
            }
            let actual = self.structural_own_scalar_locked(arena, node, &fact.key)?;
            let matches = match actual {
                None => matches!(&fact.expected, OwnScalar::Missing),
                Some(actual) => match actual {
                    Node::String(actual) => match &fact.expected {
                        OwnScalar::Text(expected) => actual == expected,
                        OwnScalar::TextOneOf(expected) => expected.contains(actual),
                        _ => false,
                    },
                    Node::Boolean(actual) => matches!(&fact.expected, OwnScalar::Bool(expected) if actual == expected),
                    Node::Undefined => matches!(&fact.expected, OwnScalar::Undefined),
                    Node::Null => matches!(&fact.expected, OwnScalar::Null),
                    _ => return Err(GuardedPropertyError::UnsupportedStructure("Unsupported structural scalar")),
                },
            };
            if !matches {
                return Err(GuardedPropertyError::BoundaryChanged);
            }
        }
        Ok(())
    }

    fn structural_array_members_locked(
        &self,
        arena: &Arena,
        content: NodeId,
    ) -> Result<Vec<NodeId>, GuardedPropertyError> {
        let Node::Array { object, length, .. } = arena.get(content)? else {
            return Err(GuardedPropertyError::UnsupportedStructure("Content must be an ordinary own Array"));
        };
        let mut members = Vec::new();
        members.try_reserve(*length as usize).map_err(|_| ValueError::Allocation)?;
        for index in 0..*length {
            let descriptor = object
                .properties
                .iter()
                .find(|entry| array_index(&entry.key) == Some(index))
                .map(|entry| &entry.descriptor);
            match descriptor {
                Some(StoredDescriptor::Data { value, .. }) => {
                    self.structural_object_locked(arena, *value)?;
                    members.push(*value);
                }
                Some(StoredDescriptor::Accessor { .. }) => {
                    return Err(GuardedPropertyError::UnsupportedStructure("Structural Array slot is an accessor"));
                }
                None => return Err(GuardedPropertyError::UnsupportedStructure("Sparse structural Array")),
            }
        }
        Ok(members)
    }
}
