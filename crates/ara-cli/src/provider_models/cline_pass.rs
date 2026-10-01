//! Fixed OMP provider-models/cline-pass.ts exact authored metadata snapshot.
use crate::model_collapse::VariantSpec;
use ara_rpc::WireString;
pub static CLINE_PASS_MODEL_METADATA: std::sync::LazyLock<VariantSpec> =
    std::sync::LazyLock::new(|| super::static_data::fixed_literal("CLINE_PASS_MODEL_METADATA"));
pub fn get_cline_pass_model_metadata(id: &WireString) -> Option<VariantSpec> {
    super::static_data::fixed_literal("CLINE_PASS_MODEL_METADATA").record_key(id)
}
