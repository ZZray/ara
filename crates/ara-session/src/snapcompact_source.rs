//! Exact fixed-OMP compaction preserveData owner. This is durable data, not a
//! live Agent message or a grant imported from a previous Host realm.
use crate::{BlobStore, native_message};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, value::RawValue};
use std::{io, sync::Arc};

#[derive(Clone, Debug)]
pub struct NativeSnapcompactArchiveSource(Arc<WireValue>);

impl PartialEq for NativeSnapcompactArchiveSource {
    fn eq(&self, other: &Self) -> bool {
        self.0.deep_equal(&other.0)
    }
}
impl Eq for NativeSnapcompactArchiveSource {}

impl NativeSnapcompactArchiveSource {
    pub fn from_wire(value: WireValue) -> Self {
        Self(Arc::new(value))
    }
    pub fn from_raw(value: &RawValue) -> io::Result<Self> {
        WireValue::parse(value.get()).map(Self::from_wire).map_err(|error| native_message::invalid(error.to_string()))
    }
    pub fn exact_preserve_json(&self) -> io::Result<Box<RawValue>> {
        RawValue::from_string(self.0.stringify()).map_err(|error| native_message::invalid(error.to_string()))
    }
    pub fn exact_archive(&self) -> io::Result<Option<ara_snapcompact::ArchiveUtf16>> {
        ara_snapcompact::get_preserved_archive_utf16(Some(&self.exact_preserve_json()?))
            .map_err(|error| native_message::invalid(error.to_string()))
    }
    pub fn checked_preserve(&self) -> io::Result<Value> {
        native_message::checked_serde(&self.0)
    }
    pub fn observe(&self) -> &WireValue {
        &self.0
    }
    pub(crate) fn envelope(raw: &Value) -> Value {
        let mut raw = raw.clone();
        if let Some(fields) = raw.as_object_mut() {
            fields.remove("preserveData");
        }
        raw
    }
    pub(crate) fn checked_entry(&self, raw: &Value) -> io::Result<Value> {
        native_message::checked_serde(&self.wire_entry(raw)?)
    }
    pub(crate) fn wire_entry(&self, raw: &Value) -> io::Result<WireValue> {
        let mut value = native_message::wire_from_serde(&Self::envelope(raw))?;
        value.insert("preserveData", native_message::clone_wire(&self.0));
        Ok(value)
    }
    pub(crate) fn encoded_line(&self, raw: &Value, blobs: Option<&BlobStore>) -> io::Result<String> {
        let value = self.wire_entry(raw)?;
        let value = match blobs {
            Some(store) => crate::blob::prepare_native_entry(&value, store)?,
            None => value,
        };
        Ok(format!("{}\n", value.stringify()))
    }
    /// Keep legacy healing local to frames; never checked-project all archive
    /// metadata merely to decide whether an old frame was truncated.
    pub(crate) fn repair_truncated(&self) -> io::Result<Self> {
        const NOTICE: &str = "\n\n[Session persistence truncated large content]";
        let Some(archive) = self.exact_archive()? else {
            return Ok(self.clone());
        };
        let notice = NOTICE.encode_utf16().collect::<Vec<_>>();
        if !archive.frames.iter().any(|frame| frame.data.as_units().ends_with(&notice)) {
            return Ok(self.clone());
        }
        let mut preserve = native_message::clone_wire(&self.0);
        let WireValue::Object(fields) = &mut preserve else {
            return Ok(self.clone());
        };
        let Some((_, snapcompact)) = fields.iter_mut().find(|(key, _)| key.equals_ascii("snapcompact")) else {
            return Ok(self.clone());
        };
        if let Some(text) = archive.text.filter(|text| !text.is_empty()) {
            snapcompact.insert("frames", WireValue::Array(Vec::new()));
            snapcompact.insert("textHead", WireValue::String(WireString::from_units(text.as_units().to_vec())));
            snapcompact.insert("textTail", WireValue::String("".into()));
        } else {
            let frames = archive
                .frames
                .into_iter()
                .filter(|frame| !frame.data.as_units().ends_with(&notice))
                .map(|frame| {
                    WireValue::parse(frame.source.get()).map_err(|error| native_message::invalid(error.to_string()))
                })
                .collect::<io::Result<Vec<_>>>()?;
            snapcompact.insert("frames", WireValue::Array(frames));
        }
        Ok(Self::from_wire(preserve))
    }
}

/// Extract before a checked serde envelope projection can reject a lone unit.
/// Archive-shaped state and non-scalar retained metadata remain owned,
/// including malformed archives. Scalar remote replay retains its existing
/// source/proof path; later admission applies the same method/shape checks.
pub(crate) fn take_entry_source(entry: &mut WireValue) -> Option<NativeSnapcompactArchiveSource> {
    take_entry_source_mode(entry, true)
}

pub(crate) fn take_owned_entry_source(entry: &mut WireValue) -> Option<NativeSnapcompactArchiveSource> {
    take_entry_source_mode(entry, false)
}

fn take_entry_source_mode(entry: &mut WireValue, archive_only: bool) -> Option<NativeSnapcompactArchiveSource> {
    if !entry.get("type").and_then(WireValue::as_string).is_some_and(|kind| kind.equals_ascii("compaction")) {
        return None;
    }
    let WireValue::Object(fields) = entry else {
        return None;
    };
    let index = fields.iter().position(|(key, _)| key.equals_ascii("preserveData"))?;
    if matches!(&fields[index].1, WireValue::Null) {
        return None;
    }
    if archive_only
        && fields[index].1.get("snapcompact").is_none()
        && native_message::checked_serde(&fields[index].1).is_ok()
    {
        return None;
    }
    Some(NativeSnapcompactArchiveSource::from_wire(fields.remove(index).1))
}
