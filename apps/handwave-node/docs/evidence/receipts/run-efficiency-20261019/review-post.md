# Independent Codex POST — bounded HandWave efficiency guidance

Reviewer: `/root/environment_evolution_review`, 2026-10-19.
Procedures: ara-git-review, ara-rust-core-review, point-delivery-audit.

## Current judgment and scope

FINAL: APPROVE / ACCEPT the bounded local Windows candidate point: advisory
current-Run budget data, current-library reuse guidance and final Skill paths.
No reachable production defect found in the reviewed increment. This does not
accept the previous general autonomous two-Session workflow, a measured efficiency
improvement or deployment; those boundaries remain explicitly separate below.

Base is `cad595debd7a3e64a487fea36ea9e04a4a61d277`; target is the current
uncommitted apps/handwave-node increment. Reviewed full hunks and necessary
callers in agent.rs, environment_prompt.md, skill_library.rs/tests and
tests/verify_host.py, test_budget_fixture.py, verify_environment_library.py,
verify_context.py and the direct verify_modes.py fixture correction. Root shared
Core/OMP WIP, UI, cloud, production binaries and
all unrelated files are excluded. Reviewer changed only this audit report;
no Cargo, Python suite, model request, commit, push or deployment was executed.
Scoped git diff --check produced no diagnostics.

## Production semantic review

with_run_budget clones the actual normal-agent Context, appends one bounded User
data carrier at its tail, and checks the complete serialized candidate with the
same TextBudget used by admission. If it does not fit, only the appended message
is removed, restoring the exact original Context. The original full request
check still rejects an originally invalid request; no history is displaced and
no extra compaction is induced by this metadata.

The call observation samples the existing AtomicUsize before reserve_call and
after any transform/summary work. remaining_including_next uses saturating
subtraction: initial n=0, last allowed n=max-1, exhausted n>=max. Summary's existing
reserve_call uses the same counter. The labels are reservations, not successful
HTTP dispatches, task completion or provider/billing usage. Phase thresholds are
explicit: exhausted at zero calls or zero time, last_call at one call, closing
at <=3 calls or <=30 seconds, otherwise working. Monotonic remaining duration
and bounded milliseconds avoid negative/overflow observations.

The existing serialized full-Context gate, cancellation/deadline reserve_call,
root Goal reservation, allowance exhaustion and stop-after-turn behavior remain
unchanged. A root/grant gate may refuse earlier, as the carrier and static prompt
explicitly disclose. Last-call tools can still result in budget exhaustion;
there is no forced success/finalization, permission increase or implicit renewal.

The carrier is appended only to the provider Context owned by normal stream_fn.
Native transcript, Session journal, context-memory Store and summary request
construction never receive this clone. OpenAI-compatible messages retain complete
assistant-call/tool-result pairs before the final User data observation; it is
not a forged ToolResult or elevated System instruction. Summary uses its separate
provider path and no budget carrier. This narrow Host still does not claim other
provider protocol support.

The preceding messages/system/schemas are cloned intact and precede the new
volatile metadata. This avoids this particular counter changing the existing
prefix; existing Goal/environment directory carriers can still change other
prefix bytes. No cache hit, speedup or token/cost saving is inferred. Working
projection telemetry remains distinct from the full model-call observation,
whose bytes are now computed from the actual selected Context including the
optional carrier.

Plan's tools and initialization permissions are untouched. The budget is numeric
current-Run data with no new identity, path or credential payload. The static
environment guidance uses current returned library paths and successful probes,
discourages unnecessary home/other-assistant exploration, and retains necessary
verification plus honest unfinished-work reporting. It is guidance, not an OS
sandbox or an instruction to skip evidence to satisfy a budget.

Skill create/install now return final destination bundle_dir only after a
successful staging rename. The receipt follows existing recursive redaction;
the source/staging directory is not returned as the installed bundle. Hash,
metadata ownership, status, licenses, resources, no-overwrite and no-implicit-code
execution behavior are unchanged. Existing skill_read returns the same path.

## Fixture review

Fixture.handler retains the original received wire body in requests before
passing a legacy scenario view to reply. Only a recognized final User budget
carrier is sliced from a shallow-copy messages list; inner native messages and
real task/steering/tool pairs are untouched. Derived and monkeypatched responders
still use the common handler; this avoids unrelated edits to their scenario logic.

The recognizer checks the exact prefix, complete field set, scope, nonnegative
integer types (rejecting bool) and known phase. test_budget_fixture covers tail
only, preservation of raw input/steering/tool pairs, malformed JSON, wrong role,
scope, types and extra fields. The initial nonblocking adapter-strengthening
suggestion led to final remaining-call arithmetic validation and a contradictory
count negative case. The actual Rust carrier calculation already enforces the
correct arithmetic and phase selection.

New actual-wire assertions correctly inspect Fixture.requests, not the trimmed
legacy agent_requests view. Environment Host checks fresh per-call counts/time,
exactly one carrier, no accumulation and absence from durable JSON/JSONL. It
checks create/install paths against actual files and preserved resource bytes.
Context Host correlates raw wire requests with actual agent/summary model_call
events and shared summary counts, and checks absence from raw history, sidecar
and summary input. These assertions prove the selected observed scenarios when
executed, not universal carrier presence: metadata remains legally optional when
the complete request lacks headroom.

## Initial source-review test plan

Rust unit cases cover initial/closing/last/max/usize overflow/max=1, time threshold
and expiry, shared reservation count, original-message immutability and the exact
full-Context boundary with escaped system overhead. An already invalid baseline
remains invalid after omission. Skill tests check both project create and global
install final bundle paths without replacing hash/resource assertions.

At initial source review, final pins, executed tests/lint, the actual candidate
image and Host/context-summary/failure/Plan/Goal receipts were pending. The final
audit below supplies that evidence. The previous autonomous two-Session failures
and staged production release remain unchanged. This review does not authorize
automatic deployment or accept a general autonomous-evolution/efficiency outcome.

## Final pinned execution audit

The reviewer recomputed all 42 implementation-source pins and all 37 fixture
pins from source-pins.json / fixture-pins.json: zero mismatches. The actual final
release EXE hash was independently computed as
`92470f7ab8cce9400e75ede12414466557e3e77f07bee448a32993d91101dabc`,
5,355,520 bytes. artifact.json identifies base cad595de... and explicitly records
production_deployed=false. This is a local candidate using the existing version,
not the previously deployed dba28da4... image.

Directly inspected retained execution receipts:

* cargo-test.log: 203 passed, zero failed, one explicitly ignored public-network
  test; 7.08 seconds, including both new Rust budget tests.
* fixture-unit.log: two helper positive/negative/nonmutation tests PASS. The
  helper now also rejects contradictory remaining-call arithmetic.
* cargo-clippy.log: warnings-as-errors check PASS; cargo-deny.log: advisories,
  bans, licenses and sources OK, dependency warnings retained.
* cargo-release.log: optimized build completed in 1m27s.
* Actual candidate Host groups: environment5, context15, regression27, web5,
  modes5: 57 successful groups. Environment/context/regression/Web receipts name
  the actual 92470f7a... image. They are controlled-provider/real process and
  filesystem effects, not real-model reasoning or a production task.
* Formatting PASS was reported by the coordinator; the reviewer directly ran
  only scoped git diff --check, not Cargo/rustfmt or test-suite reruns.

The reviewer independently parsed the retained
host-context/summary/model-requests.json and corresponding controller events:
14 wire requests correlate to 14 model_call events, comprising 12 agent and two
summary calls. Every agent observation has reserved_calls=event.call-1 and exact
remaining arithmetic, including agent call3/reserved2 after summary reservation.
Each agent request contains exactly one final carrier; no summary request
contains it. Thirteen retained state JSON/JSONL files contain zero carrier
prefixes. This directly verifies actual provider-only data, shared summary
counting and absence from durable history beyond merely reading fixture PASS.

## Modes fixture failures and narrow correction

host-modes and host-modes-v2 remain failed receipts. The first expected an older
read-only schema list, omitting the seven existing Web/environment read tools.
The second expected an incompatible busy follow to be silently inherited and
complete; base transport.prepare_follow already rejects explicit mismatched
execution modes / Goal identity. The budget increment did not change transport.

The reviewer inspected the final direct fixture correction: it expects the
incompatible follow's nonzero terminal with the exact mode error, asserts no
additional provider dispatch, releases the held original Run, requires that
original Run to finish normally, and preserves the original Goal identity,
criteria and awaiting_review state. It does not weaken the product guard or
replace rejection with success. Existing positive Goal artifact/restart/
pause/resume/explicit accept/cancel cases then complete in v3. The previously
failed expectation is not described as a new product permission defect.

## Bounded actual-model observation, not whole-trial acceptance

One bounded actual-model trial was made by the coordinator, not retried. The
original live-project-two-sessions result remains FAIL and reuse remains NOT_RUN.
However, direct inspection of create-result.json confirms the actual create Run
settled successfully with terminal code0 and a final answer, after 12 forwarded
DeepSeek v4.1-flash/openai-completions calls and 73.096 seconds. Total harness
elapsed time is 78.048 seconds and conservative reservations are 697095; these
reservations are not provider token usage or billing.

The fixture failed later because its bounded invocation recognizer handles direct
Python shell invocations, not the successful Python subprocess orchestration
actually used. The reviewer independently inspected the actual native call
`call_00_nXrRdP9h1hhGD0LwB6mU9044`: Python constructs the installed Skill/script
path and invokes subprocess.run([sys.executable, script, *args]). Its exact
completed tool_end output SHA256
`7a603b7742987259f16e9011d73bf99f5057034750be55fa531f358007e38c5f`
matches the authentic evidence attached to Skill metadata.

Independent artifact checks:

* Current bundle SHA256 recomputes to
  `9571548d1a70a09a0b3463873ea89130184b2d5e64bf23f9e9497a0b387fb7c8`,
  matching installed_hash and verified_hash.
* Actual first-summary.json has valid6 / invalid2 / INFO3 / WARN1 / ERROR2.
  Both original source-log hashes match their captured originals.
* The native subprocess output records same-Run alternate-input results
  valid7 / invalid3 / INFO2 / WARN2 / ERROR3, the bundled fixture's independent
  expected-result comparison and same-path rejection exit2. Temporary secondary
  outputs were explicitly removed. This is not a second-Session reuse result.
* Current observed project note filename/body SHA256 and managed INDEX agree on
  `badccf40affd015955a739318e7a6b4f90697ced48153950ce40b4f0ed06278a`.
  Original human index/source-rule content remains present.

Thus a narrower first-Session task completion and actual script/project-knowledge
effects are supportable independently of the recognizer. Do not rewrite the
original overall FAIL as PASS or infer cross-Session model recall. The raw
requests independently show reserved0..11, limit20, remaining20..9 and decreasing
time; every actual-model phase is working. This trial therefore does not prove
closing/last-call guidance influenced behavior or caused a speed/token saving.
Two PowerShell script failures, an earlier failed probe, failed schema/evidence
arguments and extra directory/tool exploration remain in the transcript. They
are not hidden by the successful final artifact.

## Final boundary

Accept this small local Windows observation/receipt point on the pinned candidate
and controlled-Host evidence, plus the specifically verified first-Session model
observation above. General autonomous two-Session reuse remains OPEN, as do
Linux/macOS candidate execution, automatic efficiency improvement and production
deployment. No new device grant, system dependency installation, cloud/Core/UI
change or permission modification was made by this audit. The reviewer performed
only source/receipt/artifact inspection and external report writing, with no
additional suite execution or model requests.
