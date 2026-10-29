//! Original-value Responses/Codex stream consumers, fixed OMP 596f2da (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! The prepared-request API is an explicit wire boundary. It does not project
//! canonical Context, implement account leases or accept complete Provider parity.
mod accounting;
mod cutoff;
mod leaves;
mod router;
mod transport;
use crate::actual_provider::HistoryIdHashPort;
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle},
};
pub use router::{GraphResponsesFlavor, GraphResponsesStreamState};
use std::sync::Arc;
pub(crate) use transport::stream_with_request_builder;
pub use transport::{GraphResponsesPreparedRequest, stream_prepared};

/// Fixed Buffer.from(base64)/parseImageMetadata consumer supplied by the Host.
/// None means the actual parser found no metadata, then fixed fallback is PNG.
pub trait ResponsesImageMetadataPort: Send + Sync {
    fn mime_from_base64(&self, data: &WireString) -> Result<Option<WireString>, ValueError>;
}
/// Pricing uses catalog facts owned by the Host. These are original handles;
/// no full transcript/output/usage object is exported to a typed message.
pub trait ResponsesUsagePort: Send + Sync {
    fn populate_and_price(
        &self,
        model: &crate::Model,
        output: &ValueHandle,
        usage: &ValueHandle,
        service_tier: &ValueHandle,
        request_service_tier: Option<&WireString>,
        factory: &crate::graph_values::GraphValueFactory,
    ) -> Result<(), ValueError>;
}
/// Native fixed consumer callbacks. Host supplies exact functions or leaves
/// absent. Callback failures retain their original exceptions and are not
/// converted into invented successful stream events.
pub trait ResponsesStreamHooks: Send + Sync {
    fn first_token(&self) -> Result<(), ValueError> {
        Ok(())
    }
    fn output_item_done(&self, _item: &ValueHandle) -> Result<(), ValueError> {
        Ok(())
    }
    fn completed(&self) -> Result<(), ValueError> {
        Ok(())
    }
}
#[derive(Clone, Default)]
pub struct GraphResponsesOptions {
    pub history_id_hash: Option<Arc<dyn HistoryIdHashPort>>,
    pub image_metadata: Option<Arc<dyn ResponsesImageMetadataPort>>,
    pub usage: Option<Arc<dyn ResponsesUsagePort>>,
    pub hooks: Option<Arc<dyn ResponsesStreamHooks>>,
    pub request_service_tier: Option<WireString>,
    pub sequential_cutoff: bool,
}

pub enum GraphResponsesError {
    Value(ValueError),
    Provider(crate::error::ProviderError),
}
impl From<ValueError> for GraphResponsesError {
    fn from(value: ValueError) -> Self {
        Self::Value(value)
    }
}
impl From<crate::error::ProviderError> for GraphResponsesError {
    fn from(value: crate::error::ProviderError) -> Self {
        Self::Provider(value)
    }
}
