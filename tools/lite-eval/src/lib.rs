//! Library half of `lite-eval`: Evaluation_Runner, Capability_Test_Host and
//! delivery-gate code shared by the CLI and its integration tests.

pub mod common;
pub mod deny;
pub mod digest;
pub mod gate;
pub mod host;
pub mod ledger;
pub mod notify;
pub mod records;
pub mod registry;
pub mod sealed;
pub mod stage;
pub mod workspace;
