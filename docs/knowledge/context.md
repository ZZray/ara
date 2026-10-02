# References, memory, and context

## Native archive source and image ownership

**Source-backed implemented boundary, 2026-10-02, fixed OMP `596f2da`:**
an archive image is distinguished from an ordinary User image only after
strict archive/source validation. Its runtime frame marker is derived again
on projection and never persisted as an independent trust claim. Known source
ownership and the original `archiveSourceEntryId` must survive every soft
summary in an archive-to-soft-to-soft chain; absence of legacy sources remains
unknown. Accepting arbitrary preserved images as native archive frames would
change token accounting and promote unsupported raw image summaries.

The Host owns the blob location. Complete content-addressed bytes are published
before their reference enters a Session rewrite; the journal still owns the
history-publication/fail-stop boundary. Missing or malformed blob references
are retained with diagnostics rather than silently erased. Existing canonical
regular files are currently reused without content revalidation.

Threshold rescue uses a configured sizing band, while threshold maintenance
and overflow retry retain distinct success predicates. A smaller published
archive can still lack headroom; its receipt/warning must identify the latest
actual publication. An unpublished attempt must not stamp old history. The
end event precedes one no-progress notice. These facts do not establish full
settings/concurrency or actual model readability. Both initial CAS image tasks
failed strict acceptance; [snapcompact evidence](../evidence/snapcompact.md)
records their results and the mandatory remaining contracts.

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

## Local history rewrite and usage anchors

**Implemented boundary, 2026-10-02, fixed OMP `596f2da`:** a checked local
rewrite keeps full raw identity and source slots separate from provider
projection. Session validates exact snapshot/UTF-16 edits and publishes the
candidate with its recovery discard marker atomically. Before-publication
failure restores the previous in-memory state; after-publication directory durability
failure retains the published candidate and requires Host fail-stop. Treating
both as a generic rollback would leave model and disk history divergent.

Reserved historical notices retain strict native provenance through a versioned
reduction proof; only verified native regions and placeholders may replace them.
Model/public/reopen views expose reduced content, while live event admission
still requires the original native form. This is not a general way to elevate
arbitrary user or tool content into a trusted Developer message.

Fixed usage correction applies only to changed entries strictly before a valid
provider anchor. Pending input and system/tool cost remain in the context floor.
Images/rescue can invalidate an old billed prefix; a Host-local rebase preserves
the rewritten estimate until a fresh valid usage receipt arrives. Subsequent
stale pruning must retain that invalidation, and fresh total-only usage is still
a new report. The rebase never alters original provider usage or claims exact
wire-fit without a native tokenizer/projection. A no-op rescue cannot establish
progress simply because the unchanged estimate fits.

Artifact numeric IDs belong to a selected Session. Explicit Host URI
registration and Removed tombstones precede native resolution. Large artifact
selectors stream through the existing line-window reader; whole internal
materialization retains the fixed 8MiB bound. Native nonpersistent artifact
save exists, but fixed URI recovery resolves disk Sessions only. Full registry
fallback/path-only workflows remain a separate required surface.

[Local reducer evidence](../evidence/local-reducers.md) records source scope,
executed receipts, corrected failures and the remaining contracts. This boundary
does not accept full compaction or the whole Host.

## Handoff document and live cache identity

**Source-backed boundary, 2026-10-02, fixed OMP `596f2da`:** handoff reads
the live base system prompt, normalized tool definitions and current history
through the existing provider transform. A side protocol binding must retain
the live prompt cache key while allocating its own transport Session identity;
using the side identity as the cache key cold-misses the original prefix and
sharing its append-only state can mix side output into ordinary turns. Private
auth is resolved through the original route. The controlled wire family proves
identity separation, not a measured provider cache hit.

The generated document and its resume wrapper have different ownership. The
journal stores the document plus native cumulative file lists; the model view
adds the fixed handoff wrapper. File lists come from the consumed raw prefix
and previous native details, never from parsing generated summary prose. The
retained tail contributes no consumed file operations. Same-Session publication
validates both the native cut and every original raw field before adding one
compaction entry.

Reader-time handoff interruption uses a separate child token. Pending accepted
Abort commands and EOF must be visible even while the serial command owner is
awaiting the side request. The slot and pending-stop count share a lock; the
guard remains installed through publication. EOF does not acquire permission
to cancel ordinary accepted Runs. Typed cancellation must survive the Host
error boundary so a cancelled side request cannot trigger the next model
method or a pending primary call.

[Handoff evidence](../evidence/native-handoff.md) records the bounded execution,
source scope and required gaps, including thinking effort, concurrent manual
RPC, new/switch/branch interruption and native speculative maintenance.

## Native remote replay and readable history

Fixed OMP `596f2da` remote compaction consumes full history, including its recent
tail. Its structural kept-prefix boundary and provider replay-through boundary
therefore differ. Only new raw entries after replay-through follow the checked
opaque carrier; repeated remote requests reuse the previous replacement plus
that new raw tail. Sending the structural kept tail again would duplicate
already absorbed history. The carrier is model-only projection and is never
persisted as a fabricated model response or usage receipt.

Raw originals remain the authority for a disabled/foreign/incompatible route.
Local reducers must stop at the active opaque compaction entry so they do not
modify hidden originals needed by that later readable projection. Native
compaction permits supported pictures while the text serializer keeps its own
limits. Structural validation still checks complete tool pairs and unknown
effects. A checked empty native carrier with no pending calls may continue,
while an ordinary completed Assistant retains the existing queue requirement.

Healthy empty/lazy Sessions are not durable compaction sources, but still allow
first-prompt adoption. Their raw journal must be validated before returning an
empty context, including malformed/invalid/duplicate records. Treating every
missing leaf as a fresh Session can erase the observable damaged-resume error.
Local reducers also validate that raw journal, then report no native protection
boundary for a healthy empty/lazy Session. Requiring a durable compaction
projection here would turn their ordinary no-op into an error before any Run.

[Remote evidence](../evidence/remote-compaction.md), dated 2026-10-02, records
actual repeat/reopen and disabled/foreign flows plus damaged-empty checks and
the generic real task. Full remote parity and its listed native/provider/Host
gaps remain open.
