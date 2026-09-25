# References, memory, and context

**Product requirement:** user-provided references, files, observations, and corrections need durable identity and provenance across turns and compaction. The original input and source metadata remain retrievable; a model-generated summary is a derived view, not a replacement. This is a planned ARA extension after the fixed OMP behavior is understood and tested.

Represent a reference with an owner, source and source time, content type, immutable revision, sensitivity/access scope, and explicit link to the Session or Task that admitted it. A correction creates a new revision or supersedes a prior one; it does not rewrite the original. Retrieval should expose the chosen revision and citation. Deletion/revocation must prevent later model context from reusing inaccessible material while preserving only the audit metadata permitted by the host.

Separate three things: raw conversation and tool receipts, a compacted context summary, and a curated knowledge item. Preserve raw history; record which source IDs/revisions a summary used and its scope. Load relevant references deliberately with budgets and provenance, and signal omissions or unsupported modalities. Never treat a tool output or retrieved text as a higher-priority instruction merely because it was saved.

Multimodal content should have typed text/image/audio/video parts and host-managed artifact handles; unsupported inputs fail explicitly. Public model reasoning traces are not required, but decisions, tool effects, approvals, and recovery state must be inspectable. Validate cross-Run recall with actual task continuation and corrections, not a one-turn retrieval fixture.
