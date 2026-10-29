//! TZ-only process.env bridge over the existing opaque property capability.
//! Fixed Bun34cbb9a40 JSEnvironmentVariableMap.cpp:133-158,181-205,330-345,
//! 684-700,783-811. Shared timezone state belongs to the main logical Host;
//! worker-local environment and non-TZ process.env behavior are not implemented.
use super::*;
use ara_rpc::value_heap::{
    OpaqueKind, OpaqueOperation, PropertyDescriptor, PropertyKey, ValueKind,
};

impl NativeDateDomain {
    pub(crate) fn prepare_tz_environment(&self, heap: &Arc<ValueHeap>) -> Result<ValueHandle> {
        if heap.heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        let value = heap.opaque(OpaqueKind::Object, DATE_PORT, self.heap_token)?;
        self.timezone_environment
            .set(value.node_id())
            .map_err(|_| {
                ValueError::UnsupportedOperation("Host TZ environment is already prepared")
            })?;
        Ok(value)
    }
    pub(super) fn resolve_environment(
        &self,
        request: &RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        let RuntimeRequest::Opaque {
            capability,
            target,
            receiver,
            operation,
        } = request
        else {
            return None;
        };
        if *capability != DATE_PORT {
            return None;
        }
        Some((|| {
            self.same_heap(target)?;
            self.same_heap(receiver)?;
            if self.timezone_environment.get().copied() != Some(target.node_id())
                || !target.same_identity(receiver)
            {
                return Err(ValueError::TypeError(
                    "Wrong Host TZ environment receiver".into(),
                ));
            }
            let heap = target.heap();
            let tz = |key: &PropertyKey| -> Result<()> {
                if matches!(key, PropertyKey::String(name) if name.equals_ascii("TZ")) {
                    Ok(())
                } else {
                    Err(ValueError::UnsupportedOperation(
                        "Host process.env bridge supports only TZ",
                    ))
                }
            };
            let set = |key: &PropertyKey, value: &ValueHandle| -> Result<()> {
                tz(key)?;
                // Original ToString can call/throw/reenter. No Host state mutex
                // is held here and the supplied effective runtime owns effects.
                let primitive = heap.to_primitive(value, true, effective)?;
                if heap.kind(&primitive)? == ValueKind::Symbol {
                    return Err(ValueError::TypeError(
                        "Cannot convert a symbol to a string".into(),
                    ));
                }
                let value = heap.to_string(&primitive, effective)?;
                self.host.set_timezone_env(&value)
            };
            match operation {
                OpaqueOperation::Get(key) => {
                    tz(key)?;
                    Ok(RuntimeReply::Value(match self.host.timezone_env()? {
                        Some(value) => heap.string(value)?,
                        None => heap.undefined()?,
                    }))
                }
                OpaqueOperation::Set { key, value } => {
                    set(key, value)?;
                    Ok(RuntimeReply::Unit)
                }
                OpaqueOperation::Define { key, descriptor } => {
                    tz(key)?;
                    let PropertyDescriptor::Data {
                        value,
                        writable: true,
                        enumerable: true,
                        configurable: true,
                    } = descriptor
                    else {
                        return Err(ValueError::TypeError(
                            "'process.env' only accepts a configurable, writable, and enumerable data descriptor"
                                .into(),
                        ));
                    };
                    set(key, value)?;
                    Ok(RuntimeReply::Unit)
                }
                OpaqueOperation::Delete(key) => {
                    tz(key)?;
                    self.host.delete_timezone_env()?;
                    Ok(RuntimeReply::Boolean(true))
                }
                OpaqueOperation::Has(key) => {
                    tz(key)?;
                    Ok(RuntimeReply::Boolean(self.host.timezone_env()?.is_some()))
                }
                OpaqueOperation::OwnDescriptor(key) => {
                    tz(key)?;
                    let descriptor = match self.host.timezone_env()? {
                        Some(value) => Some(PropertyDescriptor::data(heap.string(value)?)),
                        None => None,
                    };
                    Ok(RuntimeReply::Descriptor(descriptor))
                }
                OpaqueOperation::OwnKeys => {
                    Ok(RuntimeReply::Keys(if self.host.timezone_env()?.is_some() {
                        vec!["TZ".into()]
                    } else {
                        Vec::new()
                    }))
                }
                OpaqueOperation::Prototype => {
                    Ok(RuntimeReply::Prototype(Some(heap.object_prototype())))
                }
                _ => Err(ValueError::UnsupportedOperation(
                    "Host TZ environment operation is not implemented",
                )),
            }
        })())
    }
}
