//! Fixed OMP596f2da session-provider-boundary.ts:53-77 and read.ts:1248-1259.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! Original object edges live only on the Graph driver; workers receive paths.
use super::*;
use ara_rpc::value_heap::{ValueHeap, ValueKind};
use std::path::PathBuf;

// Private import, resolved only by the entered lookup's stack-scoped adapter.
const IMPORT: CapabilityId = CapabilityId(0x41524152454144);
#[derive(Clone)]
pub(super) struct CapturedHistory {
    pub roots: Vec<DeferredValue>,
    pub blobs: PathBuf,
}
impl CapturedHistory {
    pub fn capture(
        messages: &[ara_agent::AgentMessage],
        blobs: PathBuf,
        values: &SessionValues,
    ) -> Result<Self, ActualProviderError> {
        let roots = messages
            .iter()
            .map(|message| match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(root) => {
                    values.check_handle(&root)?;
                    Ok(values.heap().capture_deferred(&root)?)
                }
                _ => Err(ActualProviderError::Unsupported(
                    "Attachment capture requires original Agent Graph roots",
                )),
            })
            .collect::<Result<Vec<_>, ActualProviderError>>()?;
        Ok(Self { roots, blobs })
    }
}
pub(super) fn matches(path: &str) -> bool {
    path.strip_prefix("attachment://").is_some_and(|n| {
        !n.is_empty()
            && matches!(n.as_bytes()[0], b'1'..=b'9')
            && n.bytes().all(|b| b.is_ascii_digit())
    })
}
fn arg(
    heap: &Arc<ValueHeap>,
    args: &[ValueHandle],
    index: usize,
) -> Result<ValueHandle, ValueError> {
    args.get(index)
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| heap.undefined())
}
fn get(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    key: &str,
    r: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.get(object, &key.into(), r)
}
fn method(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    key: &str,
    args: &[ValueHandle],
    r: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let f = get(heap, object, key, r)?;
    heap.call(&f, object, args, r)
}
fn nullish(heap: &Arc<ValueHeap>, v: &ValueHandle) -> Result<bool, ValueError> {
    Ok(matches!(
        heap.kind(v)?,
        ValueKind::Undefined | ValueKind::Null
    ))
}
fn ascii(heap: &Arc<ValueHeap>, v: &ValueHandle, expected: &str) -> Result<bool, ValueError> {
    Ok(matches!(heap.snapshot(v)?, ValueSnapshot::String(s) if s.equals_ascii(expected)))
}
static IMAGE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da session-provider-boundary.ts:58 part => part.type === image",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, r| {
        heap.boolean(ascii(
            heap,
            &get(heap, &arg(heap, args, 0)?, "type", r)?,
            "image",
        )?)
    },
};
static URI: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da read.ts:1253 entry => entry.uri === readPath",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, c, _, args, r| {
        heap.boolean(heap.object_is(&get(heap, &arg(heap, args, 0)?, "uri", r)?, &c[0])?)
    },
};
static URI_MAP: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da read.ts:1255 entry => entry.uri",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, r| get(heap, &arg(heap, args, 0)?, "uri", r),
};
static MATERIALIZE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da session-provider-boundary.ts:60-77 image flatMap callback",
    name: "",
    length: 2,
    form: CompiledFunctionForm::Arrow,
    capture_count: 4,
    body: |heap, c, _, args, r| {
        let image = arg(heap, args, 0)?;
        let index = heap.to_number(&arg(heap, args, 1)?, r)? + 1.0;
        let index = heap.to_string(&heap.number(index)?, r)?;
        let label =
            heap.string(format!("Image #{}", String::from_utf16_lossy(index.units())).into())?;
        let uri = heap
            .string(format!("attachment://{}", String::from_utf16_lossy(index.units())).into())?;
        let attempt = (|| {
            // Decode completes before the MIME Get. Neither operation writes.
            let data = get(heap, &image, "data", r)?;
            let decoded = heap.call(&c[0], &heap.undefined()?, &[heap.number(1.0)?, data], r)?;
            let mime = get(heap, &image, "mimeType", r)?;
            let lower = if heap.truthy(&mime)? {
                method(heap, &mime, "toLowerCase", &[], r)?
            } else {
                heap.undefined()?
            };
            let extension = heap.call(&c[0], &heap.undefined()?, &[heap.number(2.0)?, lower], r)?;
            // Rechecks entered grant/cancel immediately before synchronous IO.
            let blob = heap.call(
                &c[0],
                &heap.undefined()?,
                &[heap.number(3.0)?, decoded, extension],
                r,
            )?;
            let path = get(heap, &blob, "displayPath", r)?;
            let entry = heap.object()?;
            for (name, value) in [
                ("label", label.clone()),
                ("uri", uri),
                ("image", image),
                ("sourcePath", path),
            ] {
                heap.define_own_data(&entry, name.into(), value, r)?;
            }
            heap.array(vec![entry])
        })();
        match attempt {
            Ok(value) => Ok(value),
            Err(error) => {
                // Only ordinary source failures are catchable; infrastructure
                // failure is retained by the admitted Graph owner.
                let reason = match error {
                    ValueError::Thrown(reason) => reason,
                    ValueError::TypeError(message) => heap.call(
                        &c[2],
                        &heap.undefined()?,
                        &[heap.string(message.into())?],
                        r,
                    )?,
                    other => return Err(other),
                };
                // logger.warn Get precedes argument expression evaluation.
                let warn = get(heap, &c[1], "warn", r)?;
                let fields = heap.object()?;
                heap.define_own_data(&fields, "label".into(), label, r)?;
                let text = if heap.instance_of(&reason, &c[2], r)? {
                    get(heap, &reason, "message", r)?
                } else {
                    heap.call(&c[3], &heap.undefined()?, &[reason], r)?
                };
                heap.define_own_data(&fields, "error".into(), text, r)?;
                heap.call(
                    &warn,
                    &c[1],
                    &[
                        heap.string(
                            "failed to materialize image attachment; attachment omitted".into(),
                        )?,
                        fields,
                    ],
                    r,
                )?;
                heap.array(vec![])
            }
        }
    },
};
static GET_ATTACHMENTS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da session-provider-boundary.ts:53-77 getImageAttachments",
    name: "getImageAttachments",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, c, _, _, r| {
        let roots = heap.call(&c[0], &heap.undefined()?, &[heap.number(0.0)?], r)?;
        let ValueSnapshot::Number(length) = heap.snapshot(&get(heap, &roots, "length", r)?)? else {
            return Err(ValueError::WrongRuntimeReply("Attachment roots length"));
        };
        for i in (0..length as u32).rev() {
            let message = get(heap, &roots, &i.to_string(), r)?;
            if !heap.truthy(&message)? {
                continue;
            }
            if !ascii(heap, &get(heap, &message, "role", r)?, "user")?
                && !ascii(heap, &get(heap, &message, "role", r)?, "developer")?
            {
                continue;
            }
            if heap.kind(&get(heap, &message, "content", r)?)? != ValueKind::Array {
                continue;
            }
            let content = get(heap, &message, "content", r)?;
            let predicate = heap.fresh_compiled_function(&IMAGE, &[])?;
            let images = method(heap, &content, "filter", &[predicate], r)?;
            if matches!(heap.snapshot(&get(heap,&images,"length",r)?)?,ValueSnapshot::Number(n) if n==0.0)
            {
                continue;
            }
            let materialize = heap.fresh_compiled_function(&MATERIALIZE, c)?;
            return method(heap, &images, "flatMap", &[materialize], r);
        }
        heap.array(vec![])
    },
};

/// Prepared once on the genuine Session state. Existing custom getter slots
/// remain authoritative; invocation never substitutes an internal scan.
pub(crate) fn install(
    session: &ValueHandle,
    realm: &crate::native_realm::NativeRealm,
    logger: Option<&ValueHandle>,
) -> Result<(), ValueError> {
    let Some(logger) = logger else {
        return Ok(());
    };
    let heap = realm.values.heap();
    if heap
        .own_descriptor(
            session,
            &"getImageAttachments".into(),
            realm.values.runtime(),
        )?
        .is_some()
    {
        return Ok(());
    }
    let getter = heap.fresh_compiled_function(
        &GET_ATTACHMENTS,
        &[
            heap.function(IMPORT)?,
            logger.clone(),
            realm.error_constructor(),
            realm.globals.string.clone(),
        ],
    )?;
    heap.define_own_data(
        session,
        "getImageAttachments".into(),
        getter,
        realm.values.runtime(),
    )
}

struct LookupRuntime<'a> {
    original: &'a dyn ValueRuntime,
    values: &'a SessionValues,
    history: Option<&'a CapturedHistory>,
    guard: &'a (dyn Fn() -> Result<(), ValueError> + Send + Sync),
    error: &'a ValueHandle,
    decoded: Mutex<Vec<Option<Vec<u8>>>>,
}
impl ValueRuntime for LookupRuntime<'_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(
        &self,
        request: RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        let RuntimeRequest::Call {
            capability,
            arguments,
            ..
        } = &request
        else {
            return self.original.resolve_effective(request, effective);
        };
        if *capability != IMPORT {
            return self.original.resolve_effective(request, effective);
        }
        let heap = self.values.heap();
        let ValueSnapshot::Number(kind) = heap.snapshot(&arg(heap, arguments, 0)?)? else {
            return Err(ValueError::WrongRuntimeReply("Attachment import kind"));
        };
        let result = match kind {
            0.0 => {
                (self.guard)()?;
                let history = self.history.ok_or(ValueError::UnsupportedOperation(
                    "Original attachment history has not been captured by the Host",
                ))?;
                heap.array(
                    history
                        .roots
                        .iter()
                        .map(|root| heap.restore_deferred(root))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            1.0 => {
                let data = arg(heap, arguments, 1)?;
                let ValueSnapshot::String(data) = heap.snapshot(&data)? else {
                    return Err(ValueError::TypeError(
                        "Buffer.from image data requires a string".into(),
                    ));
                };
                let bytes =
                    ara_session::blob::decode_image_base64(&String::from_utf16_lossy(data.units()))
                        .map_err(|e| ValueError::TypeError(e.to_string()))?;
                let mut decoded = self.decoded.lock().unwrap_or_else(|e| e.into_inner());
                let index = decoded.len();
                decoded.push(Some(bytes));
                drop(decoded);
                heap.number(index as f64)
            }
            2.0 => {
                let lower = arg(heap, arguments, 1)?;
                if nullish(heap, &lower)? {
                    return Ok(RuntimeReply::Value(heap.undefined()?));
                }
                let mapping = heap.object()?;
                for (mime, ext) in [
                    ("image/png", "png"),
                    ("image/jpeg", "jpg"),
                    ("image/jpg", "jpg"),
                    ("image/gif", "gif"),
                    ("image/webp", "webp"),
                    ("image/svg+xml", "svg"),
                ] {
                    heap.define_own_data(
                        &mapping,
                        mime.into(),
                        heap.string(ext.into())?,
                        effective,
                    )?;
                }
                let key = heap.to_primitive(&lower, true, effective)?;
                let key = if let ValueSnapshot::Symbol { key, .. } = heap.snapshot(&key)? {
                    PropertyKey::Symbol(key)
                } else {
                    PropertyKey::String(heap.to_string(&key, effective)?)
                };
                let known = heap.get(&mapping, &key, effective)?;
                if heap.truthy(&known)? {
                    return Ok(RuntimeReply::Value(known));
                }
                if !heap.truthy(&method(
                    heap,
                    &lower,
                    "startsWith",
                    &[heap.string("image/".into())?],
                    effective,
                )?)? {
                    return Ok(RuntimeReply::Value(heap.undefined()?));
                }
                let tail = method(heap, &lower, "slice", &[heap.number(6.0)?], effective)?;
                let parts = method(heap, &tail, "split", &[heap.string(";".into())?], effective)?;
                let first = get(heap, &parts, "0", effective)?;
                let subtype = if nullish(heap, &first)? {
                    heap.undefined()?
                } else {
                    let parts = method(
                        heap,
                        &first,
                        "split",
                        &[heap.string("+".into())?],
                        effective,
                    )?;
                    get(heap, &parts, "0", effective)?
                };
                if !heap.truthy(&subtype)? {
                    return Ok(RuntimeReply::Value(heap.undefined()?));
                }
                let normalized = if heap.truthy(&method(
                    heap,
                    &subtype,
                    "startsWith",
                    &[heap.string(".".into())?],
                    effective,
                )?)? {
                    method(heap, &subtype, "slice", &[heap.number(1.0)?], effective)?
                } else {
                    subtype
                };
                let length = get(heap, &normalized, "length", effective)?;
                let length = heap.to_number(&length, effective)?;
                if length == 0.0 || length > 32.0 {
                    return Ok(RuntimeReply::Value(heap.undefined()?));
                }
                let normalized_string = heap.to_string(&normalized, effective)?;
                let units = normalized_string.units();
                let valid = units
                    .first()
                    .is_some_and(|u| matches!(*u,48..=57|65..=90|97..=122))
                    && units
                        .iter()
                        .all(|u| matches!(*u,48..=57|65..=90|97..=122|46|95|45));
                if !valid {
                    return Ok(RuntimeReply::Value(heap.undefined()?));
                }
                method(heap, &normalized, "toLowerCase", &[], effective)
            }
            3.0 => {
                let ValueSnapshot::Number(index) = heap.snapshot(&arg(heap, arguments, 1)?)? else {
                    return Err(ValueError::WrongRuntimeReply("Attachment decoded slot"));
                };
                let bytes = self
                    .decoded
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(index as usize)
                    .and_then(Option::take)
                    .ok_or(ValueError::WrongRuntimeReply(
                        "Attachment decoded bytes expired",
                    ))?;
                let extension = match heap.snapshot(&arg(heap, arguments, 2)?)? {
                    ValueSnapshot::String(e) => Some(String::from_utf16_lossy(e.units())),
                    ValueSnapshot::Undefined => None,
                    _ => return Err(ValueError::WrongRuntimeReply("Attachment extension")),
                };
                let history = self.history.ok_or(ValueError::UnsupportedOperation(
                    "Attachment BlobStore Host path is absent",
                ))?;
                (self.guard)()?;
                let blob = match ara_session::blob::BlobStore::new(history.blobs.clone())
                    .put(&bytes, extension.as_deref())
                {
                    Ok(blob) => blob,
                    Err(error) => {
                        return Err(ValueError::Thrown(heap.call(
                            self.error,
                            &heap.undefined()?,
                            &[heap.string(error.to_string().into())?],
                            effective,
                        )?));
                    }
                };
                let result = heap.object()?;
                heap.define_own_data(
                    &result,
                    "displayPath".into(),
                    heap.string(blob.display_path.to_string_lossy().into_owned().into())?,
                    effective,
                )?;
                Ok(result)
            }
            _ => Err(ValueError::WrongRuntimeReply("Unknown attachment import")),
        }?;
        Ok(RuntimeReply::Value(result))
    }
}

/// Exact optional getter/receiver/find/map/join order from fixed read.ts.
/// Native error text is returned separately from original thrown carriers.
pub(super) fn lookup(
    values: &SessionValues,
    source: &ValueHandle,
    requested: &str,
    history: Option<&CapturedHistory>,
    error: &ValueHandle,
    original: &dyn ValueRuntime,
    guard: &(dyn Fn() -> Result<(), ValueError> + Send + Sync),
) -> Result<Result<String, String>, ValueError> {
    let adapter = LookupRuntime {
        original,
        values,
        history,
        guard,
        error,
        decoded: Mutex::new(vec![]),
    };
    let heap = values.heap();
    let r = &adapter;
    guard()?;
    let session = get(heap, source, "session", r)?;
    let getter = get(heap, &session, "getImageAttachments", r)?;
    let result = if nullish(heap, &getter)? {
        heap.undefined()?
    } else {
        heap.call(&getter, &session, &[], r)?
    };
    let attachments = if nullish(heap, &result)? {
        heap.array(vec![])?
    } else {
        result
    };
    let predicate = heap.fresh_compiled_function(&URI, &[heap.string(requested.into())?])?;
    let selected = method(heap, &attachments, "find", &[predicate], r)?;
    if !heap.truthy(&selected)? {
        let mapper = heap.fresh_compiled_function(&URI_MAP, &[])?;
        let uris = method(heap, &attachments, "map", &[mapper], r)?;
        let listed = method(heap, &uris, "join", &[heap.string(", ".into())?], r)?;
        let listed = if heap.truthy(&listed)? {
            String::from_utf16_lossy(heap.to_string(&listed, r)?.units())
        } else {
            "none".into()
        };
        return Ok(Err(format!(
            "Could not resolve image attachment '{requested}'. Available attachment URIs: {listed}. Use one of the listed attachment URIs, or attach an image first when none are available."
        )));
    }
    let path = get(heap, &selected, "sourcePath", r)?;
    let ValueSnapshot::String(path) = heap.snapshot(&path)? else {
        return Err(ValueError::TypeError(
            "Attachment sourcePath must be a string".into(),
        ));
    };
    Ok(Ok(path.to_utf8().map_err(|_| {
        ValueError::UnsupportedOperation("Attachment sourcePath UTF16 OS boundary")
    })?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_realm::NativeRealm;
    static CUSTOM: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "READ original getter receiver/carrier regression",
        name: "getImageAttachments",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 3,
        body: |heap, c, receiver, _, _| {
            assert!(receiver.same_identity(&c[0]));
            if !nullish(heap, &c[2])? {
                return Err(ValueError::Thrown(c[2].clone()));
            }
            Ok(c[1].clone())
        },
    };
    static WARN: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "READ warn1 mutates data2 before its Get; original warn carrier",
        name: "warn",
        length: 2,
        form: CompiledFunctionForm::Builtin,
        capture_count: 4,
        body: |heap, c, receiver, args, r| {
            assert!(receiver.same_identity(&c[0]));
            let fields = arg(heap, args, 1)?;
            heap.array_push(&c[1], &[get(heap, &fields, "label", r)?], r)?;
            if !nullish(heap, &c[2])? {
                heap.assign(&c[2], "data".into(), heap.string("Qg==".into())?, r)?;
            }
            if !nullish(heap, &c[3])? {
                return Err(ValueError::Thrown(c[3].clone()));
            }
            heap.undefined()
        },
    };
    fn source(realm: &NativeRealm) -> (ValueHandle, ValueHandle) {
        let heap = realm.values.heap();
        let source = heap.object().unwrap();
        let session = heap.object().unwrap();
        heap.define_own_data(
            &source,
            "session".into(),
            session.clone(),
            realm.values.runtime(),
        )
        .unwrap();
        (source, session)
    }
    fn image(realm: &NativeRealm, data: ValueHandle) -> ValueHandle {
        let heap = realm.values.heap();
        let image = heap.object().unwrap();
        for (key, value) in [
            ("type", heap.string("image".into()).unwrap()),
            ("data", data),
            ("mimeType", heap.string("image/png".into()).unwrap()),
        ] {
            heap.define_own_data(&image, key.into(), value, realm.values.runtime())
                .unwrap();
        }
        image
    }
    fn message(realm: &NativeRealm, role: &str, images: Vec<ValueHandle>) -> DeferredValue {
        let heap = realm.values.heap();
        let message = heap.object().unwrap();
        heap.define_own_data(
            &message,
            "role".into(),
            heap.string(role.into()).unwrap(),
            realm.values.runtime(),
        )
        .unwrap();
        heap.define_own_data(
            &message,
            "content".into(),
            heap.array(images).unwrap(),
            realm.values.runtime(),
        )
        .unwrap();
        heap.capture_deferred(&message).unwrap()
    }
    #[test]
    fn read_attachment_original_optional_getter_receiver_and_carrier() {
        let realm = NativeRealm::prepare().unwrap();
        let heap = realm.values.heap();
        let (source, session) = source(&realm);
        let entry = heap
            .import_serde(&serde_json::json!({"uri":"attachment://1","sourcePath":"literal.png"}))
            .unwrap();
        let returned = heap.array(vec![entry]).unwrap();
        let reason = heap.object().unwrap();
        let getter = heap
            .fresh_compiled_function(
                &CUSTOM,
                &[session.clone(), returned, heap.undefined().unwrap()],
            )
            .unwrap();
        heap.define_own_data(
            &session,
            "getImageAttachments".into(),
            getter,
            realm.values.runtime(),
        )
        .unwrap();
        let guard = || Ok(());
        assert_eq!(
            lookup(
                &realm.values,
                &source,
                "attachment://1",
                None,
                &realm.error_constructor(),
                realm.values.runtime(),
                &guard
            )
            .unwrap(),
            Ok("literal.png".into())
        );
        let absent = lookup(
            &realm.values,
            &source,
            "attachment://2",
            None,
            &realm.error_constructor(),
            realm.values.runtime(),
            &guard,
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(
            absent,
            "Could not resolve image attachment 'attachment://2'. Available attachment URIs: attachment://1. Use one of the listed attachment URIs, or attach an image first when none are available."
        );
        let getter = heap
            .fresh_compiled_function(
                &CUSTOM,
                &[session.clone(), heap.undefined().unwrap(), reason.clone()],
            )
            .unwrap();
        heap.assign(
            &session,
            "getImageAttachments".into(),
            getter,
            realm.values.runtime(),
        )
        .unwrap();
        assert!(
            matches!(lookup(&realm.values,&source,"attachment://1",None,&realm.error_constructor(),realm.values.runtime(),&guard),Err(ValueError::Thrown(actual))if actual.same_identity(&reason))
        );
        heap.assign(
            &session,
            "getImageAttachments".into(),
            heap.null().unwrap(),
            realm.values.runtime(),
        )
        .unwrap();
        assert!(
            lookup(
                &realm.values,
                &source,
                "attachment://1",
                None,
                &realm.error_constructor(),
                realm.values.runtime(),
                &guard
            )
            .unwrap()
            .unwrap_err()
            .contains("URIs: none.")
        );
        for path in [
            "attachment://0",
            "attachment://-1",
            "attachment://01",
            "attachment://1:raw",
            "attachment://١",
            "ATTACHMENT://1",
        ] {
            assert!(!matches(path));
        }
        assert!(matches("attachment://999999999999999999999999999999999"));
    }
    #[test]
    fn read_attachment_default_warn_mutation_indices_allfail_and_carrier() {
        let realm = NativeRealm::prepare().unwrap();
        let heap = realm.values.heap();
        let draft =
            crate::native_factory::PreparedNativeSession::from_realm(realm.clone(), None).unwrap();
        let session = draft.session_state().clone();
        assert!(
            heap.own_descriptor(
                &session,
                &"getImageAttachments".into(),
                realm.values.runtime()
            )
            .unwrap()
            .is_none()
        );
        let source = heap.object().unwrap();
        heap.define_own_data(
            &source,
            "session".into(),
            session.clone(),
            realm.values.runtime(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bad = image(&realm, heap.number(123.0).unwrap());
        let later = image(&realm, heap.string("QQ==".into()).unwrap());
        let logger = heap.object().unwrap();
        let warnings = heap.array(vec![]).unwrap();
        let reason = heap.object().unwrap();
        let warn = heap
            .fresh_compiled_function(
                &WARN,
                &[
                    logger.clone(),
                    warnings.clone(),
                    later.clone(),
                    heap.undefined().unwrap(),
                ],
            )
            .unwrap();
        heap.define_own_data(&logger, "warn".into(), warn, realm.values.runtime())
            .unwrap();
        install(&session, &realm, Some(&logger)).unwrap();
        let history = CapturedHistory {
            roots: vec![
                message(&realm, "user", vec![later.clone()]),
                message(&realm, "developer", vec![bad.clone(), later.clone()]),
                message(&realm, "assistant", vec![]),
            ],
            blobs: dir.path().join("blobs"),
        };
        let guard = || Ok(());
        let path = lookup(
            &realm.values,
            &source,
            "attachment://2",
            Some(&history),
            &realm.error_constructor(),
            realm.values.runtime(),
            &guard,
        )
        .unwrap()
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"B");
        let missing = lookup(
            &realm.values,
            &source,
            "attachment://1",
            Some(&history),
            &realm.error_constructor(),
            realm.values.runtime(),
            &guard,
        )
        .unwrap()
        .unwrap_err();
        assert!(missing.contains("URIs: attachment://2."));
        let allfail = CapturedHistory {
            roots: vec![
                message(&realm, "user", vec![later.clone()]),
                message(&realm, "developer", vec![bad]),
            ],
            blobs: history.blobs.clone(),
        };
        assert!(
            lookup(
                &realm.values,
                &source,
                "attachment://1",
                Some(&allfail),
                &realm.error_constructor(),
                realm.values.runtime(),
                &guard
            )
            .unwrap()
            .unwrap_err()
            .contains("URIs: none.")
        );
        let warn = heap
            .fresh_compiled_function(&WARN, &[logger.clone(), warnings, later, reason.clone()])
            .unwrap();
        heap.assign(&logger, "warn".into(), warn, realm.values.runtime())
            .unwrap();
        assert!(
            matches!(lookup(&realm.values,&source,"attachment://1",Some(&allfail),&realm.error_constructor(),realm.values.runtime(),&guard),Err(ValueError::Thrown(actual))if actual.same_identity(&reason))
        );
    }
}
