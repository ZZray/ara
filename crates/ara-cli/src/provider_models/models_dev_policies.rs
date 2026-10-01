//! Fixed OMP provider-models/models-dev-policies.ts.
use crate::{model_collapse::SpecRef, model_identity_wire::text};
pub fn filter_models_dev_catalog_rows(models: &[SpecRef]) -> Vec<SpecRef> {
    models
        .iter()
        .filter(|model| {
            !super::openai_compat::is_excluded_model(
                &text(model, "provider").unwrap_or_else(|| "".into()),
                &text(model, "id").unwrap_or_else(|| "".into()),
            )
        })
        .cloned()
        .collect()
}
