//! ara-lite: one local service that holds master/worker tasks and chat in SQLite.
//!
//! Rust port of the Go `tools/ara-lite` tool. The module layout mirrors the Go
//! files; see `docs/port-map.md` for the mapping.

pub mod cli;
pub mod client;
pub mod error;
pub mod help;
pub mod lock;
pub mod mcp;
pub mod profile;
pub mod serve;
pub mod server;
pub mod snapshot;
pub mod store;
pub mod types;
pub mod util;

pub use error::{Error, Result};
