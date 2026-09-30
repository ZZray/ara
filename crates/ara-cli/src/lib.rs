//! Host-owned model routes and native configuration for the ARA reference host.
//!
//! Account state, catalogue selection and credentials belong here; the reusable
//! Agent and AI crates retain their host-independent ports.

pub mod credential_store;
pub mod model_cache;
pub mod model_catalog;
pub mod model_config_file;
pub mod model_route;
pub mod models_config;
pub mod retry_fallback;
