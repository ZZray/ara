//! Shared native references for the shallow extra-CA options helpers.
//! Container clones retain handles; spread creates only a fresh outer record.
use super::VariantSpec;
use crate::catalog_discovery::DiscoveryTransport;
use ara_rpc::{WireString, WireValue};
use std::{
    any::Any,
    fmt,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub enum ExtraCaValue {
    Undefined,
    /// An absent array index; iteration reads it as undefined while spread of
    /// the array object omits the index. Never authored as a record property.
    Hole,
    Null,
    Bool(bool),
    Number(f64),
    String(WireString),
    Array(Arc<Mutex<Vec<ExtraCaValue>>>),
    Object(ExtraCaObject),
    Fetch(Arc<dyn DiscoveryTransport>),
    /// Functions and host objects keep the caller's actual shared handle.
    Opaque(Arc<dyn Any + Send + Sync>),
}
impl fmt::Debug for ExtraCaValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undefined => f.write_str("Undefined"),
            Self::Hole => f.write_str("Hole"),
            Self::Null => f.write_str("Null"),
            Self::Bool(v) => v.fmt(f),
            Self::Number(v) => v.fmt(f),
            Self::String(v) => v.fmt(f),
            Self::Array(_) => f.write_str("Array(reference)"),
            Self::Object(_) => f.write_str("Object(reference)"),
            Self::Fetch(_) => f.write_str("Fetch(reference)"),
            Self::Opaque(_) => f.write_str("Opaque(reference)"),
        }
    }
}
#[derive(Clone)]
pub struct ExtraCaObject(Arc<Mutex<ExtraCaSlots>>);
impl fmt::Debug for ExtraCaObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExtraCaObject(reference)")
    }
}
#[derive(Clone)]
struct ExtraCaProperty {
    key: WireString,
    value: ExtraCaValue,
    enumerable: bool,
}
struct ExtraCaSlots {
    fields: Vec<ExtraCaProperty>,
    prototype: Option<ExtraCaObject>,
}
impl Default for ExtraCaObject {
    fn default() -> Self {
        Self::new(None)
    }
}
impl ExtraCaObject {
    pub fn new(prototype: Option<ExtraCaObject>) -> Self {
        Self(Arc::new(Mutex::new(ExtraCaSlots { fields: Vec::new(), prototype })))
    }
    pub fn get(&self, key: &str) -> ExtraCaValue {
        self.get_key(&key.into())
    }
    pub fn get_key(&self, key: &WireString) -> ExtraCaValue {
        self.property_key(key).unwrap_or(ExtraCaValue::Undefined)
    }
    /// Property lookup includes the prototype and retains present undefined.
    pub fn property(&self, key: &str) -> Option<ExtraCaValue> {
        self.property_key(&key.into())
    }
    fn property_key(&self, key: &WireString) -> Option<ExtraCaValue> {
        let slots = self.0.lock().expect("extra CA object poisoned");
        if let Some(property) = slots.fields.iter().find(|property| &property.key == key) {
            return Some(property.value.clone());
        }
        let prototype = slots.prototype.clone();
        drop(slots);
        prototype.and_then(|prototype| prototype.property_key(key))
    }
    pub fn set(&self, key: impl Into<WireString>, value: ExtraCaValue) {
        self.define(key.into(), value, true);
    }
    pub fn define(&self, key: WireString, value: ExtraCaValue, enumerable: bool) {
        let mut slots = self.0.lock().expect("extra CA object poisoned");
        if let Some(property) = slots.fields.iter_mut().find(|property| property.key == key) {
            property.value = value;
            property.enumerable = enumerable;
        } else {
            slots.fields.push(ExtraCaProperty { key, value, enumerable });
        }
    }
    pub fn own_entries(&self) -> Vec<(WireString, ExtraCaValue)> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .expect("extra CA object poisoned")
            .fields
            .iter()
            .filter(|property| property.enumerable)
            .map(|property| (property.key.clone(), property.value.clone()))
            .collect();
        let index = |key: &WireString| {
            let text = key.to_utf8().ok()?;
            let value = text.parse::<u32>().ok()?;
            (value != u32::MAX && value.to_string() == text).then_some(value)
        };
        rows.sort_by(|(a, _), (b, _)| match (index(a), index(b)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
        rows
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl ExtraCaValue {
    pub fn string(value: impl Into<WireString>) -> Self {
        Self::String(value.into())
    }
    pub fn array(values: Vec<Self>) -> Self {
        Self::Array(Arc::new(Mutex::new(values)))
    }
    pub fn object(fields: &[(&str, Self)]) -> Self {
        let record = ExtraCaObject::default();
        for (key, value) in fields {
            record.set(*key, value.clone());
        }
        Self::Object(record)
    }
    pub fn as_object(&self) -> Option<ExtraCaObject> {
        if let Self::Object(value) = self { Some(value.clone()) } else { None }
    }
    pub fn get(&self, key: &str) -> Self {
        match self {
            Self::Object(record) => record.get(key),
            Self::Array(values) => {
                let values = values.lock().expect("extra CA array poisoned");
                if key == "length" {
                    return Self::Number(values.len() as f64);
                }
                key.parse::<usize>()
                    .ok()
                    .filter(|index| index.to_string() == key)
                    .and_then(|index| values.get(index).cloned())
                    .map_or(Self::Undefined, |value| if matches!(value, Self::Hole) { Self::Undefined } else { value })
            }
            Self::String(text) => {
                if key == "length" {
                    return Self::Number(text.len() as f64);
                }
                key.parse::<usize>()
                    .ok()
                    .filter(|index| index.to_string() == key)
                    .and_then(|index| text.units().get(index).copied())
                    .map_or(Self::Undefined, |unit| Self::String(WireString::from_units(vec![unit])))
            }
            _ => Self::Undefined,
        }
    }
    pub fn is_undefined(&self) -> bool {
        matches!(self, Self::Undefined | Self::Hole)
    }
    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Hole | Self::Null)
    }
    pub fn strict_same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Undefined, Self::Undefined) | (Self::Hole, Self::Hole) | (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => Arc::ptr_eq(a, b),
            (Self::Object(a), Self::Object(b)) => a.same_identity(b),
            (Self::Fetch(a), Self::Fetch(b)) => Arc::ptr_eq(a, b),
            (Self::Opaque(a), Self::Opaque(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub fn own_entries(&self) -> Vec<(WireString, Self)> {
        match self {
            Self::Object(record) => record.own_entries(),
            Self::Array(values) => values
                .lock()
                .expect("extra CA array poisoned")
                .iter()
                .enumerate()
                .filter(|(_, value)| !matches!(value, Self::Hole))
                .map(|(index, value)| (index.to_string().into(), value.clone()))
                .collect(),
            Self::String(text) => text
                .units()
                .iter()
                .enumerate()
                .map(|(index, unit)| (index.to_string().into(), Self::String(WireString::from_units(vec![*unit]))))
                .collect(),
            _ => Vec::new(),
        }
    }
    pub fn items(&self) -> Vec<Self> {
        if let Self::Array(values) = self {
            values
                .lock()
                .expect("extra CA array poisoned")
                .iter()
                .map(|value| if matches!(value, Self::Hole) { Self::Undefined } else { value.clone() })
                .collect()
        } else {
            Vec::new()
        }
    }
    pub fn spread(&self) -> ExtraCaObject {
        let record = ExtraCaObject::default();
        for (key, value) in self.own_entries() {
            record.set(key, value);
        }
        record
    }
    pub fn from_variant(value: &VariantSpec) -> Self {
        fn convert(value: &WireValue, path: Vec<WireString>, undefined: &[Vec<WireString>]) -> ExtraCaValue {
            if undefined.contains(&path) {
                return ExtraCaValue::Undefined;
            }
            match value {
                WireValue::Null => ExtraCaValue::Null,
                WireValue::Bool(v) => ExtraCaValue::Bool(*v),
                WireValue::Number(v) => ExtraCaValue::Number(*v),
                WireValue::String(v) => ExtraCaValue::String(v.clone()),
                WireValue::Array(items) => ExtraCaValue::array(
                    items
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let mut next = path.clone();
                            next.push(index.to_string().into());
                            convert(value, next, undefined)
                        })
                        .collect(),
                ),
                WireValue::Object(fields) => {
                    let record = ExtraCaObject::default();
                    for (key, value) in fields {
                        let mut next = path.clone();
                        next.push(key.clone());
                        record.set(key.clone(), convert(value, next, undefined));
                    }
                    ExtraCaValue::Object(record)
                }
            }
        }
        convert(&value.value, Vec::new(), &value.undefined_paths)
    }
    /// Snapshot the serializable view only; helpers keep the native carrier.
    /// Opaque and cyclic edges are omitted from this host projection. It is
    /// not JSON.stringify behavior; the original carrier retains those edges.
    /// This projection is never used to decide identity or perform a spread.
    pub fn to_variant(&self) -> VariantSpec {
        fn convert(
            value: &ExtraCaValue,
            path: Vec<WireString>,
            undefined: &mut Vec<Vec<WireString>>,
            ancestors: &mut Vec<usize>,
        ) -> WireValue {
            let identity = match value {
                ExtraCaValue::Array(value) => Some(Arc::as_ptr(value) as usize),
                ExtraCaValue::Object(value) => Some(Arc::as_ptr(&value.0) as usize),
                _ => None,
            };
            if matches!(
                value,
                ExtraCaValue::Undefined | ExtraCaValue::Hole | ExtraCaValue::Opaque(_) | ExtraCaValue::Fetch(_)
            ) || identity.is_some_and(|id| ancestors.contains(&id))
            {
                undefined.push(path);
                return WireValue::Null;
            }
            if let Some(id) = identity {
                ancestors.push(id);
            }
            let result = match value {
                ExtraCaValue::Null => WireValue::Null,
                ExtraCaValue::Bool(v) => WireValue::Bool(*v),
                ExtraCaValue::Number(v) => WireValue::Number(*v),
                ExtraCaValue::String(v) => WireValue::String(v.clone()),
                ExtraCaValue::Array(_) => WireValue::Array(
                    value
                        .items()
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let mut next = path.clone();
                            next.push(index.to_string().into());
                            convert(value, next, undefined, ancestors)
                        })
                        .collect(),
                ),
                ExtraCaValue::Object(record) => WireValue::Object(
                    record
                        .own_entries()
                        .iter()
                        .map(|(key, value)| {
                            let mut next = path.clone();
                            next.push(key.clone());
                            (key.clone(), convert(value, next, undefined, ancestors))
                        })
                        .collect(),
                ),
                _ => unreachable!(),
            };
            if identity.is_some() {
                ancestors.pop();
            }
            result
        }
        let mut undefined_paths = Vec::new();
        let value = convert(self, Vec::new(), &mut undefined_paths, &mut Vec::new());
        VariantSpec { value, undefined_paths }
    }
}

pub fn with_extra_ca_tls_native(existing: &ExtraCaValue, extra_ca: &WireString, roots: &[WireString]) -> ExtraCaValue {
    let existing_ca = existing.get("ca");
    let mut values = if existing_ca.is_undefined() {
        roots.iter().cloned().map(ExtraCaValue::String).collect()
    } else if matches!(&existing_ca, ExtraCaValue::Array(_)) {
        existing_ca.items()
    } else {
        vec![existing_ca]
    };
    values.push(ExtraCaValue::String(extra_ca.clone()));
    let tls = existing.spread();
    tls.set("ca", ExtraCaValue::array(values));
    ExtraCaValue::Object(tls)
}
/// Implements the two object spreads. The new init/TLS/CA containers retain
/// each unchanged property's actual reference, including cycles and opaque
/// host objects; CA iteration retains elements and fills holes as undefined.
pub fn with_extra_ca_init(init: Option<&ExtraCaValue>, extra_ca: &WireString, roots: &[WireString]) -> ExtraCaValue {
    let undefined = ExtraCaValue::Undefined;
    let init = init.unwrap_or(&undefined);
    let tls = with_extra_ca_tls_native(&init.get("tls"), extra_ca, roots);
    let merged = init.spread();
    merged.set("tls", tls);
    ExtraCaValue::Object(merged)
}
