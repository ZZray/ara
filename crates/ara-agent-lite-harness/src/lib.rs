//! Bounded, independently reviewed tasks over the actual Lite `execute` entry.
//!
//! Hosts own identities, credentials, paths, tool authorization and Task acceptance.
//! Tool identity checks and Host settlement declarations are not an OS sandbox or
//! proof of external effects. Dropping a waiter does not stop the worker. Cancellation
//! is cooperative: Core and Host futures are joined before releasing Session ownership.
//! A crashed or ambiguous task leaves a durable Session fence; this crate deliberately
//! supplies no automatic fence reset or replay operation. `resume` continues such a
//! Task only after the Host committed a settled reconciliation of what it left behind.

mod runner;
mod snapshot;
mod stop_review;
mod storage;
mod types;

pub use ara_agent_lite_core as core;
pub use runner::{resume, resume_checked, start, start_with_options, TaskHandle};
pub use snapshot::{budget_review_index_path, budget_snapshot_path};
pub use stop_review::{stop_review_index_path, stop_snapshot_path};
pub use types::*;
