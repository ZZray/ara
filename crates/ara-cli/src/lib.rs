//! Host-owned model routes and native configuration for the ARA reference host.
//!
//! Account state, catalogue selection and credentials belong here; the reusable
//! Agent and AI crates retain their host-independent ports.

#![recursion_limit = "256"]

pub mod auth_broker_usage;
pub mod auth_storage;
pub mod auth_storage_policy;
pub mod auth_storage_registry;
pub mod auth_storage_state;
pub mod bun_hash;
pub mod catalog_behavior;
pub mod catalog_discovery;
pub mod catalog_extra_ca;
pub mod catalog_proto_schemas;
pub mod catalog_protobuf;
pub mod catalog_rules;
mod catalog_tls;
pub mod codex_usage;
pub mod config_request_auth;
pub mod context_budget;
pub mod credential_store;
pub mod custom_models;
pub mod daily_model_config;
pub mod handoff;
pub mod js_regex;
pub mod local_reduction;
pub mod model_cache;
pub mod model_catalog;
pub mod model_collapse;
pub mod model_config_file;
pub mod model_config_values;
pub mod model_identity;
pub mod model_identity_wire;
pub mod model_manager;
pub mod model_patch;
pub mod model_policy;
pub mod model_registry;
pub mod model_registry_discovery;
pub mod model_registry_extensions;
pub mod model_registry_loader;
pub mod model_registry_runtime;
pub mod model_route;
pub mod model_wire_policy;
pub mod models_config;
pub mod native_compaction;
pub mod openai_codex_auth;
pub mod provider_model_reference;
pub mod provider_models;
pub mod remote_compaction;
mod request_auth_retry;
pub mod retry_fallback;
pub mod session_artifacts;
pub mod session_usage_headers;
pub mod snapcompact;
pub mod static_model_registry;
