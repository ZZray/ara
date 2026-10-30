//! Library half of `lite-eval`: Evaluation_Runner, Capability_Test_Host and
//! delivery-gate code shared by the CLI and its integration tests.

pub mod baseline;
pub mod common;
pub mod compare;
pub mod deny;
pub mod deps;
pub mod digest;
pub mod failure;
pub mod gate;
pub mod host;
pub mod ledger;
pub mod long_compact;
pub mod long_trial;
pub mod manifest;
pub mod metrics;
pub mod notify;
pub mod policy;
pub mod probe;
pub mod records;
pub mod registry;
pub mod runner;
pub mod sealed;
pub mod stage;
pub mod workspace;
