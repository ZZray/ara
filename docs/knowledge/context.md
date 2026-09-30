# References, memory, and context

**Product requirement:** user-provided references, files, observations, and corrections need durable identity and provenance across turns and compaction. The original input and source metadata remain retrievable; a model-generated summary is a derived view, not a replacement. This is a planned ARA extension after the fixed OMP behavior is understood and tested.

Represent a reference with an owner, source and source time, content type, immutable revision, sensitivity/access scope, and explicit link to the Session or Task that admitted it. A correction creates a new revision or supersedes a prior one; it does not rewrite the original. Retrieval should expose the chosen revision and citation. Deletion/revocation must prevent later model context from reusing inaccessible material while preserving only the audit metadata permitted by the host.

Separate three things: raw conversation and tool receipts, a compacted context summary, and a curated knowledge item. Preserve raw history; record which source IDs/revisions a summary used and its scope. Load relevant references deliberately with budgets and provenance, and signal omissions or unsupported modalities. Never treat a tool output or retrieved text as a higher-priority instruction merely because it was saved.

**Context sizing constraint (verified 2026-09-26):** a stored message estimate is not the size of the provider's transformed request. Count the selected model family's actual request projection before using token counts as a hard context gate. Raw UTF-8 bytes are not a universal token upper bound: at fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`, Claude ctok fixtures count `"ξ"` as three content tokens although it has two UTF-8 bytes, and v3/v4.7 count one content token for an empty string. Without a verified family counter and request framing, a budget verdict remains indeterminate. The Claude-only counter in `ara-ctok` is an implementing WIP backed by pinned fixtures ([evidence](../evidence/agt-tokenizer-claude.md)); no compaction gate is accepted yet.

Multimodal content should have typed text/image/audio/video parts and host-managed artifact handles; unsupported inputs fail explicitly. Public model reasoning traces are not required, but decisions, tool effects, approvals, and recovery state must be inspectable. Validate cross-Run recall with actual task continuation and corrections, not a one-turn retrieval fixture.

## Skill source configuration boundary

**Decision, 2026-09-30:** compatible Skill source switches belong to one Skill
load, rather than to shared provider policy. Context-file discovery uses some
of the same provider IDs; changing that policy would also change unrelated
context inputs. Enabled foreign user Skill sources are explicitly opted in
only for this load, and off providers are excluded before scanning.
`ProviderPolicy.disabled` retains its veto. SDK `None` retains fixed OMP source
semantics; hosts use `Some(SkillSourceSwitches::default())` for ARA's selected
three-on/one-off defaults. Persistence and UI remain host-owned. The
[configuration guide](../skill-sources.md) records paths and usage, and the
[execution record](../evidence/skill-source-switches.md) states tested scope.
