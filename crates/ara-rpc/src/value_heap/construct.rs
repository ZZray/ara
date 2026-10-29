//! Neutral constructor operations. Host state stays outside the heap.
//! Explicit newTarget preserves original derived class/super construction.

use super::*;

impl ValueHeap {
    pub fn is_constructor(
        self: &Arc<Self>,
        constructor: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        self.admit(constructor)?;
        match self.snapshot(constructor)? {
            ValueSnapshot::Function(CallableTarget::Compiled(key)) => self.compiled_is_constructor(key),
            ValueSnapshot::Function(CallableTarget::Host(capability))
            | ValueSnapshot::Function(CallableTarget::Intrinsic(Intrinsic::NativeMember(capability)))
            | ValueSnapshot::Opaque { kind: OpaqueKind::Function, capability, .. } => self.runtime_bool(
                runtime.resolve_is_constructor(capability, constructor, runtime).map(RuntimeReply::Boolean),
            ),
            ValueSnapshot::Function(CallableTarget::Intrinsic(
                Intrinsic::ObjectConstructor
                | Intrinsic::ArrayConstructor
                | Intrinsic::SetConstructor
                | Intrinsic::MapConstructor,
            )) => Ok(true),
            _ => Ok(false),
        }
    }

    /// General neutral construction seam for the admitted selected domains.
    /// Unsupported Host/opaque constructors never fall back to an intrinsic.
    pub fn construct(
        self: &Arc<Self>,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.construct_with_new_target(constructor, arguments, constructor, runtime)
    }
    pub fn construct_with_new_target(
        self: &Arc<Self>,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        new_target: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(constructor)?;
        self.admit(new_target)?;
        if !constructor.same_identity(new_target) && !self.is_constructor(new_target, runtime)? {
            return Err(ValueError::TypeError("newTarget is not a constructor".into()));
        }
        for argument in arguments {
            self.admit(argument)?;
        }
        match self.snapshot(constructor)? {
            ValueSnapshot::Function(CallableTarget::Compiled(_)) => {
                self.construct_compiled_with_new_target(constructor, arguments, new_target, runtime)
            }
            ValueSnapshot::Function(CallableTarget::Intrinsic(Intrinsic::MapConstructor)) => {
                // Preserve the realm's already selected Intrinsic Map identity.
                // Original GetPrototypeFromConstructor observes newTarget once
                // before branding the receiver and selecting its adder.
                let prototype = self.get(new_target, &"prototype".into(), runtime)?;
                let prototype =
                    if matches!(self.kind(&prototype)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                        prototype
                    } else {
                        self.handle(self.intrinsics.map_proto)
                    };
                let receiver = self.object_with_prototype(Some(&prototype))?;
                let iterable = arguments.first().cloned().unwrap_or(self.undefined()?);
                self.initialize_map_receiver(&receiver, &iterable, runtime)
            }
            ValueSnapshot::Function(CallableTarget::Host(capability))
            | ValueSnapshot::Function(CallableTarget::Intrinsic(Intrinsic::NativeMember(capability)))
            | ValueSnapshot::Opaque { kind: OpaqueKind::Function, capability, .. } => {
                if !self.is_constructor(constructor, runtime)? {
                    return Err(ValueError::TypeError("Value is not a constructor".into()));
                }
                let result = self.runtime_value(
                    runtime
                        .resolve_construct_with_new_target(capability, constructor, new_target, arguments, runtime)
                        .map(RuntimeReply::Value),
                )?;
                if !matches!(self.kind(&result)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                    return Err(ValueError::WrongRuntimeReply("Host Construct must return an object"));
                }
                Ok(result)
            }
            ValueSnapshot::Function(CallableTarget::Intrinsic(
                Intrinsic::ObjectConstructor | Intrinsic::ArrayConstructor | Intrinsic::SetConstructor,
            )) => Err(ValueError::UnsupportedOperation("Selected intrinsic Construct is not installed")),
            _ => Err(ValueError::TypeError("Value is not a constructor".into())),
        }
    }
}
