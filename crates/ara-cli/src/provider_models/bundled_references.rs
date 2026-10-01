//! Fixed OMP provider-models/bundled-references.ts.
use crate::model_collapse::SpecRef;
pub use crate::model_identity_wire::{ReferenceResolver, create_bundled_reference_map, to_model_spec};
use ara_rpc::WireString;
use std::{collections::HashMap, sync::Arc};
pub enum ProviderReferenceSource {
    Map(HashMap<WireString, SpecRef>),
    Lazy(Arc<dyn Fn() -> HashMap<WireString, SpecRef> + Send + Sync>),
}
impl From<HashMap<WireString, SpecRef>> for ProviderReferenceSource {
    fn from(value: HashMap<WireString, SpecRef>) -> Self {
        Self::Map(value)
    }
}
pub fn create_reference_resolver(source: impl Into<ProviderReferenceSource>) -> ReferenceResolver {
    match source.into() {
        ProviderReferenceSource::Map(map) => ReferenceResolver::new(map),
        ProviderReferenceSource::Lazy(callback) => ReferenceResolver::lazy(move || callback()),
    }
}
