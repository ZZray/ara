//! Fixed OMP RPC transport. This crate does not dispatch commands or own an Agent.
//!
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d, rpc-frame.ts,
//! rpc-input.ts and rpc-mode.ts's ordered stdout writer. See THIRD_PARTY.md.

pub mod frame;
pub mod input;
pub mod json;
pub mod writer;
pub use frame::{EncodedFrames, RpcError, RpcFrameDecoder, RpcFrameEncoder, encode_rpc_frame};
pub use json::{WireString, WireValue};
