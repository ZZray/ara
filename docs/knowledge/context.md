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

## Native RPC soft compaction provenance

**Implemented boundary, 2026-09-30, `599db7c`:** a chained soft summary uses
the last validated derived summary and the kept/later raw window. Its checked
commit carries previous raw source IDs forward, so the journal retains both
the original observations and a verifiable cumulative replaced prefix. A
derived summary never becomes an invented raw user observation. Shared Core
receives the lower-trust model projection through its existing idle interface;
the Host exposes native `compactionSummary` and owns policy/provider reset.
Missing legacy source provenance is an explicit refusal to update a summary.
Print/REPL keep their earlier V1 scope; full maintenance remains mandatory
reproduction work. [Execution and audit](../evidence/rpc-soft-compaction.md)
record the bounded implementation and remaining behavior.

The same journal writer retains late user Bash on its captured pre-compaction
branch. Fixed OMP restores the active leaf in memory but rebuilds from the last
raw entry on reopen. Preserve that source-backed distinction while reproducing
OMP; changing persisted leaf semantics belongs to later customization.

## Raw entry identity and model fragments

**Implemented boundary, 2026-10-02, fixed OMP `596f2da`:** a raw Session
entry owns one real ID and origin, independently of its zero/one/multiple
provider-visible messages. Session validates and projects that group; Core
borrows it through an explicit Host mapping without a Session dependency.
Native cuts retain metadata IDs and use raw origin for backtracking/turn starts.
Only raw `type=message` contributes to the reverse retention estimate; custom,
branch and metadata entries do not. Excluded Bash can count for that estimate
while contributing no model message or summary source. These extension-role
estimates are text proxies, not full native tokenizer or hard wire-fit evidence.

A summarized context-bearing raw group contributes its ID once. Custom images
remain ordered Developer text plus User image fragments (image-only has a User
fragment); old single-message snapshot APIs explicitly reject multiple fragments.
Context/reopen uses plural projection directly, so a compatibility API error
cannot resurrect the replaced raw prefix. Public RPC history retains the native
custom/hook/branch receipt. Reserved malformed Skill/LoopGuard records fail
before generic conversion. Verified historical LoopGuard/custom Developer
projections can be summarized; a genuine raw Developer message remains protected.
Branch/legacy summary User projections neither create nor answer a real prompt.
New checked native writes enforce raw metadata backtracking; readers also accept
safe older V1 summaries whose kept User follows title metadata. Applying the
new writer rule to all old records would invalidate already accepted history.

[Grouped raw-entry evidence](../evidence/native-raw-entries.md) records the
bounded execution and audit. Full provider-native legacy summary payloads,
attribution/historyRewriteAt, remaining raw roles/token contracts and other
compaction methods remain open; this seam is not full surface acceptance.
