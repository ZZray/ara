# Compact node context and memory

## Requirement and ownership

The node must preserve a user's supplied text independently of a derived model
summary, retrieve relevant information across runs, and distinguish a proposed
memory from an adopted one. This is a HandWave Host extension, not fixed OMP
parity or a claim about the Shared ARA Core.

The native identity boundary is controller fingerprint plus canonical working
directory, with a distinct validated Web workspace/session key for `session-v1`
clients. A new Web key selects its own `ui-<hash>.json`; it neither falls back to
the old cwd checkpoint nor copies its history. Legacy clients retain their old
path contract. A shared Ray QR does not identify individual people. The first
implementation uses that existing scope and does not claim personal-account
isolation. Ordinary shell tools run as the node's OS account: file permissions
cannot prevent that same account from modifying its own memory files.

## Selected first slice

The bounded slice is verified on the final Windows/Linux release executables,
with actual Windows public-HTTPS/model acceptance. See the
[exact artifacts, tests and remaining limits](../evidence/context-memory-20261014.md).

Keep the existing native transcript, route identity, tool-pair validation,
duplicate-run check and incomplete-run fence. Add a versioned, bounded sidecar
using dependencies already present in the small executable. Do not pull in the
rpi harness, SQLite, embeddings, a vector service or a Git runtime.

- References preserve user objective, context brief and admitted steering text
  as distinct sourced records, with hashes, run identity and revisions. Model
  grants are redacted before persistence; this security exception is explicit.
  Corrections supersede a record rather than overwriting its original text.
- A model-only working view carries an explicitly derived summary, a source
  directory, selected adopted memories and recent complete tool rounds. A
  summary never replaces original messages, becomes a real user observation or
  gains System authority.
- Memory proposals carry fact/preference/decision/lesson kind, actual source
  IDs and content hashes. Automatic extraction creates candidates. Adoption and
  revocation come from exact commands in real controller-supplied user input,
  never from model output, reference text or a tool result.
- Revocation or correction invalidates prior derived working context. Raw
  history remains an audit record; a projection watermark prevents earlier
  assistant/tool paraphrases from reintroducing withdrawn material. The same
  native Session identity continues; no tools are replayed.

The fixed rpi transform hook at `agent_loop.rs:441-567` transforms provider input
without changing native history. Its return type has no error channel, and the
default converter drops Custom messages. Consequently, derived data needs an
explicitly labelled provider-only carrier and a Host fatal/cancellation gate
before the provider HTTP call. Returning an empty context after failure would
not be a valid recovery path.

Summarization uses the same route, grant, call allowance and deadline as the
foreground run, has no tools and performs no silent grant renewal. Each call
must reserve budget before HTTP; malformed output, cancellation, unavailable
budget or write failure retains raw evidence and the previous stored summary.
Source revocation or correction removes the derived summary from the active
sidecar. Raw transcript evidence remains; the first slice does not maintain a
separate archive of every superseded summary. A failed compaction preserves the
previous valid sidecar. References record received input, not proof that an
associated request executed or succeeded.

The summarizer asks the Chat-compatible route for `response_format=json_object`
through a cloned model configuration. Ordinary Agent calls retain their normal
format. JSON mode does not guarantee the required schema: complete JSON parsing,
required-field types and source validation still apply, and unsupported routes
fail explicitly. Unused extra root metadata is discarded; candidate objects
still reject unknown fields. Extra root fields cannot adopt memory or modify
the stored summary schema. A
complete response may contain Thinking metadata even when ThinkingOff was
requested; only Text enters the summary parser. Bounded metadata diagnostics
record content kinds and parse shape, not response or Thinking bodies.

For the specifically verified `deepseek-v4.1-flash` route, the summary clone
also sends `thinking.type=disabled`. The pinned rpi ThinkingOff setting omits
an effort parameter; it does not disable this supplier's default reasoning.
Other model IDs and ordinary Agent calls are not assigned this supplier-specific
extension. A bounded route probe and actual Host trial are separate evidence.

Byte thresholds bound work and storage. They are not a tokenizer or a guarantee
that the request fits the model. Current discovery provides an output token
limit, not a verified context capacity. Missing provider usage remains unknown;
an estimate or default zero is not reported usage.

## Preserved limits and next slice

The current native checkpoint still rejects 256 messages or a file larger than
16 MiB. Each MessageEnd rewrites the complete checkpoint. Projection reduces
model input but does not remove this storage ceiling or rewrite cost. Extending
long sessions requires a separately reviewed, crash-consistent segmented raw
journal and bounded active checkpoint under the same identity. Do not remove
the limits and call the result unlimited memory.

Text is the first slice. Original binary attachments, verified model-family
tokenizers, semantic vector retrieval, background consolidation and stronger
control-side signed memory adoption are subsequent work, not delivered claims.

The user's 2026-10-15 cloud-storage request and selected device-isolation/shared
project/personal direction are covered by the separate
[cloud-memory implementation and design](cloud-memory.md). M1 adds explicit
device/project/Ray-shared cloud records and bounded, current-Run snapshots;
snapshot bodies remain separate from this local sidecar and raw transcript.
Epoch changes, missing snapshots and unavailable snapshots invalidate old
cloud-dependent projections. Exact original audit remains. Queued follow-ups
strip an inherited active snapshot rather than reviving withdrawn cloud content.
Full cloud synchronization, local outbox, automatic consolidation, embedding and
authenticated personal identity remain unimplemented. M1 production/browser
recall and revocation pass on the final release; the extra read-only call in the
revocation trial is recorded separately from the successful answer. See
[final evidence and limits](../evidence/cloud-memory-20261015.md).

## Build evidence trap: normalized source times and reused targets

The 2026-10-15 Linux source-package work exposed a stale build-cache boundary:
source archives intentionally normalize file mtimes to zero for reproducibility.
Extracting changed bytes over a previous tree while reusing its Cargo target
directory can satisfy timestamp-based freshness checks and retain an older
executable. A successful, very short Cargo invocation therefore does not prove
the packaged source was compiled. Use a fresh extracted source directory and a
fresh target directory for source-package acceptance, then inspect the required
new behavior plus artifact SHA/version. Preserve the initial stale-build receipt;
do not label it a successful rebuild or infer a runtime defect from that binary.
Final Linux timings and artifact hashes belong in the release evidence, not in
this durable rule until the fresh-build receipt is checked.

## Method sources and acceptance

Official sources read on 2026-10-14:

- [Anthropic context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents): compact working context, structured notes and just-in-time source retrieval.
- [Letta memory and dreaming](https://docs.letta.com/configuration/memory): separate durable memory with explicit teaching and optional background consolidation. The node borrows the separation, not its framework or automatic adoption policy.
- [LangMem concepts](https://github.com/langchain-ai/langmem/blob/main/docs/docs/concepts/conceptual_guide.md): typed durable memories and retrieval. No LangMem runtime is imported.

Acceptance must inspect actual provider requests and stored artifacts: unchanged
original reference bytes/hash, closed tool boundaries, post-summary recall and
correction, candidate/adopted separation, revocation, scope isolation, restart,
budget/failure/cancellation guards and measured executable size. Controlled
provider tests and real-model trials prove different boundaries. Evidence and
independent review determine acceptance; this design page alone does not.

## Model declarations and text allocation

The Node now reads context_window/max_tokens/input from the current authorized
Hub model directory (including OMP nested metadata). Conflicting positive limits
use the smaller declaration; conflicting modality sets intersect. Unknown
context retains the old192KiB projected-message policy; malformed declarations
fail explicitly. Declared modalities remain distinct from the Host's currently
text-only input transport. A model claiming image support does not add an image
tool or make browser attachments available to this Node.

Known context sizes adjust the text projection/summary trigger. Output and
framing reserves, actual system/schema overhead and a4MiB local resource limit
remain. Both ordinary and summary requests check the fully serialized Context,
including second-stage JSON escaping, before provider HTTP. Byte allocation is
an estimate, not actual tokens or proof of supplier maximum context. A capacity
shrink cannot reset native identity, discard raw history or relax the allowance;
uncompactable content fails with its audit retained. New capability validation
occurs before pending-run persistence so an invalid capacity does not strand the
Session. See[execution evidence](../evidence/model-capabilities-20261015.md).

## Working-context display contract (2026-10-16)

The Host emits `context_status` for the actual serialized working projection
and `model_call.context_observation` for the complete serialized Context. These
reuse TextBudget's JSON allocation checks; they are UTF-8 byte measurements,
not model token usage or a guarantee about provider framing. Declared model
window capacity is shown separately. The near-threshold signal means a future
turn may need maintenance, not that compression is already complete.

`compressing` is emitted only after budget reservation and immediately before
the real summary request. `compact.completed` still follows successful durable
summary persistence. Browser cards must scope live observations to the current
Session/device/Run and reject history playback as current usage. Missing data
is unknown, never a fabricated zero-percent meter. This telemetry does not
change compaction policy, model choice, source retention, or memory adoption.

Cloud appointments can resume an already paused, leased Goal through the
existing command interface. The Node does not gain a separate calendar engine:
its original lease, root budget, Jobs, and review remain authoritative. Hub or
Node reconnection invalidates the old writer connection; schedules cannot
silently repair ownership, extend authority, or replay an unknown command.

## Reviewed tool experience

The 2026-10-15 extension reuses this Store's memory states and source invalidation
for conditional failure/recovery experiences. It resolves actual native tool
pairs in the current Run, retains bounded evidence on a candidate, and recalls
only user-adopted/current experiences on demand. See
[the complete contract](reviewed-experiences.md) and
[versioned execution evidence](../evidence/evolution-20261015.md). This does not
add cross-Session sharing, automatic adoption or a background learner.
