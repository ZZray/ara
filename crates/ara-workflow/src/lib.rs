//! Bounded, Host-reviewed workflows over the actual shared ARA Agent.
//!
//! [`WorkflowHost`] supplies the execution wrapper and authoritative effect,
//! artifact and persistence review. This crate does not own Task acceptance,
//! permissions, storage, native Session recovery, compaction, or billing.
//! A workflow requires exclusive Host ownership of one Session generation.
//! The started worker survives a dropped waiter; cancellation is explicit.

mod driver;
mod types;

pub use driver::{ReconciliationFence, Workflow, WorkflowHandle};
pub use types::*;
