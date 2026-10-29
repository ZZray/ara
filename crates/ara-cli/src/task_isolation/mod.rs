//! Host-owned Task isolation. Ported from fixed OMP b91db66e task/{worktree,isolation-runner}.ts.
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! A copy is not an OS security sandbox; admission is an exclusive writer/capture capability.
mod capture;
mod owner;
pub mod worktree;

pub use capture::{ApplyReceipt, CaptureReceipt, NestedPatchArtifact};
pub use owner::{IsolationLease, IsolationRequest, LeaseAdmission, PublishPermit, ReleaseReceipt};
