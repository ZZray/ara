# ARA engineering evidence cases

Project cases follow the user's October 13, 2026 authorization and the local
AI engineering evidence ledger. Runtime status belongs in linked evidence;
these notes retain reusable investigation constraints. No case accepts an OMP
surface or authorizes a release.

## CASE-20261023-ARA-text-continuity-admission

- Scope: the dirty `56d55d2` worktree's new continuity Host/example consumer,
  2026-10-23. It reuses public Core, Session and Workflow contracts; it does
  not implement original-Agent native resume or change fixed OMP acceptance.
- Ordinary JSON messages reopen as `NativeMessageSource::Persisted`, so the
  presence of `native_message` does not imply a live Graph owner. An initial
  blanket rejection caused three of thirteen tests to fail. The repaired
  bridge uses the existing strict `checked_legacy` observation for persisted
  sources, compares body/source against the current Journal, and keeps live
  Graph and archive owners unsupported. A successful lossy DTO conversion
  would not establish the same source guarantee.
- Successful read-path receipts alone did not prove the required knowledge
  fact was returned. Final review now checks actual returned text and its
  order before the final write. The first negative fixture also had an invalid
  assumption: the real ReadTool adds three trailing context lines to a finite
  ordinary-text range. A `:1-1` read of the short note therefore contained its
  token. Keep the actual returned content as evidence; the corrected fixture
  puts the fact outside that returned heading window and retains the original
  rejection assertion. Heading-only plus oracle-correct output then receives
  NeedsChanges; real body retrieval and another write receives Verified.
- A root-scoped replay marker needs matching canonical Session ownership.
  Prepared absolute Journal/source paths cannot be reused from a copied root.
  Independent source review also found that `starts_with(root)` permitted an
  ancestor-root/junction alias. Require the exact canonical `root/session`
  directory and its Journal/source parents before acquiring the marker. The
  ordinary copied-root case is executed; the junction variant is source-
  reviewed, not a separately executed native junction experiment.
- Final provenance checks must include original source sidecars, not just new
  append receipts. A source-read claim also needs the actual receipt key
  `source_entry_id` and a successful lookup. The live task used that lookup
  to retrieve the original correction, so successful continuation cannot be
  attributed solely to summary fidelity. Summaries and originals retain
  separate owners and source identities.
- Final evidence: all52 current package tests pass, including13 continuity
  functions; controlled and live trials use distinct original prepare/resume
  processes with cumulative7 and8 logical calls. The live processes total
  18.564 seconds; no comparable baseline exists. Independent Codex POST
  `9072b7e8cae0168478aa78bcff1592ac64f995f7dc3513bcef869754d9be9d70`
  accepts the checked text/two-file consumer scope after inspecting original
  journals, actual tool results, artifact bytes, source pins and original
  budget. Early compile, bridge, fixture and Clippy failures remain recorded.
  General retrieval, long histories, generic public restoration, Full/Lite,
  coding gains and real-model Length recovery remain open. See
  [continuity evidence](../evidence/core-workflow-20261023.html#continuity).

## CASE-20261023-ARA-project-read-contract

- A Host wrapper can remove an existing tool capability through its advertised
  schema. The first Full S3 consumer enumerated only bare paths for ReadTool,
  while the real tool omitted the middle of a large public file and suggested
  an inline line range. Core schema validation rejected the one requested
  `tests/cases.json:1-160` before the wrapper executed. This was a new Host
  integration defect; the underlying ReadTool already supported that selector.
- Validate the parsed base against the original file grant while preserving
  the existing selector parser. The wrapper first parses without filesystem
  access, checks the granted regular file, then verifies the real tool's
  literal-path priority. A literal colon name or Windows alternate stream
  must not change the authorized target. Only NotFound, NotADirectory and
  InvalidFilename count as absence; an uncertain literal probe fails closed.
  Writes retain exact bare paths. Native regression reads an omitted marker
  through finite windows and checks unauthorized paths and write selectors.
  A portable ADS test has an explicit filesystem-availability branch; its
  overall PASS alone does not establish collision coverage on every platform.
- The same first trial stops after seven logical requests with a thinking-only
  `Length` response: all 16384 reported output tokens are reasoning, without a
  final submission. The other call and time allowances remain. The two saved
  source changes separately pass the original public10/private48 after a new
  compile. That diagnostic does not create an original Host review, release
  the old fence or accept the failed Run. The selector defect is confirmed;
  its causal contribution to Length remains unproven.
- Explicit reasoning controls must be checked at the actual request boundary.
  In the existing Chat adapter, `force_reasoning_off` is not the consumed
  switch. The opt-in Host contract uses `disable_reasoning` and the existing
  `zai-thinking-disabled` policy encoding. A model catalog's reasoning flag
  and a Host declaration that a wire parameter is supported are separate facts.
  Current HTTP tests preserve the default body and show that the explicit
  mode adds only `thinking: {"type":"disabled"}`. A bounded second live trial
  submitted a public10/private37 candidate, received sourced NeedsChanges and
  continued in the same Session, then exhausted its 32-request allowance.
  No provider capability is inferred from a model name or missing usage fields.
- A third trial restored ProviderDefault and explicitly submitted after public
  checks passed, retaining the finite read windows. It reached normal Stop and
  VerifiedArtifact in 18 requests and 141.208 seconds. Host public10/private48
  passed; a separate fresh compile of the sealed 18 files passed the same58.
  Independent POST SHA `5c5e704b72fbda071f2b6b8d17055bcdcd837d0a07c7803092f7bd9f6e4c6531`
  accepts this artifact and executed Host path. Core terminal admission did not
  change. The two earlier failed Runs remain failed; reused cases, adaptive
  trials and changed instructions do not establish a new holdout, a single
  causal explanation for Length, or general coding stability.
- [Versioned execution record](../evidence/core-workflow-20261023.html#original-s3)
  retains the failed Run, independent artifact diagnostic, restored read
  windows and explicit generation-policy checks. General coding, reverse
  analysis and release acceptance remain separate gates.

## CASE-20261023-ARA-workflow-terminal-alias

- `RunReport::clone` retains the same mutable `AgentMessage` records. An
  execution driver can await its result and replace a tool-free assistant's
  `Length` body with `Stop` through the public record API. A separate cloned
  report is therefore insufficient evidence for terminal admission.
- The original `ara-workflow` fault test reproduces an incorrect
  `VerifiedArtifact`. The repair seals a private terminal-admission value in
  the canonical owner before publishing any result alias to the driver. The
  original records and provenance remain available; continuation uses the
  sealed fact. The same attack then stops at `NonStopAssistant` without review.
- Cancellation of a waiting future must also preserve access to reconciliation.
  A consumed, unique join handle can lose its eventual closed-gate fence.
  Cloneable control handles retain a shared completion result; the Host must
  keep one control owner until settlement or reconciliation. Dropping every
  handle does not settle effects or reopen admission.
- Reopening a `SessionJournal` validates neither all expected records nor their
  provenance: the reader can skip malformed entries. A verifying Host compares
  actual append IDs and bodies, preserves source receipts, and commits its
  artifact checks. An identical Session header alone is insufficient.
- Length recovery differs across consumers. The Core already handles some
  truncated tool-call turns, and the CLI has its own maintenance recovery.
  The public typed workflow can separately request Host settlement of a
  completed tool-free Length. That receipt must not have an artifact-success
  verdict. A new input remains in the original Session and under the original
  request, attempt and absolute-time allowances; normal artifact review still
  needs a later Stop. A continuation opportunity is not a recovered task.
- New journals intentionally stay in memory until their first Assistant.
  Synchronizing the journal pathname immediately after an initial User append
  caused the new consumer's first native test to fail before settlement.
  Respect `is_on_disk()` while capturing append receipts, then verify every
  original body and provenance after materialization. Do not create a separate
  journal or relax settlement to hide that failure.
- [Module evidence](../evidence/core-workflow-20261023.html) separates the
  executed red/green fault, controlled feedback repair, small real-model C++
  task and open dependency audits. These results do not establish complete
  coding, reverse analysis, Native Graph integration or product acceptance.

## CASE-20261021-ARA-fixed-source-stable-rust

- V4 follow-up on2026-10-29: after explicit user authorization, the bounded
  async correction and reviewed repairs execute all nine original groups:
  89pass/0fail/0ignored,490.566s build and207.108s execution. Independent EXEC
  fda054bc and point audit9fc19281 accept the executed recovery scope while
  retaining M5/M8/O1/O8 as partial. Final WIP preservation does not establish
  byte equality during an ordinary child's active interval; primitive fault
  fixtures do not establish the actual Task owner startup/kill/publication
  outcomes. Preserve those proof boundaries when reusing green test results.
  See the current [Task recovery handoff](../handoffs/2026-10-14-omp-upgrade.md).
- V3 follow-up: all selected images compile in1311.006s; nine groups execute
  in248.639s with84pass/5fail/0ignored. A retained release can result from a
  final-capture fetch URL parser failure before backend stop; inspect the exact
  release/effect receipts before calling it a cleanup failure. Dirty/current
  synthetic trees need the fixed source's destination ODB, otherwise branch
  three-way apply can miss its preimage and fall back to a WIP-bearing tree.
- The actual Host shutdown event can remain unsettled while a library-only
  lifecycle snapshot reports an empty settled set after unregister. Fixtures
  must observe the Host receipt that includes historical close errors and
  retained resources; retain both views and fail when the actual event is
  missing. In this run actual Root close remains false with exit1; no production
  false-success claim is supported by the failed observation assertion.
- Synthetic device-only WRITE is advertised coordination transport, not a
  complete filesystem WRITE delegation. Task preflight must preserve the
  existing ToolContext distinction while retaining exact grants and all-batch
  validation before creation. The observed failure creates extra child
  journals; it does not establish an unauthorized filesystem write.
- A three-response fake script can fail a correct async Main continuation:
  Wait consumes one result, a later original result triggers a fourth Main
  request, and an exhausted script returns500. Trace original job/result and
  request timestamps before increasing a timeout. The proposed bounded3or4
  expectation keeps child2each and checks any fourth request's original-child
  notice. The user later explicitly authorized this expectation change;
  V4 execution and the limited acceptance above supersede the pending state.
  Independent source review af69de48 alone does not accept runtime behavior.

- Follow-up on2026-10-29: after cache cleanup, the original three selected
  preparation commands reach a missing `ara_agent::AgentTool` import at the
  two MCP `definition()` calls in binary Task preflight. The single-line repair
  changes neither grants nor assertions. The failed cold build is retained:
  1346.878s compiler time, zero executed tests. Independent source review
  06b0e81b approves the exact import before adoption.
- The same module cache then prepares all images in623.860s and executes all
  nine original groups in115.346s:81tests pass,8fail,0ignored. A successful
  compiler is separate from the failed behavior result. The strict Graph
  fixture uses one required string, so the existing single-string normalizer
  can legally map `context` into `task`; the assertion did not print its actual
  return. The source-backed fixture repair requires both real flat-Task fields,
  `task` and `solutionSpace`, preserving the input and all four assertions.
- Windows canonical paths can carry a verbatim prefix that Git rejects as a
  worktree destination. A short private-Git comparison succeeds with an ordinary
  path and fails with the equivalent verbatim path; the defect is independent
  of path length. The scoped rcopy repair converts supported drive/UNC prefixes
  only in Git argv, retaining canonical filesystem paths and ownership checks.
  UNC/non-UTF8/Unix behavior remains source-reviewed, not runtime-verified.
- The same run separately hits a273-character shallow-clone keep path. Use a
  short private runtime/home for the repeated original scenarios; changing
  that prerequisite does not prove the production argv repair. Host missing
  child aliases and print os123 also remain observed failures pending rerun.
  Independent review c2feda11 approves the combined three-file delta and the
  unchanged three-command/nine-group recovery. The next run is launched;
  no M1-M9 acceptance or overall counter increase is claimed here.

- The Task M1-M9 module at fixed OMP b91db66e fails its first native-library
  build with E0658 at two `Option::map_or_default` calls in the copied
  `ara-task-vcs/src/git/patch.rs`. Reading the fixed source and independent
  source review did not establish compatibility with the installed stable
  toolchain. Preserve the complete backend; equivalent stable APIs can retain
  the original empty-string and empty-map behavior without reducing scope.
- The failed build takes162.502s, with153fresh/reused and125compiled Cargo
  artifact messages. No Rust tests execute and the subsequent CLI build does
  not start. These counts describe build messages, not distinct packages or
  accepted behavior. The original RESULT, stdout and stderr remain in
  `peer-owned-resource-runtime-windows-v1` under the existing D evidence root.
  The narrow two-call repair and vendor note await successor execution; the
  failure does not justify a toolchain upgrade or a new full-workspace build.
- Before adoption,35files from the independent `ara-agent-lite-core` extraction
  appear outside the original1365source inputs; no original input changes or
  disappears. The current21manifests and four candidate manifests contain no
  membership/dependency on that crate. Independent reviewer scope approval
  cf77af2e permits excluding that unrelated subtree from this module's input
  freeze. The manifests remain pinned, so a new dependency edge still invalidates
  the batch. Preserve those WIP files and avoid treating their independent
  changes as a Task test failure. This exclusion accepts no behavior in that crate.

## CASE-20261021-ARA-retained-lock-and-audit-boundaries

- Scope: fixed OMP b91db66e, baseline HEAD935e5493 and its dirty compiler inputs;
  the Task resource-owner candidate is not compiled or accepted by this case.
  One Cargo metadata resolution retains all510baseline package identities and
  adds81packages. Normalized existing dependency changes are only the two Task
  libraries under ara-cli and allocator-api2/equivalent under hashbrown0.17.1;
  other textual dependency changes disambiguate newly duplicated names.
- Current full cargo-deny returns FAIL:31advisory/maintenance and2license
  diagnostics reference unchanged baseline package versions. New packages have
  zero reported errors. Presence in the old lock is not proof of exploitability
  or an accepted waiver; do not label the full audit PASS, silently upgrade
  unrelated dependencies, or infer a ready product/release from this delta.
- Fixed gix-pack0.72.0 consumes uluru3.1.0 under MPL-2.0. Its archive checksum
  7c8a2469 matches the fixed OMP lock. Retain the full license and source access
  notice and review an exact package/version exception, rather than a general
  MPL allowance or a reduced Git backend. This candidate policy still awaits
  independent review and does not authorize distribution.
- Resolution takes24.313s and the actual audit169.730s; these are dependency
  preparation costs, not compilation or test execution. A separate0.564s CLI
  argument-order failure is retained. New-only debug0 overrides are authored
  for75package names absent from the baseline; existing dependency profiles
  remain unchanged. Compile savings and actual cache reuse remain unmeasured.
- Evidence: ROOT-DEPENDENCY-GATEb01d76d4, resolved lockdc028eb5, raw audit log
  7d015cc3 and diagnostic classification99abb716. Full source/plan review,
  delivered-code execution, platform/model/product acceptance and the failed
  global dependency gate remain separate obligations.

## CASE-20261018-ARA-child-source-contract-boundaries

- Rustfmt parsing does not expand a `json!` body. The same Task module's second
  build passes native preparation in43.977s, then fails CLI preparation in
  481.304s because `json!` sees an array literal followed by an index at
  task_host_cases.rs162. Grouping that Rust expression preserves the emitted
  route label; it changes no assertion or model input contract. Retain the
  original macro diagnostic and check related authored fixtures together before
  the next cached build. No tests execute in that failed525.281s batch; a syntax
  parse receipt cannot establish type checking or macro expansion.
- Task candidate d1637641 has a source-confirmed preflight mismatch: its
  task_host::prepare treats both an agent definition's declared tools and the
  caller's explicit tool request as mandatory parent exposure. Bundled scout
  declares find; reviewer/security-reviewer declare find/lsp, while the reference
  Host's twelve BuiltinKind/TOOL_NAMES contain no find. These actual agent names
  therefore fail before ChildHost creation. Existing child preparation already
  intersects declarations with actual parent capabilities. Preserve strict
  checks for explicit caller requests and actual grants; definition metadata
  must not manufacture a capability. Root verifies the relevant declaration,
  preflight and consumer sources after the independent reviewer finding. The
  same concentrated G3 needs real bundled-specialist calls; a fixture-only agent
  cannot prove this path. V2 source repair is independently approved by8f16d5ab
  and adopted byca092ac8; all three grant kinds require `Some(true)`, including
  an explicit rejection of `Some(false)`. Actual execution remains pending.
- A pure-Jujutsu fixture needs .jj/repo, not only .jj. In the same candidate,
  G1 creates an empty pure/.jj inside an outer Git checkout and then expects
  rejection. The fixed filesystem discovery correctly ignores that incomplete
  workspace, so the outer Git root is returned and unwrap_err would panic when
  reached. Correct both pure and colocated fixture shapes while keeping the
  strict discovery code and all original assertions. This is a source-derived
  prediction, not a recorded test execution or permission to weaken discovery.
- A tool-end observation is not a Run-end gate. V3's real soft Wait returns
  successful/interrupted, but its feature fixture releases the original
  grandchild model response immediately. That completion can become an aside
  while the child Run remains active, legitimately causing request4; the
  fixed3-response script then returns500 and retries until settlement times out.
  Keep the raw failed capture. The narrow candidate waits for the real child
  agent_end before releasing those same bytes, retaining original3soft/2hard
  responses, raw Wait/journal controls and timeout constants. This specifies
  the intended interleaving rather than adding replies or suppressing production
  parent-result delivery. V4 executes the complete P4 successfully in15.397s;
  soft/hard preserve3/2requests and both observe childEndObserved=true. Final
  independent point acceptance remains separate in linked evidence.
- A raw peer body and its prompt envelope are separate contracts. Hub v2 P2
  fails a rendered trailing-space assertion although the actual journal's
  details.message preserves both trailing spaces exactly. Fixed b91 prompt.ts
  render544-547 invokes post-render format;210-214 trims each rendered line.
  The recovery checks exact raw details/from/to and the separately normalized
  envelope, preserving the raw requirement rather than changing production
  formatting to satisfy a fixture. V2's original failure remains recorded.
- Pending IRC before a tool call does not imply Native Wait executed. V2 P4
  has a real delivery ACK, then Immediate BatchInterrupt emits
  interrupt_skipped/executed=false before the kernel. Existing real Wait mode
  bypasses that generic skip and lets the kernel consume the queue first;
  preserve both modes as distinct controls. The original early-aside path
  separately exposes a current-Host domain defect after tool refresh: its
  temporary projector lacks a journal encoder and produces Legacy input for
  an actual Graph request. The recovery creates new IRC and parent-result
  inputs using the current PreparedNativeSession values at the producer,
  following existing edit/service seams. Do not import Legacy at request time,
  change global SessionFactory reuse, or add a fixture frontier when the
  existing real mode supplies the correct prerequisite. Independent diagnosis
  c84565f2 and source-backed mode checks support the candidate; actual v3
  execution remains pending in the linked evidence.
- A source-reviewed caller can still use a private cross-crate API. Hub's first
  module batch records E0624 for SessionJournal::path_to,456.930s compilation
  and0tests; Root and independent source review missed the visibility boundary.
  Public branch() is not equivalent because it chooses the current leaf, while
  this Host projects the explicitly selected retained fork. The narrow candidate
  traverses public entries(), preserving last-ID-wins, missing/cycle termination,
  nearest session_init (invalid depth gives0) and None->0 semantics. Keep the
  original real fork comparison and frozen failed receipt. Candidate equivalence
  is source support; executable recovery remains pending in the linked evidence.
- Copying a batch plan must rebind both Child and Peer artifact-directory envs,
  not just runtime_output/ARA_HOME. V11's source and effects were isolated per
  fresh UUID, but fourteen new Cold roots landed under the retained v10 parent
  and were absent from v11's default artifact manifest. Root and plan review
  missed this. Preserve the frozen plan/result; verify new UUIDs/processes,
  double-hash the actual roots and check every old retained file unchanged.
  V11 supplements278new files while preserving947old ones without replay or
  moving receipts. Hub adoption now checks both artifact bindings against its
  own new runtime before source adoption; source-root isolation and evidence
  archive ownership are separate checks, and neither is an OS sandbox.
- V10 crosses the original Cold748 alias failure but its catalog supplement
  stops before any model request: requestModelId was put into authored models.yml.
  Fixed ModelDefinition/custom finalization and the daily authored-field allowlist
  do not admit that internal field. Existing materialized HostModel donors do
  preserve it. Keep the production allowlist and original wire/catalog/journal
  assertions; supply a feature-gated private donor through ordinary Registry
  composition. Source-reviewed v11 does this without creating a production
  configuration option. Source approval is not execution of the restored case.
- A quota-owned earliest sibling wait can be shorter than an unchanged raw
  provider hint. V10 records auto_retry_start delay3136ms followed by CURRENT
  provider wait increased although the raw hint is still2hours; only the first
  model POST occurs and no consume is attempted. rpc_host captured its effective
  delay as NativeRetryWaitGuard's provider ceiling, so the later raw-hint recheck
  falsely reports growth. Freeze max(original raw hint,ordinary backoff) before
  quota/switch overrides and retain the original recheck. The fixed upstream
  turn-recovery earliest-sibling path then re-ranks on the next getApiKey.
  Independent v10 EXEC04d71737/source POST9455b682 and Root direct source/wire
  checks support this diagnosis; v11 executes the original actual recovery and
  same-Graph2h->3h refusal->2h restoration successfully. This is not a timing-tolerance
  adjustment or permission to skip CURRENT ownership/budget/cancel checks.
  [Actual failures and delivered-source recovery](../evidence/omp-upgrade-child-native-contract-20261018.md).

- V5 execution distinguishes a catalog selector from an opaque request model ID.
  Assigning provider/id directly to a raw route duplicates the provider when
  persisting the child model; reopening then clears the endpoint as if switching
  models. Catalog metadata can also identify a different requestModelId, so a
  repair must preserve the catalog reference in the saved selector while sending
  the original wire ID. A confirmed catalog selection must bypass raw-route
  heuristics, including early returns; do not change the global raw-ID parser.
  The grouped source-v7 candidate and original Cold748 control are adopted
  after independent review; runtime execution remains pending.
- Fixed auth/select.ts distinguishes skipping whole-pool ranking from checking
  selected-row usage safety. A warm explicit pin still receives the latter.
  V5's zero-usage assertion is contradicted by that source; preserve production
  safety and observe the actual selected account/provider/endpoint plus zero
  ranking calls. This is a fixture correction, not permission to suppress usage.
- The v5 Windows stack overflow is observed at first harness-tag Regex compile.
  An original1MB/copy16MB same-scenario comparison completes only on the copy,
  with0model requests. Actual Windows debug events and matching PDB show large
  startup/fixture poll frames beneath the compiler. V7's startup-only boxing
  reduces the observed caller span by about7KB but still overflows on the normal
  image; it is not a sufficient fix. The matching v7 debug image supports a
  separate boxed constructor for the feature-gated fixture future, requiring
  original1MB execution. V8's boxed fixture makes all5actual Peer banks pass,
  including P2, but8post-readiness RPC reset paths still overflow. A successful
  fixture entry is not proof that the main RPC actor/turn frames fit1MB.
  Do not change tag grammar or accept a patched diagnostic image.
  Retaining the original short private roots subsequently proves8reset
  RPC/Gate failures also have stack-overflow stderr. The ninth CLI failure has
  another cause. EOF and gate timeouts alone did not establish these distinctions.
- A new empty-home Chat RPC process also reaches readiness/accepts one prompt
  and overflows before model work, with no reset operation or tools. This
  separates the shared command future's stack pressure from reset/account
  behavior. V4's matching ordinary image captures entry into start_admitted
  beneath the serial serve/run caller chain. Large-offset nearest PDB names
  must not be treated as proof that an auto-compaction method actually ran.
  The reviewed narrow candidate boxes the handle constructor and moves its
  unchanged execute/error-response/reconcile body into handle_owned; it adds
  no task or scheduling point. Stack sufficiency still needs ordinary runtime
  evidence. V9 saves48864observed caller bytes but still overflows farther down
  the same command path, so this individual boundary is not the whole fix.
  Reassess the full forwarding/constructor chain before another build.
  The independent diagnostic review was interrupted by the platform,
  and the subsequent source review uses a separately identified Codex reviewer.
- A completed saved-reset operation does not prove that the later assistant
  stream produced visible text. V7's CLI has one confirmed consume/two model
  POSTs/the same Session, but assistant.content is empty while providerPayload
  retains the expected text. Fixed b91 Codex output_item.done finalizes message
  and reasoning only on a previously opened block; function_call done can emit
  an end event without inserting an absent block into final content. The Rust
  actual Responses router follows that contract. Existing reset fixtures need
  same-id/index output_item.added predecessors for text/reasoning/tools; preserve
  their original done/error/completed events and effect/Unknown assertions.
  V8 executes the original CLI reset/final-text case successfully;8RPC cases
  remain failed before they can exercise the same repaired text stream.
- Custom-model schema acceptance is not daily-route authentication acceptance:
  generic auth:oauth passes the file schema but daily configuration rejects it.
  Standard apiKey makes a valid synthetic catalog while disabling HostDefault
  on a newly built unscoped route. A control requiring an original root pin B
  must actually create HostDefault before adding the child catalog through the
  existing same-backend offline refresh; forging a selection flag or replacing
  root auth changes the tested behavior. Captured child scope must still precede
  configured/installed/environment/keyless escapes. These source-backed fixture
  prerequisites do not authorize generic OAuth protocol expansion.

- A production struct-field addition must include old cfg(test) constructors,
  even when integration targets already compile. Cold9's binary-unit target
  exposes six ActiveRun and one Session omissions; the grouped constructor
  audit confirms the complete seven-line None repair. Preserve the failed
  compiler receipt and execute the original owner tests in the module batch.
  A controlled model-call budget also does not prevent configuration reads:
  the existing binary-unit fixture loads LoopGuardSettings via ara_home.
  The reviewed outer launcher binds an initially empty process-local ARA_HOME
  before the frozen runner; integration env_clear keeps its own private roots.
  V5 is launched, not yet runtime-accepted; exact hashes and receipts are in
  the existing upgrade handoff. No shared runner or product configuration changes.

- Cold9's source parser accepts a json! body before macro expansion. A leading
  JSON array followed by .into_iter() needs the entire Rust expression grouped;
  syntax-only success did not establish that macro contract. The following
  type frontier finds ara_ai::now_ms returns i64, distinct from floating usage
  clocks; use integer millisecond addition for the synthetic JSON expiry.
  Preserve both failed batches (350.303s then21.981s, zero tests), regroup the
  new helper API checks, and modify only the affected test file to retain
  other source mtimes. Source review and a parser remain separate from Cargo.
- Attribute compiler artifacts by target.kind and profile.test, not their
  count: Cold9 v2's two new artifacts are a normal library and Child test
  image, not a successful ordinary binary or CLI unit image. Installed Cargo
  documentation also distinguishes build --tests --keep-going (all test
  targets) from test --no-run (the selected module); test has no keep-going
  flag here. Preserve the four selected images instead of widening tests to
  obtain diagnostics. Current recovery is pending execution; see the exact
  source pins, failure receipts and limits in the existing upgrade handoff.


- Final independent v9 EXECca7113e0 checks the physical source/runtime/images
  and original frames/journals with0findings. Root accepts only the observed
  cancel-close race repair and the actual fixture corrections; all12selected
  functions pass. Original Unknown failures remain Unknown, and no cancel-ACK
  injected-failure matrix or full Child/Peer acceptance is inferred. The exact
  original contracts and remaining source identities remain in linked evidence.


- V9's delivered source now has actual controlled Windows Host evidence:
  both original root-close observations settle both peers with no unresolved
  jobs/turns, and entered soft/hard Wait results remain distinct. All12selected
  original functions pass after237.755s incremental build and30.592s execution;
  full source/image bindings are checked. This closes the observed self-race
  in that scenario, not the complete cancellation fault matrix. Cancel-error
  preservation remains source evidence; no new canceled-ACK injection was run.
  Complete Child/Peer and durable source-case promotion still await their own
  original coverage gates. See the linked current evidence for final EXEC.


- V8 causal receipts disprove an unresolved-job explanation for the P1 root
  timeout: both exact native close flights are Ok and active/unresolved lists
  are empty; a single lifecycle row retains a cancellation actor-response loss.
  Concurrent same-child cancel/close can destroy its own ACK. The v9 repair
  joins cancel first but retains failure and continues cleanup; siblings still
  run concurrently. Production acceptance awaits actual successor execution.
- A bus-delivered message and ParentSteer are distinct valid Wait frontiers.
  Fixed bus consumes an existing waiter first; a successful message result
  without interrupted:true is correct there. To test entered soft interruption,
  trigger the existing real ParentSteer actor path, retaining the original
  normal result/durable/uniqueness assertions. Do not change production Wait
  to discard a correctly consumed message just to satisfy a fixture.


- V7 distinguishes a queued result from a real interrupt: the actual tool event
  is synthetic interrupt_skipped/executed:false before Native Wait enters.
  Automatic parent completion already has a separate job notification and
  nonconsuming Wait notice. The v8 source repair removes only its IRC-peek
  classification; runtime evidence is still required. ParentSteer/test107 is
  a separate original streaming case, not proved by abort_and_prompt alone.
- Ordinary tool refresh and ACP plan refresh have different prerequisites.
  V7 registered/removed URI refusal phases pass, but ordinary Peer Sessions
  have no ACP plan owner. The v8 fixture reuses production set_host_tools;
  adding a fake ACP owner or weakening the production guard would change the
  tested contract. Fixed b91 RPC set_host_tools/refreshRpcHostTools is the source.
- Child shutdown events do not prove root settlement. Preserve the first
  close Result and final unresolved jobs/active/retained close diagnostics.
  A sealed+Ready dispatch warning alone neither creates Unknown nor explains
  root_close_snapshot's filter; do not turn it into a speculative fix.


- Actual v6 integrates successfully but returns8pass/8fail. P1/P2/P5 root,
  descendants and effects are already settled before their90s process waits:
  a retained URI event sender prevents output-task channel EOF after the real
  session_shutdown. Terminal-frame ownership and ordinary channel lifetime
  need separate completion checks; raising the timeout cannot fix this cause.
- Small-window summary scripts must follow the actual raw-entry cut and window
  planner. Preserved16000context/keep16 produces4summary windows, not the
  fixture's1response; supplying a final tool-call response to the second summary
  caused IncompleteResponse and eventual cancellation. An initial5window
  hypothesis was disproved before adoption by token/cut/source-ID evidence.
  Keep exact bounded responses and observable model/tool/journal assertions.

- Compiler artifact kinds have distinct boundaries: the combined v5 packet
  finishes both integration images but fails the ordinary binary. Do not infer
  binary success from library/test artifacts or from a previous diagnostic
  disappearing. Its60diagnostics arise from five missing wiring/import/local
  error-conversion categories; repair them as one source delta and preserve
  the failed receipt. The subsequent v6 combination uses4fresh syntax parses,
  35exact bridges and the same16groups, retaining both the newer fixture suffix
  and the separate WriteTool conversion. Runtime is pending; see the linked
  [Child evidence](../evidence/omp-upgrade-child-native-contract-20261018.md).

- Scope/status: CHILD-NATIVE-CONTRACT, fixed OMP b91db66e, reference Host.
  Source defects were independently reviewed and checked against the pinned
  source. Repairs remain awaiting compilation/runtime evidence; this case
  accepts no child/Peer/Task/OMP surface. Current exact candidate and review
  identities belong in the [upgrade handoff](../handoffs/2026-10-14-omp-upgrade.md).
- A missing new report and invalidation of an old report are different events.
  Fixed executor.ts2035-2057 and task/executor-subagent-reminders.test.ts736-788
  invalidate the earlier prose when a later assistant turn starts non-yield
  work. Reverse-scanning past that work and preserving last_report on None
  can return narration written before the actual work. The local repair uses
  NoUpdate/Report/Invalidate and joins nonempty text blocks with newlines.
  A controlled RunReport state test proves a different boundary from an actual
  Native yield callback: the latter may reject empty final output before the
  result assembler can observe it. Keep both observations distinct.
- Candidate lookup success does not prove model authorization. Fixed
  persisted-revive.ts180-183/212 and sdk.ts3073-3088 retain the saved model as
  an authorization fallback after the requested role. Current registry lookup
  can return a configured model whose daily Fixed lease has no key. Child
  selection must consult the existing current registry/auth policy, retain
  explicitly keyless endpoints, and try the saved candidate when the role is
  unavailable. A missing-model fixture alone cannot cover missing credentials;
  a generic ambient test key can also hide that failure branch.
- Iterative preprocessing does not make recursive serde_json Value destruction
  iterative. Named strict-schema temporaries, replacement assignments, rejected
  conversion trees and imported dereference temporaries need explicit local
  disposal on success/error/early return. The scoped repair reuses its worklist
  disposal; it does not prove every public config/prompt/Core/provider consumer
  is safe at arbitrary depth. The original 20,000-layer fallback and constrained
  512-layer counterpart still require execution. No crash-depth measurement is
  inferred from source inspection.
- Fixed executor.ts2459-2473/3650-3672 distinguishes a known busy rejection
  before dispatch from an unknown/lost reply. Empty-item followup can lose the
  interval between State ACK and actual actor admission to an ordinary turn;
  sequential fixture operations cannot prove this interleaving. The scoped
  repair stamps an internal typed NotDispatched receipt in that actor guard,
  permits at most three total initial attempts, waits for the competing owner
  before contract mutation, and leaves admitted/unknown effects unreplayed.
  Independent source POST543f56a4 supports the repair; the gated real-actor,
  attempt-limit and lost-ACK/one-write assertions still require execution.
- The first executable Child bank reveals a projection boundary distinct from
  callback correctness. Fixed yield.ts659-667 deliberately includes own
  undefined-valued data/type fields. ValueHeap.export_serde is a strict tree
  bridge and rejects them, so the test's details-envelope projection fails
  before its JSON assertions. The isolated helper candidate uses the existing
  native JSON stringify; original Graph identity/UTF16 assertions stay outside
  that projection. It has not passed successor execution yet.
- An actor-local AgentConfig override is not necessarily the effective model
  request. Child's forced final yield sets config.tool_choice, but RpcHooks
  returns a shared ExecutionSnapshot and Core config_from_snapshot replaces
  that field. The actual C3 request bank reaches its forced-choice assertion
  with the expected path absent. Recovery must carry the scoped override
  through the existing snapshot owner; changing the expected request would
  hide the production wiring gap. Source repair and execution remain pending.
- Native shutdown must not depend on every externally retained catalog dropping
  its emitter. The C5 journal/operation receipts show the denied write, accepted
  yield and inspect completed before close; session_shutdown is emitted, while
  write_native_output still waits for channel closure. A retained child catalog
  can keep that channel alive. The proposed recovery uses the actual shutdown
  frame as the Native writer's terminal boundary, preserving pending-reply
  rejection. Actual ordinary-process close and affected Native regressions are
  still required; increasing the timeout does not address that ownership cycle.
- Fixed executor.ts2646-2700 classifies a terminal model error as a failed
  outcome. ARA's actor retains an unsuccessful completion, while WaitIdle also
  returns the provider error; followup currently replaces this with a generic
  error path. Recovery must distinguish a persisted actual RunEnd::Error from
  unknown dispatch, cancellation and persistence failure. The latter must not
  be inferred to be a known model result merely because clean=false. The
  isolated typed-receipt candidate remains awaiting review and execution.
- Efficiency/limits: source checks used non-mutating syntax parsing, with no
  Cargo during these source repairs. The first concentrated build later failed
  E0509 after448.870s with490reused/6compiled and0tests: adding Drop to
  OutputSchema also makes moving parameters out illegal. The two-line repair
  uses mem::take rather than a recursive clone. Syntax parsing had not proved
  Rust ownership/type correctness. Recovery writes only changed bytes, keeping
  unchanged file mtimes; no final compile-time saving is claimed yet. Final
  affected-package compilation, real process/failure banks and independent
  execution review remain required before accepting the module. The v5 selected
  integration build passes in608.062s with499reused/5compiled; tests97.708s
  produce2PASS/5FAIL. This is an incremental build, with no measured full-build
  comparison. Retain successful exact-source banks and consolidate the five
  failed/affected banks after source review; no module acceptance is inferred.
- The later v4 batch separates those boundaries: pool/native JSON details now
  passes, actual C1 known-model failure and main C3 forced-choice assertions
  execute, and C5 passes actual denied/granted write and close frontiers. Four
  other failures remain; partial progress is not a completed Child contract.
  The one zero-Cargo debugger capture identifies validate_object ->
  validate_leaf_keywords -> validate_node recursion at the original512-layer
  Graph input. The earlier20000-layer loose fallback completed first; repairing
  only owner drop or raising a stack limit would target the wrong failure.
- Preserving borrowed Graph semantics during that repair requires suspended
  schema-child continuations, not a serde snapshot or eagerly collected
  iterator. A next() error does not close its own cursor; an error in an entered
  body does. Ref leases and outer iterator closes must unwind in their actual
  nested order, including original-versus-close exception priority. Independent
  source review covers all15 internal child sites and a combined actual-runtime
  trace regression. The first candidate omitted an unchanged regex helper;
  independent review catches that unresolved name before Cargo, and a one-block
  successor restores it. Syntax parsing alone had not proved name resolution.
  The adopted successor's schema-lib preparation passes179.505s; execution and
  complete-case acceptance are still separate gates.
- Original-case mapping must retain its actual observation owner. Four fixed
  provider strict/schema cases were mapped to Host coverage, but every current
  process init omitted outputSchema. Green core schema tests cannot establish
  those wire requests. A historical steering test also injects after the tool,
  whereas the fixed yield case injects after streamed call discovery and before
  execution. Preserve that timing gap instead of bridging by a similar name.
  The [versioned Child record](../evidence/omp-upgrade-child-native-contract-20261018.md)
  keeps all five observations OPEN while existing ordinary-bin/fixture reuse is
  investigated. No passing narrow batch advances the full surface counter.
- Later v7 execution passes the four original schema-validator families and
  crosses the original512-layer Child frontier. The Child schema bank then
  exposes a separate fixture-state error: an incremental section and a
  no-sections terminal sample reused the same owner. Fixed yield.ts533-580
  treats those states differently. Isolate the samples rather than weakening
  the terminal assertion or changing production yield behavior.
- A durable Number can retain its exact bits while changing JSON spelling:
  declared compaction f64 fields72.0/0.0 reopen as exact integers72/0 through
  native checked serde. Complete serde Value equality distinguishes these
  representations. Normalize only those two declared numeric fields with
  the existing checked native bridge; keep IDs/unknown metadata strict and
  reject9007199254740993 instead of rounding an entire metadata tree.
- A smaller retained-token budget does not always permit a larger compaction.
  The actual C5 fixture passes CLI keep1; the reverse scan stops at the final
  ToolResult, which is not a cut point, and finds no retained suffix boundary.
  The preserved journal has7+5 approximate raw tokens per complete tool cycle;
  candidate keep16 crosses one full cycle and selects a legal custom-message
  boundary. Source/receipt derivation supports this fixture repair; repaired
  runtime execution remains pending. compaction.keepRecentTokens occurs only
  in an unknown-setting preservation test and is not this production reader.
- The ordinary-bin provider supplement distinguishes successful execution
  from a verifier mistake. All16operations and6controlled Responses requests
  complete in8.560s, but the verifier expects bare call_id while the actual
  router preserves the fixed call_id|item_id composite. Preserve the failed
  run and raw journals/events; an exact-ID successor may recheck immutable
  artifacts offline instead of replaying the process. Such reconciliation is
  reused actual evidence, not fresh execution or full Codex OAuth acceptance.

## CASE-20261019-ARA-peer-scan-attachment-race

- Scope/status: Peer candidate library v2 at fixed OMP b91db66e. Root and
  independent Codex source review confirm a reachable deletion race; runtime
  acceptance remains pending. Full review6e3bf317 covers15paths and retains
  original233+59 identities/five banks. Exact candidate/fix/review pins are in
  the [upgrade handoff](../handoffs/2026-10-14-omp-upgrade.md).
- A scanner precheck sees an exact parked/detached generation, releases the
  registry mutex, then invokes generic unregister(key). Another worker may
  publish a live revival of the same generation between those calls. Generic
  deletion checks only the key and can remove the now-live actor's registry
  ownership. No await in the scanner hunk does not make its two mutex-protected
  calls atomic across runtime workers. Attachment publication CAS alone cannot
  protect against a later deletion that ignores attachment state.
- The one-hunk repair reuses existing unregister_dead: exact key, Parked and
  runtime=None are checked under the same registry lock as publication. If
  revival wins, deletion rejects; if deletion wins, the stale parked expectation
  rejects. Absent-only insertion still guards the replacement. Independent
  delta POST6635e6d2 approves the source correction; original broad banks and
  actual Native integration remain unrun. No new Core API or test target is
  justified by this repair.

## CASE-20261019-ARA-peer-root-activation-and-hook-wrapper

- Scope/status: fixed b91db66e Peer Host production v2,17paths. Independent
  Codex POST9ed02960 and Root source checks establish four candidate defects;
  no runtime acceptance is inferred. Source-plan PRE28cde597/Root85d879ed
  authorize the grouped successor. Exact pending state belongs in the
  [upgrade handoff](../handoffs/2026-10-14-omp-upgrade.md).
- Adding a producer to CliHooks does not prove RPC can observe it. The actual
  wrapper stack is CliHooks -> RpcHooks -> ToolLoopHooks. RpcHooks returns only
  edit asides and screens out the new Peer producer. A genuine queued notice
  can therefore interrupt a Native tool without ever delivering its triggering
  raw message into the next model context. Trace every forwarding layer and
  the durable callback; an interrupt event is not message-consumption evidence.
- Candidate adopt assigns the new Session/Agent/config before fallible old
  Peer root shutdown, then refresh takes the old runtime before awaiting its
  receipt. Unknown shutdown consequently leaves a failed command with a new
  logical Session and no new binding. Added ownership work must obey the
  existing preparation-before-publication boundary. Uncertain external effects
  cannot be rolled back into a pretend-live previous owner.
- A root seal protects one activation, not permanent rejection of the original
  conversation. new B then switch A must retain A's persisted identity/file,
  while stale A callbacks remain attached to the old permanently sealed owner.
  The approved repair stages a successor Host with retained old receipt and
  shared runtime allocation. It also preserves the existing detached Bash
  producers and their original journal routing; broadly closing them would
  silently change a separate stable contract. Same-Session Wait rebind requires
  authoritative settled proof and no pending/Unknown claims.
- Immediate and Wait interrupt settings have distinct legal scheduling. The
  Immediate notice/soft signal path exists; an already-completed AutoInFlight
  parent result can still depend on the next aside boundary under Wait mode.
  Calling it running until the30min cap creates mutual waiting. The scoped
  repair recognizes that exact source-owned completion frontier, returning
  normally without duplicating the completion, shortening the cap or changing
  Core. The repaired original banks must exercise both settings.

## CASE-20261018-ARA-service-fixture-boundaries

- Scope: HOST-SERVICE-CONTROL dependency at fixed OMP b91db66e, Windows
  and Linux reference Hosts. This is an investigated case, not complete service/WRITE/OMP
  acceptance. Exact snapshots, receipts, retained failures and current pending
  work are in the [upgrade handoff](../handoffs/2026-10-14-omp-upgrade.md).
- A successful custom append can leave a new SessionJournal lazy and absent
  from disk. Source `persist` returns before the first assistant materializes
  it. A persistent service ACK therefore uses the existing materialize barrier
  only for that lease's exact claimed weak message; customType/body equality is
  not authority. Ordinary messages and no-session retain their existing lazy
  and memory-only behavior. Delivered v2's concentrated unit bank executes
  actual lazy/materialize/file-error and weak-lookalike cases successfully;
  full process-bank acceptance remains pending.
- Tokio1.53.1 Windows ChildStdin.shutdown does not close the owned write handle:
  poll_shutdown ultimately returns Ready without dropping the pipe. The EOF
  fixture waited for CLI exit while retaining this handle. Drop the actual
  ChildStdin, keeping the original deadline/journal/pending assertions. In v2
  the journal already had one eof-race completion and broker pending was empty;
  v3 passes that EOF frontier and reaches the later provider-error case. This
  evidence does not justify changing the production REPL loop.
- The busy fixture executes Bash. Its Windows `ping ... > NUL` created a
  literal reserved-name project file, and strict artifact metadata collection
  failed with os_error1. Use the actual shell's `/dev/null`; do not suppress
  arbitrary metadata errors or broaden the collector exclusion list. V3 passes
  this frontier with original retention checks intact.
- Windows Console mode0 is not equivalent to the Unix cfmakeraw fixture.
  V2's actual PTY child lost two LF bytes, while the pipe case passed. Source
  broker/process preserves PTY data through write_all/flush before ACK, matching
  fixed OMP. Enabling VT input in the test child makes the original exact19byte
  assertion pass in v3; no production newline rule or expected bytes changed.
  The subsequent query-response case still fails and requires separate evidence;
  do not generalize this byte case to complete terminal-query acceptance.
- Test driver reuse must bind the actual ordinary binary and runtime fixtures.
  V2's selected build legitimately relinked an unchanged-source LSP fixture,
  invalidating its pre-build image hash. Source/log/artifact identity plus the
  new image hash permits explicit binding and fresh execution, without another
  Cargo call. Preserve the failed gate and distinguish an old driver from new
  execution. A same-build normal fixture should use this provenance boundary
  rather than forcing the previous executable hash after relinking.
- Measured compilation: first service batch909.367s; repaired v2 selected
  build460.606s (490reused/16compiled); subsequent fixture-only v3 build59.541s
  (505reused/1compiled). V2 zero-Cargo execution125.344s yields5/7groups and
  9/11functions PASS; v3 focused execution74.843s still fails the later query
  and provider-error observations. Tiny harness recovery also incurred avoidable
  preparation/review time; keep its delta small and reuse existing validators.
  These are observed costs and limits, not a predicted full-product ETA.
- A long test-output prefix also lengthens tempfile homes and the derived Unix
  broker socket. The original Linux build passed311.374s, but three process
  banks failed before dispatch with SUN_LEN errors and a fourth could not see
  broker metadata. Keep the exact production/socket contract and choose a short
  test prefix. Same-image recovery passes all four process banks44.837s with
  0Cargo; retain the already-passed unit instead of rebuilding or repeating it.
- Filesystem type and evidence lifetime are different constraints. A short
  ext4 `/tmp` recovery emitted four PASS results, but its raw artifacts were
  absent after WSL restarted. This is lost evidence, not formal acceptance.
  Use a persistent short output and keep an outer process alive until terminal
  artifacts are copied and individually hashed. The corrected recovery exports
  361verified files; earlier tool output/failure records remain historical.
- Text-mode rewriting can duplicate CR in an existing CRLF source. The query
  stage candidate's308CRCRLF endings produced a49.659s failed build before any
  test. Root restores only duplicate CR bytes, compares every original line
  (only the intended query line differs) and performs non-mutating syntax
  parsing before the concentrated build. This avoids changing production or
  declaring a whitespace-corrupted candidate equivalent merely from its text diff.
- The single diagnostic Windows query run records actual accepted CSI5/CSI6
  replies, then an OSC11 send without a reply. Linux independently records all
  three exact replies. This locates the blocked probe but does not prove which
  OS component consumed it. Keep the platform matrix and Windows OSC11 gap;
  do not infer native support from parser unit tests or another platform.
  [Raw evidence, costs and remaining acceptance](../evidence/omp-upgrade-service-control-20261018.md).

## CASE-20261018-ARA-upgrade-fixture-and-cache-boundaries

- Fixed scope: EditTool upgrade from596f2da to b91db66e. Independent source
  triage4e1a0e00 identifies a real schema-consumer defect: two new taxonomy
  overrides contain glob rather than model. The old exact-only scanner panics
  on that missing string, including during ordinary CLI startup. Retain the
  new rules and implement fixed-source exact/pattern precedence; deleting the
  rules would hide the defect while losing required identities. Source
  POST5c37b407 approves the repair candidate; runtime acceptance is pending.
- A grammar directory change does not unload an already-loaded language:
  WasmGrammar.loaded is a process-lifetime OnceLock. A missing-INI test that
  follows the real INI installer in the same process observes that language.
  The unchanged executable/source passes the same missing-language bank in a
  fresh process,0Cargo/0.053s; independent POSTecdab2bc confirms that boundary.
  Isolate the conflicting banks rather than changing valid runtime caching.
- A linked-parser fixture cannot silently stand for the target's downloaded
  TLA+ grammar. The existing AST group now has an explicit real-release
  module prerequisite, catalog size/SHA verification and unchanged edit/apply
  assertions. Acquisition is not proof that parsing or the tool loop passed.
- Review caught an incorrect fixture migration: a legacy LSP test Host's
  default rich result has final_content=None, so WRITE retains the requested
  text contract there. A real rich supplier returns settled formatter bytes.
  Preserve this distinction when updating snapshot-tag expectations; the
  rejected host-contract candidate and corrected source POST853e6fee remain.
- Versioned fixture supersession must retain observable effects. New seen-line
  guards assert refusal plus unchanged bytes and revealed content before the
  same-payload retry succeeds. New final-argument matching tests pass actual
  effective arguments; they must not restore the old empty-argument bypass.
  [Current source packets and execution boundaries](../handoffs/2026-10-14-omp-upgrade.md).
- The next batch distinguishes another real renderer defect from fixture
  errors: the second selected input Get belongs inside the fixed source's
  try/catch, while the initial typeof Get stays outside. Retained original
  throw-identity/partial/final observations expose the misplaced boundary.
  A mutated toJSON on an already-cached input is not a fresh throw observation;
  use fresh identities and an effect count to prove the intended branch ran.
- A borrowed EditCall handle is not an entered effect. The same-ID finally
  observation now waits for the real temporary-file write, holds its writer
  acknowledgement, replaces the preview and then releases the old effect.
  This tests ownership at an actual lifecycle frontier without timing sleeps.
  Source POST6e462d14 approves that fixture; the final complete-module audit
  3e621529 accepts its actual execution on the final delivered source.
- Cancellation need not mean no file exists: the fixed LSP supplier commits a
  newly created file before formatting. Verify its exact admitted bytes and
  this request's format/cancel/didClose correlation, rather than attributing
  another request's cancellation or expecting a rollback absent from source.
- The normal Windows binary's initial native-protocol probe times out in the
  tested ConPTY collector. A separate real-child transport probe preserves
  OSC877 but drops outbound APC (raw SHA14062f69); fixed OMP emits APC and its
  OSC877 adaptation is for input. This establishes a collector/platform
  boundary, not a passing native protocol or permission to change the wire.
  Retain the failure and exercise the same full protocol with a real Linux
  PTY; neither result establishes physical Tern visual acceptance.
- A Run's native surface close is not the next prompt's input-readiness receipt.
  The real PTY first run passed editing/undo/submission/journal then sent EOF
  before the next raw prompt owner existed. The same unchanged ordinary binary
  passes both full protocol cases after an explicit reopened empty-editor
  prerequisite; all12original waits/budgets/EOF assertions remain,0Cargo.
  Actual frame5/6/7 shows draft→edited→undo, independently of a broad historical
  frame predicate. Final native EXEC18f20b4e and point3e621529 verify the raw
  frames, current source/runtime/binary, journal and clean process exit.
- A UTF16 character-count receipt from an earlier WRITE contract is not the
  upgraded ordinary-file UTF8 receipt. The actual `hé😀` write still preserves
  exact bytes and all nine callback/failure modes; migrate the final count4→7
  from fixed write.ts, without changing the separately deferred progress
  message or production behavior to satisfy a stale assertion. A single
  cfg(test) migration recompiles only the bin-test unit (518reused/1compiled).

## CASE-20261018-ARA-grammar-upgrade-source-closure

- Scope/status: complete EditTool's direct grammar/terminal prerequisites at
  fixedb91db66e; source repairs independently reviewed and frozen100-path
  composition adopted; subsequent reviewed repairs complete the104-function
  Windows union and actual Linux native two-case gate. **Complete EditTool and
  declared direct prerequisites are accepted** by point3e621529; full grammar/
  AST/READ/WRITE modules and release gates remain separate and open.
  Original D candidates/rejected reviews are preserved;
  [current composition and exact receipt identities](../handoffs/2026-10-14-omp-upgrade.md).
- Evidence: independent G09 SOURCE POST0c5bbc22 checks42 frozen paths against
  exact upstream and finds a real missing withDownload producer/consumer chain,
  blank-lang filtering, language-error search counts, target-wise missing-set
  ordering and a scheduling-sensitive peer-lock fixture. A declared intentional
  UI difference did not establish full source parity.
- Correction: keep the process registry and actual TerminalDriver subscriber
  within the existing output owner; track only real transfer/verify/atomic
  install, preserve headless behavior and keep subscriber lifetime independent
  from shared installation. Independent activity POST4c7e1c6b additionally finds
  that one drain timestamp can turn a long queued download into a hidden fast
  success. V2 preserves producer monotonic time across the queue and isolates
  tests from unrelated process-global activity. Independent follow-up3851af55
  approves these source repairs and existing HTTP fault assertions; runtime
  verification remains pending.
- Verification trap: a350ms child lock lifetime plus a parent200ms elapsed
  assertion can fail a correct lock or miss contention after scheduler delay.
  Use bounded parent-release handshake, actual held-lease probe, no-transfer
  observation, release and successful settlement. G09-FIXES-SOURCE-POST0eafc897
  approves four exact source fixes; it is not a runtime receipt.
- Efficiency boundary: no Cargo/model ran in this source-preparation turn.
  Source review and complete-module bank preparation run concurrently. No
  comparative speedup, compile-time reduction or product readiness is claimed.
- A second independent runtime-source review catches a production/fixture pair
  that agrees with itself but contradicts fixed registry enum parsing: outer
  whitespace is trimmed and JSON strings decoded before enum validation. The
  test's ` hashline ` invalid-fallback expectation cannot justify keeping the
  same defect in production. Preserve the original fixture/source snapshots,
  derive the repair from the fixed registry and recheck the concentrated bank;
  production/test repair is included in the frozen98-source composition and
  covered by unified SOURCE POST190c2299; execution remains pending.
- Final shared-source review catches Windows modifier loss, split Alt/UTF8
  consumption, empty-string preview truthiness, eager original Graph getters
  and stale ANSI rows during a late native handshake. Fixes stay in existing
  concentrated banks; parsing or self-consistent typed fixtures cannot prove
  original Graph access order or a real terminal transition.
- Dependency preparation is separate from compilation. The static lock fails
  the first locked fetch in13.203s. One workspace-only offline resolution
  removes44 unused identities without introducing a registry version/checksum
  change; independent lock POST816eaf27 confirms501 final packages and retained
  wasm/native-test/Go dependencies. Resolved locked fetch then takes13.773s.
  These are prerequisite times, with0 compiler/test executions.
- The actual offline dependency audit passes source/bans checks but rejects
  two unchanged existing license grants. Cached advisory checking also reports
  fixed Wasmtime29.0.1 vulnerabilities. Preserve the failures and exact lock;
  controlled local verification does not establish dependency or release
  acceptance. Do not silently upgrade the pinned dependency graph to erase
  this result. Exact receipts live under the current handoff's complete-root-v1
  dependency-preparation-v1 packet.
- The first whole-module no-run build exposes an unresolved private helper;
  the next layer exposes duplicate module declarations, a private type import
  and an unconstrained closure error type. Fix the complete emitted diagnostic
  set per layer, preserve source snapshots and continue the identical build
  configuration. The first two failed builds consume232.248s/228.591s and
  reuse243/460 artifacts; they are not discarded whole-workspace rebuilds.
  Independent narrow source reviews ee365e7b/fe381f97 approve the mechanical
  fixes while compilation proceeds. No runtime success is inferred from them.

## CASE-20261015-ARA-upgrade-composite-fixture-migration

- Scope: fixed596f2da to b91db66e EDIT engine and direct Host consumers;
  [source, failed batches and current verification boundary](../evidence/omp-upgrade-edit-engine-20261015.md).
- Observation: the migrated Host target first reports14PASS/4FAIL. Four
  source-reviewed fixture corrections yield3PASS/1FAIL because the old READ
  display assertion at the end of one composite function was previously
  unreachable after its earlier failed assertion. The initial review missed
  that second contract change; successful compilation did not rule it out.
- Distinguishing evidence: fixed READ source changes default byte budgeting,
  raw CR/BOM/terminal-LF rendering and decoded-snapshot tags. Production
  behavior matches those source paths. An old assertion alone does not justify
  modifying READ normalization or discarding the original invalid-byte input.
- Intervention: inspect every remaining assertion in the failed function
  against fixed source before another build. Update only the source-superseded
  expected contract, preserving the original inputs, exact continuation text
  and actual file effects. Independent Codex SOURCE2e041aa6 approves the last
  block; UnixHost-v4 actually executes it successfully in0.115s after36.497s
  incremental compilation. Other17 successful bodies/helpers remain unchanged.
- Efficiency: native closure so far uses275.605s compilation/7.308s execution,
  including failures. Reuse415 successful library/AST tests and17 Host groups
  with an explicit source/dependency bridge. These are measured process times,
  not a wall-time baseline or claimed speedup. Windows and complete-point audit
  remain pending at this entry; no full READ/EditTool acceptance is inferred.

### Recovery — 2026-10-18

- Actual Windows batch1454.048s build/23.173s tests found two image failures
  from omitted existing Bun bindings, one stale raw-LF assertion and two
  incorrect filter namespaces. The driver rejected missing filters instead of
  letting successful sibling filters hide unexecuted obligations. Reusing the
  same images with the bindings/names fixed passes5 functions in11.159s,
  without a new build; all2555 Host source pins remain unchanged.
- The initial recovery description misread the panic line by one and proposed
  changing both plain and raw expectations. Independent source/runtime review
  rejects that candidate: plain at221 already passes; raw at222 is the failure.
  The rejected v6 candidate was never adopted; only the v7 raw expectation is
  changed. No production READ behavior is modified.
  This demonstrates why an actual numbered source/receipt check must precede
  treating a test mismatch as an implementation defect or source supersession.
- Final failed function executes successfully in0.079s; targeted build231.995s
  reuses446/compiles6 artifacts, not an assumed single artifact. Root verifies
  current1206sources/77deliverypaths/14Windowsimages and165 distinct successful
  Windows functions with explicit prior-source bridges. Independent Codex
  FINAL-EXEC36f8e673 APPROVE and POINT-AUDITa367d2f9 ACCEPT_COMPLETE_POINT now
  close CRATE-PI-EDIT; Root rechecks immutable receipts and current pins before
  writing ROOT-ACCEPTANCE in the linked recovery packet. Full READ/EditTool/
  product gates remain open. Total8Cargo compile1961.648s/tests41.719s include
  failures across dates; no wall-time or review/wait duration is inferred.
- The supposedly smaller Cargo repair command changes the selected package/dev
  feature union: original logs include Tokio test-util and Hyper http2/server,
  while the focused command does not. It emits a different snapshot executable
  fingerprint and recompiles six artifacts despite unchanged production source.
  Build selection and execution selection must be separate: preserve the reviewed
  batch's package/profile/feature/target configuration when repairing its source,
  and narrow only the executable filters. This does not request workspace builds;
  unchanged module artifacts should be reused. The next batch must measure the
  resulting cache reuse; no comparative speedup has been established yet.

## CASE-20261015-ARA-native-fixture-descriptor-prerequisite

- Scope: complete Retry upgrade at fixed b91db66e, delivered dirty Windows
  reference Host; [versioned execution and repair receipts](../evidence/omp-upgrade-retry-20261014.md).
- Observation: the single CLI target compiles successfully in983.746s with
  496 reused/9 compiled artifacts. The first aggregate then fails in1.651s
  with `Value is not callable` after41 original behavior markers. Other two
  exact functions pass on that image in0.266s, with no Cargo invocation.
- Source-supported defect: the added Date descriptor save/restore fixture
  calls Object reflection methods absent from this reference realm. Existing
  ValueHeap already exposes own_descriptor/define_own; this is a fixture
  prerequisite, not evidence that production needs a new reflection framework.
  Root and independent Codex account_identity_review corroborate the mismatch;
  the initial source review missed the fixture's actual import requirements.
- Narrow intervention: preserve the complete original PropertyDescriptor,
  including Function identity and flags, through those existing heap methods.
  All original inputs/assertions and production files stay byte-identical.
  Five source-bank diagnostics distinguish where subsequent execution stops.
- Status: repair source independently approved3e9c5cf1; v2 runtime confirms
  all five owner_a banks including the complete Native hint bank PASS. Its
 138.738s build reuses504/compiles1 artifact; aggregate2.033s then fails farther
  downstream with `Retry callback owner expired`. The descriptor defect was
  therefore not the only fixture prerequisite failure. Both failed aggregates
  remain; source-bank success does not accept the complete module.
- Reuse boundary: the two successful exact function bodies are unchanged by
  this fixture repair and have independent EXEC9d47b300. Reuse those receipts
  explicitly; neither replaying them nor expanding production Object APIs is
  necessary to validate this intervention. No comparative speedup is claimed.

### Same closure: callback identity does not retain a Rust closure owner

- The next original-driver call is the new reentrant-clock fixture. Root and
  Core author independently trace its inline Arc to NativeSignalDomain's
  intentionally Weak callback registry. The local temporary is dropped when
  registration returns; the later original Date.now callback cannot upgrade
  it. Two additional terminal-drain fixture callbacks have the same defect.
- Existing successful fixtures retain a named original Arc and pass a clone.
  Keep that prerequisite ownership in each new fixture through its actual
  work/drain, preserving the production Weak registry, callback identity,
  original Run and all assertions. A graph callable handle is not proof that
  its separately owned Rust callback is still alive.
- At the v2 checkpoint, pending: systematized prerequisite review of the remaining new owner_b banks,
  independently reviewed fixture delta and complete failed-aggregate replay.
  At that checkpoint, no repaired callback execution or complete-point acceptance was claimed.

- V3 intervention update: source audit34ab5ff9 and actual aggregate5.344s
  confirm reentrant/finally/NativeRun-exit callbacks now execute. The aggregate
  fails later at lazy cap preparation, so this confirms the closure lifetime
  diagnosis without accepting the module. Build132.952s,504 reused/1 compiled;
  the original failed packets and review misses remain retained.

### Direct Runtime dependencies cannot be supplied accidentally by Logger

- The later synchronous lazy cap Call fails because its new Math.floor
  dependency is not installed by the independently usable RetryBindings.
  Number.isFinite and Math.max are present. Logger preparation can install
  floor, but NativeRealm and logger-free complete Host preparation do not.
  Root/Core author/reviewer read those exact constructors and b91stream.ts194.
- This is a production integration omission, unlike the preceding two fixture
  defects. A fake test floor or unconditional Logger preparation would hide the
  missing direct dependency. The narrow repair adds FLOOR to existing Retry
  imports using the same ToNumber/runtime pattern as CEIL/TRUNC; supplied own
  descriptors and the original Math object are preserved.
- At the source-review checkpoint, audit54920937 approves the one-file repair. Its actual lazy fault and
  cross-process execution is still pending; no success is inferred from the
  source review. Keep deterministic cap execution separate from real model,
  Linux Host and full product acceptance.

- Final update: v5 actually passes all five lazy fault rows and four genuine
  two-process HTTP modes after the floor import and subsequent EOF observer
  fix. The complete aggregate passes8.833s; independent EXEC5416d9cf and
  pointdae94226 accept the declared Windows/numeric provider scope. This
  supersedes the earlier pending execution state, not the Linux/model limits.

## CASE-20261015-ARA-lazy-EOF-result-receiver-close

- Scope: b91db66e Retry cap upgrade, dirty Windows reference Host;
  [exact packets, reviews and limits](../evidence/omp-upgrade-retry-20261014.md).
- Observation: v4 build383.916s passes,502 reused/3 compiled. The aggregate
  runs7.791s; queued-close and queued-abort pass. Active-close reaches its final
 3s forward timeout after300 actual events, cancellation notification and
  held-lease/unfinished-resource assertions, then genuine resource release.
  This exact error can only come from the row's final timeout wrapper; its
  start/buffer/abort waits have no such wrapper. No larger timeout was tried.
- Source cause: EOF forward waits only for Run-close or original result.
  Consumer-close instead aborts request-local source and closes delivery.
  Producer Drop seals the authentic resource receipt but does not settle the
  original result Promise. The still-pending EOF observer can block the join.
  Root/Core author/reviewer independently confirm this reachable wait cycle;
  its specific EOF-first runtime schedule remains source inference.
- Intervention: add the existing delivery-close guard to the EOF observer,
  retain original Run-close priority and `!sink.is_done()` so producer terminal
  self-closure cannot suppress original-result observation. Keep outer genuine
  receipt join and releaseOnce, without settling a fake result or detaching work.
- At the source-review checkpoint, independent reviewbf6fe580 approves one production hunk;
  actual replay is pending. Preserve300events, original3s and all assertions.
  A green compile or isolated source review cannot accept the full Retry point.

- Final intervention: v5 build374.393s/502 reused3 compiled; same aggregate
  passes8.833s without changing300events,3s or any fixture assertion. All five
  fault rows and four physical two-process modes pass, including active close
  after genuine resource release. Root/reviewer verify1203sources/27pins/index
  and4currentimages; EXEC5416d9cf/pointdae94226 independently accept this scope.
  Only the EOF observer branch changed from failed v4, supporting that causal
  diagnosis in the exercised path. It does not establish an arbitrary scheduler
  trace, real model, Linux Host or whole-product result. All earlier FAILs stay.

## CASE-20261014-ARA-live-Symbol-compaction-bridge

- Scope: fixed b91db66e account upgrade, dirty delivered Rust source and preserved
  index; [source/runtime identity and failed receipts](../evidence/omp-upgrade-openai-account-20261014.md).
- Observation: ordinary CLI stores a correct actual WRITE/tool result, then its
  next request fails at Session native_compaction_entry's checked legacy bridge.
  A bounded exact failed-case repeat saved3 synthetic journals (6.230s,0Cargo).
  Persisted assistant JSON remains valid; usage{} correctly means unknown.
- Source-localized cause: Responses Graph router uses private Symbol properties
  on the original block and clears a partial marker by assigning Undefined,
  retaining the own Symbol as required by upstream. Persistence omits Symbol
  keys, whereas strict export_serde rejects them. The projection maps its lower
  error to UndecodableMessage. Valid saved JSON cannot disprove live-domain error.
- Counterexample/rejected repair: filling fake usage, removing canonical Symbols,
  or changing the strict exporter would change unrelated contracts. A no-summary
  fastpath alone would leave manual compaction broken.
- Initial candidate decision: reuse the existing narrow assistant/budget observation
  at a separately named compaction DTO boundary. Keep canonical roots/blocks,
  arguments/payload identity, strict checked_raw/export and non-assistant roles.
- Initial status: source diagnosis independently corroborated; repair/runtime validation
  pending. Direct Symbol/strict-boundary assertions plus the original actual
  account tool/compaction/restart workflow are required before closure.

### Same case: assistant repair progresses to tool-result observation

- V2 actual build passes in818.369s (501 reused/9 compiled,0 compiler errors).
  The existing direct native Symbol guard completes, then the full account CLI
  still fails; it is not a completed workflow. Continuation: Codex computer
  PASS0.674s,Compatible Responses computer FAIL1.071s,Auth publication
  PASS2.542s,account CLI FAIL5.286s,shared-storage CLI PASS69.204s.
- Distinguishing evidence: a zero-Cargo3.899s repeat saves3 synthetic journals.
  V2 journal003 identifies toolResult d6b78d2e as the failing entry; v1 identified
  assistant16489bb5. Compare source entry IDs/roles, not only the shared error
  string. The first repair reached the next consumer boundary.
- Tool-result source cause: absent provider metadata becomes own Undefined in
  actual result coercion and emit_result. Strict live export rejects it while
  persisted JSON omits it. Valid saved receipts do not represent every native
  own property. Keep this distinct from the original assistant Symbol cause.
- Scoped next repair: enrich the existing disposable compaction model DTO with
  original tool-result details/useless; preserve all typed fields and prunedAt
  semantics, strict selected-detail failures and canonical identity. Do not
  weaken checked_raw/export, alter the producer or change default budgeting.
- V3 FINAL14 source review7ed18971 and driver PREb6226a9e approve the source
  scope and bounded plan, not execution.23 exact historical function successes
  plus5 required fresh checks may support28 distinct functions only after
  delivered runtime and independent point audit. V3 results remain pending.
- Retention: old failure receipts and executable hashes remain preserved;
  Cargo target paths can be overwritten by later builds, so their old executable
  bytes are not asserted to remain available without a separate archive.

### Final controlled Host confirmation

- Final actual CLI and native guard pass after the second targeted DTO extension.
  The assistant private Symbol stays own Undefined; the tool-result optional
  metadata stays own Undefined; strict checked_raw/export still reject both.
  Derived compaction views retain arguments/payload/details/useless, reject
  selected BigInt values, and preserve original Graph identities. Custom and
  other roles retain the prior strict path.
- The real ordinary CLI executes identified/email browser/device WRITE/READ,
  same-Session process restart, compaction budget checks, exact-row refresh and
  logout denial. Controlled fake upstreams do not establish actual OpenAI login.
  Independent Codex point audit2e5732f3 approves only this bounded contract.
- Final build423.257s (502 reused/7 compiled), five fresh functions152.449s;
  three-build total2124.762s and all test/diagnostic time377.272s preserve the
  failure/rework cost. Different selections are not a compiler-speed benchmark.
- Status: verified on the delivered controlled Windows Host and final CLIe8145ea6;
  full NativeGraph/Auth/OMP and live account/product gates remain open. Source,
  exact runtime/audit hashes and temporary-artifact limits are in the evidence.

## CASE-20261014-ARA-URI-media-and-link-recovery

- Identity: b91db66e upgrade over596f2da; adopted dirty Rust snapshot,
  preserved index and1200 source pins. [Raw failures, intervention and receipts](../evidence/omp-upgrade-uri-mock-20261014.md).
- Observation: four-target joined build timed out after2402.126s. Raw Cargo
  still emitted complete READ/Host/Mock test images; no successful build-finished
  or current ordinary CLI. A whole batch timeout does not invalidate completed
  compiler-artifacts, nor does it permit using an old CLI for new Host acceptance.
- Hypothesis: concurrent large links and observed D queuing contribute to waiting.
  This is an inference from a limited process/disk sample, not a complete measured
  performance cause. Keep toolchain/profile/features/target stable and serialize
  necessary target builds; do not clean incremental/download caches.
- Diagnostic execution: first five new READ URI labels passed; the SVG assertion
  failed. Actual source checks inspect_image before media decode and returns
  metadata under that policy. The fixture had enabled Inspect for SVG/PDF while
  asserting decoded Image output. The production path matched the intended policy.
- Intervention: test-only SVG/PDF default policy plus separate Inspect video
  context; existing codec/page/frame/ffprobe/no-extraction assertions retained.
  Single-target READ build119.907s,459 reused/1 compiled;8/8 PASS0.870s and
  independent bounded EXECd2a865a3 confirm this fixture prerequisite diagnosis.
- Reuse: the unchanged Mock image replayed7/7 in0.114s without Cargo; actual
  source bridge and independent full-point audit841eedea accept this local module.
  This does not erase the failed joined build or accept Host/ACP/complete READ.
- Historical pending gate: current ordinary CLI and binary-owner execution was
  required before Host closure; final Host/owner receipts below close those
  selected gates. Driver BASE initially duplicated a directory and failed before Cargo;
  preserve the failure/superseded PRE and require the corrected driver hash.
  These measurements compare different selected targets and do not establish
  a claimed20x overall compiler speedup.

## CASE-20261014-ARA-ACP-fixture-prerequisites

- Scope: fixed596f2da to b91db66e, current dirty Rust Host; all original FAIL
  packets are retained in [Host recovery evidence](../evidence/omp-upgrade-uri-mock-20261014.md).
- A malformed global config during pending approval fails Native refresh before
  autosave. The existing rollback error is expected; it cannot prove the later
  autosave getter catch. Isolate the getter failure in the real private consumer
  with temporary project config and a valid-save control; keep actual Host
  rollback/recovery as its own branch. No production lifecycle shortcut is added.
- Windows authored temp path and CLI canonical cwd may spell a directory with
  different case. Original and new exact-parent alias guards agree with upstream.
  Canonicalize executor TEMP/TMP to satisfy the same-root fixture premise rather
  than broadening production matching. Actual alias passes3.929s with0 Cargo.
- Default xdev and auto-enabled astEdit can supply device WRITE before ACP client
  capabilities bind. The original local EDIT-bypass source test does not assert
  model tool visibility. A supplemental hidden-WRITE matrix must explicitly
  isolate those supplier settings in that case, while keeping all original
  edit/denial/mode/identity assertions and separate default-path groups.
  Independent source diagnosisc747137f confirms this prerequisite mismatch.
- The first reviewed refresh-error fixture used `join("agent/config.yml")` in
  an exact message assertion, while the native producer uses two path joins.
  Windows display separators differ although filesystem targets agree. Fresh
  Host build146.608s/504 reused/1 compiled passes16 healthy labels before this
  assertion fails. The same precondition is checked in the unexecuted owner
  getter path before another link. This is test correctness, not a demonstrated
  production failure. Source review had missed the platform display mismatch.
- Final intervention succeeds: Host build216.215s/504 reused/1 compiled,
  autosave16.291s/all22labels and hidden-WRITE1.542s PASS; owner-only build
  403.418s/503 reused/1 compiled and test0.130s/five markers PASS. Independent
  final point audit8cb953c1 accepts only the bounded delta with unchanged-source
  bridges. Default transport restoration on plan exit is separately OPEN;
  no full ACP/READ acceptance. Preserve both reviewed misses and failed packets.

## CASE-20261014-ARA-READ-publication

- Status: decoder and attachment native groups pass; actual attachment Host
  verification independently audited. Complete READ remains open. Created
  October 14, 2026, Asia/Taipei.
- Identity: fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`, dirty ARA
  worktree. The original actual Windows CLI SHA and raw receipts are in
  [READ publication evidence](../evidence/read-policy-publication-20261014.md).
- Observation: ordinary image policy checks completed thirteen Runs; switching
  to an admitted model with `imageInputDecoder: stb` failed before publication.
  The batch is FAIL, not a completed READ module.
- Initial distinction: model admission alone could not prove executable model
  selection. Current registry metadata preserved the field, while the daily
  adapter classified it as an unimplemented wire option.
- Evidence: raw `set_model` error, current resolver rejection/projection and
  existing READ Host metadata consumer; independently verified fixed image
  helper source. Decoder is a Host image policy, with existing schema vocabulary,
  rather than a provider wire parameter. No extra decoder or wire implementation
  was needed.
- Action: four narrow projection/admission hunks and one concentrated existing
  native test group. Independent Codex SOURCE PRE/POST approved; Root adopted
  exact candidate files and preserved the Git index. Actual repaired CLI switching
  to stb and back now produces physical PNG200x200 and original WebP bytes.
- Counterevidence: an early proposed no-API model fixture was invalid under
  existing business validation and was discarded. The supported raw override
  path instead has a configured API and no matching catalog model.
- Verification trap: the physical journal is mutable under native local
  reduction. Comparing every old RPC image with only the final journal mistakes
  intentional supersession for loss. Preserve the original physical journal at
  each Run, compare actual blob bytes there, then independently check the final
  eligible supersession notice, provenance and `prunedAt`. Do not skip old
  receipts or allow arbitrary replacement.
- Efficiency: original bounded process batch 32.271 seconds, 26 localhost
  responses, zero Cargo and CAS. There is no comparable before/after speed
  baseline yet. Current strategy combines directly dependent fixes before one
  selected-module build, keeping cache/profile/features stable.
- Additional source root cause: the attachment callback originally asked the
  factory's initial builtin for a bound Run after catalog refresh. The active Run
  belongs to the existing execution_source/current catalog snapshot instead.
  Reusing that existing owner closes the observed expired-locator error without
  changing graph history validation or extending weak-reference lifetime.
- Observation boundary: agent_end precedes the Host's later serial completed()
  callbacks. Comparing journal bytes immediately can capture legitimate prior
  READ supersession while a subsequent rejected command is in flight. A round
  trip through the existing command loop fences completed(); assert idle/model
  identity first and keep the rejection byte equality strict. This is not a
  reason to allow arbitrary journal mutation or add sleeps.
- Measured verification cost: latest build520.727s with506 reused/5 compiled;
  four native tests1.559s; actual attachment7Runs51.494s. Driver corrections reuse
  the audited ordinary binary and incur zero Cargo. Changing feature/profile
  identity while diagnosing a script failure would invalidate unrelated caches.
- Bounded policy/PDF/environment process matrix now passes23Runs72.475s with
  unchanged source/binary and zero Cargo/CAS. Actual PDF full saved1706x960 and
  returned second-stage1568x882 differ in both dimensions and bytes. Independent
  execution audit passed for these23 cases; physical outputs are linked in the
  evidence page. The unchanged negative journal assertion now passes after the
  observation fence, providing intervention evidence for this sampling race.
- Remaining: complete READ original coverage, same-process environment mutation,
  mounted xD vision and real-model boundary. No product/release or full
  fixed-marker claim.
- Review: `/root/read_policy_review`; source POST SHA and exact adoption receipts
  are linked in the evidence page. A successful source review does not substitute
  for runtime validation.

### Delimited READ verification inputs and incremental build boundary

- The selected READ build reused507 artifacts and compiled5 in460.139s; nine
  tests took0.532s and exposed three verification-input errors. These timings
  distinguish compilation/linking from test execution; they do not establish
  a full-build speedup ratio.
- Core ZIP routing needs the existing Host calendar port even when a fixture
  does not test timezones. The group installs a controlled valid instant while
  retaining real ZIP creation, member reading and literal path precedence.
  This controlled port does not verify actual Host Date behavior.
- Nonraw :2 includes surrounding context. A selected-line-only assertion must
  request :raw:2-2; changing production selection to fit that assertion would
  violate the fixed source contract. Only the test path/expected target changes.
- The native image fixture needs explicit Bun/Host entry/project bindings.
  Supplying them passes the same pinned executable in3.147s with zero Cargo.
  Reused Host initialization also writes project autoResize=true, overriding
  the earlier home setting. Choose the private exact-byte fixture policy before
  launch and retain its effective config; do not disable product image policy.
- Fixed session-persistence.ts at596f2da (SHA4bb62421) leaves image base64 below
  1024 characters inline. The70-byte tiny PNG has96 base64 characters. Its
  persisted inline bytes are valid evidence; requiring a blob for it invents
  a different contract. Preserve the failed oracle and audit retained physical
  journals/raw results with the correct source predicate before any replay.
- deny.toml and THIRD_PARTY_NOTICES.md are delivered metadata outside the
  existing binary source inventory. Check their exact reviewed hashes separately
  from compiled inputs, with a closed explicit file list; never exempt missing
  Rust inputs or present metadata checks as executable dependency acceptance.
- Test-only repairs use the same packages/features/profile/target and compile
  the affected harnesses without read_host_native, preserving the ordinary CLI.
  Host evidence reuse requires exact production/input pins and an explicit
  two-test-file bridge. Original failures and complete READ gaps remain visible.
- Terminal repair batch measured243.972s compilation with507 reused/2 compiled,
  six groups/nine tests4.775s, total267.062s. Omitting the ordinary-CLI dependent
  integration target avoids its additional link for a test-only correction.
  This observation does not measure a clean-build speedup. Independent corrected
  Host audit4310cf90 accepts the existing four-Run inline-image evidence while
  preserving the original false-oracle FAIL; no Host replay was needed. Exact
  receipt identities and remaining coverage are in the linked evidence page.

### Plan-reference alias verification and dispatch ownership

- Fixed OMP read.ts744–760 uses the current plan reference before approval.
  A source-backed missing-file alias must consume that live owner and preserve
  lexical path priority; approved-only gating would change the upstream contract.
- Bare `:raw:N` does not imply a single line. New exact-line fixtures need
  `:raw:N-N`. Initial two-group FAIL was an input/expectation mismatch, not
  evidence that production must discard the remaining file. Only new inputs
  were corrected after independent source review; historical FAIL stays visible.
- A registered Core ContentUriPort is not proof that Native xdev dispatch can
  reach it. Enabled Native Write routes xd URIs through the device catalog first;
  default ACP has no mounted propose member. Root and independent Codex source
  diagnosis agree. Default Native ACP planning remains an implementation gap.
  A disabled-xdev private Host fixture can verify only the disclosed Core route.
- Fixture overrides must retain their original effective configuration. Root
  rejected an unadopted one-line override that dropped LSP/edit settings while
  adding xdev=false; appending the requested setting avoids a hidden second change.
- Measured module compilation:488.396s/504 reused/5 compiled, then86.796s/
  507 reused/2 compiled after test-only repair. Production libraries and CLI
  remained fresh in the latter. This measures narrower changed inputs, not a
  clean-build speedup. Core took0.111s; Host27.243s ended at an unmet route
  prerequisite. Extend neither the deadline nor unrelated test coverage to
  diagnose a dispatch error. [Exact failure receipts and limits](../evidence/read-policy-publication-20261014.md).
- The append-only private correction compiles only one Host harness:49.0s,
  508 reused/1 compiled. Actual Host group then passes15.007s/9Runs/23responses
  with unchanged CLI/Core artifact. This intervention verifies configured
  ContentUriPort dispatch; it does not repair or accept default Native mounting.
  Root inspected the raw/final receipts and6 legitimate read supersessions;
  independent execution POST15cbe861 approves this bounded configuration.

### Profiler review before the shared module build

- Reviewing a nested regular expression requires its outer capture contract.
  The sample THREAD_RE may be unanchored, but its input is produced by fixed
  `/^(\d+)\s+(.*)$/`. Clipping an embedded terminator incorrectly accepts an
  entire line that the outer expression rejects. Independent review identified
  the error and Root verified the fixed source before changing the D candidate.
  Original21 profiler assertions are retained; runtime validation is pending.
- Source ownership is sufficient to reject a Store redesign. Fixed Native
  record_snapshot accepts Rust String and stores Arc<str>. Two bounded installed
  runtime probes observe UTF8 BOM/replacement/CRLF decoding and NAPI lone-unit
  replacement versus astral preservation. Exact WireString rendering remains
  separate from the native snapshot conversion. These observations are package/
  source/API-boundary evidence, not reproducible-binary or complete READ evidence.
- The shared in-memory helper has a PDF regression surface even when its
  caller becomes a thin wrapper. Independent review checks observable text,
  selectors, truncation continuation, snapshot/seen lines and abort behavior;
  metadata alone cannot substitute for the prior continuation text. Internal
  immutable resources must keep their independent routing and limit policy.
  Combine source corrections before one module build rather than compiling
  each review finding separately. Source review is still in progress.

### Profiler terminal diagnosis and compilation scope

- Source version: fixed596f2da, delivered READ unionv9. Root verified the
  real code path in rpc_host1929 and actual_message_projection157: public
  projection happens before journal append. json_data's scalar serde_json
  bridge rejects escaped lone UTF16 and RunSink records persistence_failed.
  The failure label alone therefore cannot identify the journal writer as
  the cause. New Host execution stops before CPU ToolResult persistence;
  exact Wire pipeline reuse remains under independent review.
- The initial E0509 build98.689s produced no executable. Its reviewed
  single-file borrow/clone repair keeps WireString exact and avoids moving
  out of a Drop value. Incremental retry689.638s reused504/compiled7 artifacts;
  eight selected groups executed in2.018s,6PASS/2FAIL. This is an incremental
  observation, not a clean-build comparison or a complete-module acceptance.
- Avoid repeated compilation for diagnosis: inspect retained stderr/physical
  journal and source first, combine affected repairs, and select targets by
  changed dependency scope. Keep original tests; reuse unchanged passing
  receipts only with source/executable identity checks. Cases and transient
  results are linked from the existing READ evidence/handoff; no new111 item
  is accepted by these narrow receipts.
- A test target name is insufficient to choose its process dependencies.
  The shared runner initially required an LSP artifact for every ara bin test;
  this selected journal-failure family actually invokes a retained real LSP
  process helper. Disabling the fixture check would hide that prerequisite.
  A private reviewed fallback binds the known historical fixture image and
  source pins while keeping default Cargo features and honest compiler counts.
  Its prior lock differs: disclose controlled helper reuse, do not call it a
  new delivered artifact. Root preflight and independent PRE approve the
  prepared runner; the newly selected Host execution remains pending.
- Independent source POST caught a reachable Value/Wire retry comparison type
  mismatch and a new legal-large-u64 refusal in an unchanged legacy printer
  contract. Repair the DTO conversion and preserve the direct legacy Value
  entrance before compiling. Exact v2 delta review closes source findings;
  actual build and runtime remain separate. Source preflight also caught a
  nonstandard delivery-state label before Cargo; keep the supported state
  spelling and record runtime pending in its own field rather than inventing
  new state labels for the same source delivery.
- A public cache carrier change reaches multiline test readers too. Searching
  only `session.messages` misses chained `.messages` reads. The actual bin-test
  build found three errors in two remaining cfgtest adapters, while production
  and integration executables completed. Check multiline cache readers and
  retain their complete expectations. The reviewed fixes leave the production
  prefix exact; only the bin-test target needs compilation. Reuse the valid
  produced images through an explicit source-impact/receipt bridge, keeping
  the aggregate failed build failed and real compiler counts unchanged.

### Profiler model sanitizer versus display fidelity

- The next delivered physical journal disproves public-cache loss: all four
  tool receipts persisted, no prunedAt; CPU content has719 UTF16 units while
  displayContent has722. Fixed sanitize-text.ts23-28 and agent-loop coerceToolResult
  sanitize model content only. Rust tool_sanitize.rs58-72 and actual_execute.rs377
  preserve that boundary. Two explicit lone high surrogates and one surrogate
  cut by the tree display limit disappear from content; the valid astral pair
  and the exact raw display survive. The newly appended Host oracle incorrectly
  required raw display equality for sanitized content. Source-backed expected
  behavior must be checked before proposing a production cache repair.
- Save existing raw frames, model requests, journal and settlement before
  final publication assertions. A failed test after a successful process can
  otherwise leave only its journal, with the in-memory requests unavailable;
  do not claim those missing receipts were inspected.
- Cargo integration tests also request the normal binary. Even cfg(test)-only
  edits in its shared source can invalidate that binary's fingerprint. Keep
  package/profile/features/target identity stable and read artifact.fresh;
  selecting two tests is a scope decision, not a promise of zero binary linking.
  Never alter mtimes or compiler receipts to force apparently fresh reuse.

- The two-integration-target repair actually compiled three targets, including
  the ordinary CLI:234.619s,506 reused/3 compiled. Its Gate passed but a newly
  written expected-tail helper lost a leading empty row by testing accumulator
  length instead of row index. Preserve empty rows when constructing expected
  slices. Check all later assertions against saved raw receipts before the next
  build; this found no other oracle mismatch without replaying the Host.
- The final retry selected only the Host target, keeping the same package/cache
  identity:94.714s,504 reused/1 compiled, normal CLI fresh=true/exact; Host2.892s
  PASS. This is reduced build scope, not an equivalent-workload benchmark.
  Independent EXEC POST83e6d245 supports1 fresh +7 source-bound historical groups.
  Full READ and the observed reopen-model decode warning remain OPEN.

### Typed mirror warning versus resumed Native model history

- Fixed596f2da, delivered READ/ACP unionv15. Source investigation distinguishes
  the legacy Entry.model_messages typed decode from the Native wire/graph
  restoration and actual provider projection. The CPU presentation contains
  three lone UTF16 units (display722; sanitized model content719). A failed
  whole-message mirror can warn even when the actual graph path retains model
  content. Warning text alone is insufficient evidence for a recovery rewrite.
- A fresh process using the existing delivered CLI resumes the original
  byte-copied Session. Actual loopback request preserves exact CPU719 and the
  original paired assistant call, while journal display722 remains intact.
  Warning persists; the controlled answer succeeds and the owned process
  settles. This is bounded runtime counterevidence to CPU-history loss, not
  complete READ or all-message recovery acceptance. See the existing
  [READ evidence](../evidence/read-policy-publication-20261014.md).
- This source-independent driver executes with zero Cargo in2.568s. The prior
  ACP source batch takes758.711s with505 artifacts reused and5 compiled; its
  tests take32.264s. Incremental cache reuse and fast execution do not eliminate
  linking cost. Group related edits, preserve build identity, select affected
  targets, and reuse current pinned binaries for external Host checks rather
  than re-entering Cargo for driver changes. Complete gates still require their
  applicable executable evidence at the integrated delivery checkpoint.

### Upgrade target provenance and integration-test compilation cost

- Exact upgrade596f2da→b91db66e,2026-10-14. New source is a separate frozen
  checkout, with sync_target recorded independently from immutable baseline,
  reviewed source and complete ported-through markers. File classification
  under the old111rules does not prove unchanged behavior or new acceptance.
  Retain historical receipts with their original SHA, then reconcile executable
  changes per module. See [upgrade ledger](../upstream/changes.md).
- The two Mock terminal-field additions use native get/assign handles and
  shallow error spread. Existing native identity/response-scope tests plus the
  original25 and source banks pass in one target:6Rust tests, not31test functions.
  Source and independent execution review preserve the distinction from actual
  fallback-credit/provider/Auth capability usage. A native object fixture proves
  transport identity, not credit consumption or authorization.
- One selected integration target still builds the ordinary CLI as its Cargo
  dependency. Observed build802.722s,496reused/9compiled; tests0.150s. Changes
  in ara-ai invalidate its consumers even when only two field assignments change.
  Target selection reduces test scope but cannot promise only one compiled
  crate or no normal-binary link. Group related AI edits before rebuilding;
  reuse current physical images for source-independent checks. Do not invent
  a measured no-bin workaround or alter Cargo layout/features during a small
  reproduction patch. Independent EXEC8a34d415 confirms the actual counts.

### Retry fixture prerequisites and false negative-path acceptance

- Exact upgrade596f2da to b91db66e,2026-10-14. The new Responses coarse fixture
  first passed its typed cases but failed actual transport with ValueNotCallable.
  Its bare ValueHeap lacked Array.prototype.indexOf, which the reference
  NativeRealm installs. Bind the existing public intrinsic on the same heap;
  do not repair a fixture environment by changing provider semantics or copying
  an identity-scanning algorithm. After that binding all14 Compatible scenario
  markers execute successfully. The entire17-scenario test remains incomplete.
- The next failure is served()=0, not missing tool events: the fixture called
  Codex with model.api=openai-responses. Codex's existing public request encoder
  requires openai-codex-responses and rejects before HTTP. A generic Error
  assertion alone passed this configuration failure and could have masqueraded
  as the intended stream-disconnect case. Preserve request-count, tool lifecycle,
  terminal and replay evidence assertions, and validate the public request
  builder before sending this negative-path fixture.
- Root and independent Codex retry_independent diagnose both omissions from
  raw outputs and exact source. PRE had missed the heap binding and wrong API;
  retain these review misses and all FAIL receipts. v5 fixes test prerequisites
  only; its whole-group execution is pending. Historical19 tests are audited
  separately, with explicit source bridges; partial14 markers do not add a
  successful Rust test or accept a new upstream surface.
- Last measured single-target build337.463s,154 reused/1 compiled; test5.589s.
  Runtime-root-v3 FAIL is preserved. See the [current handoff](../handoffs/2026-10-14-omp-upgrade.md)
  for v5 source/review pins and the pending single-target packet. These measured
  failures support checking the full fixture prerequisite chain before linking
  again, not reducing mandatory behavior coverage.

- Final intervention completes all17 rows after the API prerequisite correction:
  build543.790s,154 reused/1 compiled; test6.185s. Independent Codex EXEC2591a4dc
  approves the bounded delta and extends19 historical/source bridges to the
  final fixture snapshot. This confirms the prerequisite diagnosis within the
  authored matrix, not full Host/model/provider acceptance. Full failed packets
  remain. [Final evidence and limits](../evidence/omp-upgrade-retry-20261014.md).

### Chat transient flags and local retry have different source owners

- Fixed b91db66e JSON body probes attach transient classification, but the local
  `empty-completion-retry.ts:157` reconstructs an error from message/status.
  Root and independent Codex initially inferred a missing flag side channel;
  exact `error/retryable.ts` and `utils/fetch-retry.ts:515-519` disproved that
  inference. The proposed shared retry/provider constructor changes were
  removed before adoption. A statusless `Throttled. Please retry` can therefore
  be transient for Session routing while retaining one local POST. A structured
  429 can use the existing local retry. Classification success does not prove
  which retry layer owns another request.
- Existing Host slow-callback tests encoded the old Chat timeout explicitly.
  New-source migration retains exact callback settlement and POST count and
  verifies successful text; consumers must be included before the grouped
  build. Root and independent source POST68209eeb approve six adopted paths;
  fresh runtime/point acceptance was pending at that first-adoption checkpoint.
  The final acceptance below supersedes it. Scalar UTF-16/HTTP-prefix edges,
  non-JSON, full Chat and actual account trials remain unaccepted. See the
  [versioned Chat evidence](../evidence/omp-upgrade-chat-20261014.md).
- Source review missed a closing brace in a new Host `json!` fixture; the first
  affected-target compile spent372.092s before exit101. Preserve that failure
  and the independent review miss. Six exact changed files subsequently passed
  non-mutating Rust syntax parsing in under a second before the next build.
  This is a cheap fixture prerequisite, not a replacement for type checking,
  actual tests or behavior audit. Narrow source POST v2 verifies the one-byte
  intervention with unrelated raw bytes and index unchanged.
- Two supplemental expectations also conflicted with actual owners. The
  normalized AllocationQuota code is appended to the message and the fixed
  quota classifier consequently adds UsageLimit; attaching Transient does not
  suppress that classification. Graph `failureEvidence.replayBlocked` reports
  committed output, while the Host `RetryClass.replay_blocked` reads the separate
  `same_route_blocked` permission. Root and independent diagnosis69922798
  correct only those fixture assumptions, add an explicit Graph flag check,
  and preserve POST/content/credential/settlement assertions and all failures.
  The final audit accepts62 distinct successes,60 historical/source-bridged
  and2 fresh; build157.182s/504 reused/2 compiled, tests2.789s. This validates
  the owner distinction and bounded intervention, not full Chat or Agent.

### Snapcompact lazy images: typed projection lost runtime attribution

- Fixed upgrade596f2da to b91db66e,2026-10-15, dirty worktree preserved.
  New lazy archive selection and source reconstruction passed their new real
  filesystem fixture, but the original reopen regression failed at
  `ara-session/tests/snapcompact.rs:296`: `image.compaction_frame` was false.
  Root checked the DTO declaration: serde intentionally skips this runtime
  field. The new exact-source-to-typed conversion deserialized the field away;
  the previous manual converter explicitly set it true. This was a production
  regression in the candidate, missed by initial independent source review.
- Restore attribution only on the derived archive User image blocks after
  the checked conversion. Do not add it to durable raw images or relax the
  old assertion. Session v3 passes the original seven snapcompact tests, one
  context-view family and four selected persistence/failure unit tests.
  Build8.090s,171 reused/4 compiled; snapcompact execution36.430s.
  This supports the diagnosed conversion boundary, not full CTX/model parity.
- Root also failed to inspect a candidate-import schema error before starting
  the dependent run. The fix had not reached E, so the old image repeated its
  same failure for36.032s. Both failures remain recorded; a zero-rebuild old
  image is not a repaired-source pass. The importer now handles the declared
  list/single-file schemas, and the repaired run verifies the actual source
  hash before Cargo. This is an observed local correction, not a promoted Skill.
- A separate taxonomy review found that namespace rescore with no winner
  incorrectly cleared an existing family tie. Fixed upstream requires a new
  winner and no new tie; the one-condition correction and strict/lenient
  observations pass in the final Host-linked catalog bank. Independent final
  EXEC77b14236 and complete-point audit25a4e5a1 accept the full package and
  declared Windows Host dependency scope; full CTX/model/product gates remain.
  See [source, failed outputs and verification boundaries](../evidence/omp-upgrade-snapcompact-20261015.md).

## Case 20261019 ARA local ToolError display boundary

- Scope: fixed b91db66e Child/Peer49-path composition41655d70. The actual
  session60094 compiler stops in ara-tools at a new URI dispatch wrapper's
  `write!(f,"{error}")`: E0277,58.855s,487reused/0completed new artifacts,
  zero tests. Source/runtime/index remain exact; RESULT1e4cb223 and all raw
  diagnostics are retained. Syntax/source approval had not established type
  correctness. This is a local Rust integration defect, not upstream behavior.
- Actual `ara-agent/src/tool.rs:226` defines ToolError(pub String) and a blanket
  From<T:Display> implementation. The adapter needs the existing message payload;
  changing the shared error trait contract would expand scope. Use the String
  locally. A concentrated source check finds the same mismatch in main's
  borrowed persistence_failure call and RPC's owned persistence_failed call.
  Repair all three conversions together; preserve receipt classification,
  failure control flow and original errors. No new test target is introduced.
- Independent wrapper POST35ba16f0 and combined POST3254f24d approve source-only.
  Root adopts final-source-v2manifest7cca6264, three hunks/three files,
  46unchanged mtimes,1362pinned inputs; only3files are freshly parsed,36parse
  results are inherited by exact hashes. The three changed paths have no
  intersection with the previously reviewed AI293/publication142 closures.
  Session65753 performs the same single CLI affected-target build and fails
  at the next layer after89.706s,497reused/1compiled,0tests. Its actual three
  diagnostics identify two further local declarations: missing qualification
  of ToolOutput and ambiguous marker-result error type. Only ara-tools emits a
  new compiled artifact; the main/RPC binary callbacks have not compiled yet.
  This passes the first wrapper boundary while disproving complete type
  correctness. A broad static
  precheck did not establish that missing boundary; use the actual affected
  compiler diagnostics for the next concentrated correction. Real-process
  tests and complete Child/Peer acceptance remain pending; retain both failures.
- Evidence and follow-up: [combined Child/Peer receipts](../evidence/omp-upgrade-child-native-contract-20261018.md).
