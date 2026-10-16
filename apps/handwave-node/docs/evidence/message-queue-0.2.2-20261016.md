# HandWave 0.2.2 managed composer queue

Base: `576a1579a3f50eceb525e747b46b2f1f5bcae828`.
Scope: independent `apps/handwave-node` Host only; root Core/OMP WIP excluded.
Go presentation base: `05d09a4`, maintained stable Web overlay.

## Implemented behavior

The connection actor owns the existing FIFO, with at most16 pending messages
of8KiB UTF-8 each. The queue snapshot and control protocol is scoped by explicit
Session, connection epoch, revision and active Run. Edit changes the effective
message only; a structured legacy task objective is normalized before queue
admission so displayed edits cannot execute an old hidden objective. Models,
native Session, criteria and Goal authorization retain their original owner.

Remove/edit/front/steer wait for actual acknowledgements. Atomic steering moves
one pending item into the same active Run; applied denotes sourced text admitted
to execution context, not model compliance. Ending/canceled Runs report unapplied
guidance, and the UI offers explicit draft recovery. No implicit replay or new
durable scheduler is added. Controller detach clears this temporary queue.

## Executed local checks

Final source fmt and Clippy warnings-denied PASS;130/130 unit tests PASS with
`--test-threads=1`. The first parallel suite had127 passes/one old temporary-dir
name collision (`local_tools.rs` PID+time naming). A subsequent final-source
link failed with MSVC0xc0000142 before executing tests. Linker help and the
unchanged final-source serial test retry succeeded; no tool code or assertions
were weakened, and both failures remain in the evidence directory.

Frozen offline cargo-deny PASS (existing duplicate-dependency warnings, cached
advisory snapshot). Explicit static-CRT release:4,957,184bytes, SHA256
`da4b6d0586c3587e1d0e1c1e31909bddb02d572bc262211f6538b12510bc9ad7`.
Actual normal/delay PE imports were checked; no dynamic CRT is required.

`tests/verify_queue.py` on this EXE plus maintained real Hub Windows SHA
`98f1b0d5726a732a8c7e34bdff25ccb3f562784a0e99dedc491e9d93632f8939`
passes five process groups in14.936s/six controlled provider calls. It observes
the queue while the real model HTTP request is held, then validates FIFO,
edit/remove/front, stale epoch/revision/Run and cross-Session rejection, stable
pending identity, sixteen-item capacity, actual Python exclusive file effect,
steering source admission before native handover and handover before the next
Run's events.
Canceled-before-admission reports unapplied without requeue; disconnect gives
a new epoch and does not replay pending text. Removed/replaced/rejected messages
are absent from actual provider requests. One canceled HTTP10053 is explicitly
classified/recorded; idle HTTP connection-reset stderr remains in the logs.
The original fixture failed only on that unclassified expected cancellation and
is retained; the final fixture narrows the exception to the cancellation phase.

The exact final image also passes four native-Session process groups in20.033s
and three appointment process groups in25.721s/four controlled calls/two slices.
The latter keeps original root budgets and awaiting_review; its final paused
state belongs to negative-case setup. The pending-grant cancellation regression
reproduces on both the installed0.2.1 image and the unpublished first candidate:
only model-grant-end, no frontend terminal, zero model/tool effects. The scoped
fix claims one canceled terminal before stopping an admitted Run with no model
worker; timeout/denial ownership and late-grant rejection remain intact. New
steering outcomes flush before a next Run's handover. Final cancellation and
back-to-back controller evidence is retained in grant-final-v2: eight groups
PASS/16.201s, including three cancel→next-message trials with two actually
observed early active projections. In every trial the old out.done preceded
the next model-grant request, with exactly one canceled terminal and no new
model/tool effects. The UI therefore defers handover until its old out.done
has finalized the prior stream, even if a new native active snapshot is early.

Independent Codex reviewer `context_memory_ui` reviews exact scoped source and
receipts with ara-git-review and ara-rust-core-review. It identified initial-list
and mobile Enter UI problems, structured-objective ambiguity and cancel/handover
ordering; final review
and production evidence are recorded separately. Controlled-provider process
checks do not establish real-model intelligence, Linux/ARM/macOS compilation,
power-loss durability or production browser acceptance.

Evidence root: `D:/WorkSpace/tmp/handwave-queue-20261016/`.

## Production and browser acceptance

Final independent Codex review: `queue-independent-review.json`,
`APPROVE_SOURCE_PROCESS_AND_CONTROLLED_UI`; seven findings closed. The final
idle/send accessible-label delta was separately verified in
`queue-label-delta-independent.json`. Source/process review is distinct from
the coordinator's browser observations below.

The installed Windows image matches the release SHA above. The published ZIP
is `handwave-node-windows-da4b6d05.zip`, SHA256
`bbfcf23f6c9fc64b4cd358c5de4e014f448e4bd1df6055b67b6ab92d3ffb58fb`.
The source archive is `handwave-node-source-58b3a60433a3ab7f.tar.gz`, SHA256
`58b3a60433a3ab7f5d56b694f1134c35c4a2372da3c10440426373b7262bf423`.
Its runtime source snapshot precedes this documentation closeout; the immutable
published URL was not rewritten to add the later evidence.

Ray was built from clean prospective tree
`71151e813ffd6b77f51d61416ead640f92233d84` with the preserved stable Web overlay
SHA256 `f2a01bedb53562b86b5e410950f7c7c2363079a5e348a91c83edd92591e05511`.
Go test/vet/build passed. The final deployed Ray SHA256 is
`39a834285391b6101433e4fcb09339fec0a87350593e181dc3cf5391857695d2`.
`deploy-label-receipt.json` confirms thirty protected files unchanged, unchanged
Hub/OMP edge/Gateway/RainFlow processes and binaries, sixteen verified package
downloads, and preserved normal-login/application gates. Existing device grants
were not extended. The dirty local Go authentication/Web changes were excluded.

Real authenticated browser trial QUEUE022 executed one safe Python script, but
the first Auto Run subsequently received model HTTP400; its FIFO successor
returned QUEUE022-A, while a later message had already started before deletion
was attempted and received HTTP429. That message was not deleted. The late
`live-queue-desktop.png` does not prove queue layout.

A separate bounded trial QUEUE022B selected DeepSeek for that session only.
It ran one Python print/sleep/print request and returned QUEUE022B-READY. While
busy, three messages appeared above the composer. Actual browser actions
deleted the second with a native 'not executed' acknowledgement, edited the
first from EDIT-OLD to EDIT-NEW, and transferred the third into current-Run
guidance. The guidance receipt reported context admission, without claiming
model compliance. The edited FIFO successor then returned EDIT-NEW; the
deleted message did not appear as a subsequent chat turn. Native Runs were
FACCNAPFXE and ORZI3BAM34, respectively. No retry was requested.

Screenshots `live-queue-desktop-final.png` (1280x900 dark) and
`live-queue-mobile-light.png` (390x844 light) show the real pending queue.
At390px the queue occupies358px at x16 with scrollWidth356; buttons measure
36x44px. This is browser viewport emulation, not physical phone/IME acceptance.
Temporary viewport/theme/model changes were restored, including Auto.

`live-placeholder-final.png` shows the plain contextual suggestion without
'Tab fill' text. A real DOM-backed browser check observed placeholder
'结合当前对话，说明下一步可以做什么。'; pressing Tab filled that exact value
without sending. The draft was cleared afterward, and the idle button exposes
'发送'. Queue UI tests pass24/24, suggestion tests45/45. Reconnect/restart
durability, all Auto providers, and real Linux/macOS/ARM builds remain outside
this acceptance.

## Reproduction and documentation boundary

Executed from `apps/handwave-node` with Cargo target outside the repository:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked -- --test-threads=1
cargo deny --locked --offline check
python -B tests/verify_queue.py --binary <final-exe> --hub-binary <maintained-hub> --output <isolated-evidence>
```

The exact final-source check runner and outputs are retained in
`rust-delivered-checks.py`/`.json`; the process result records both EXE hashes
and its fixture hash. Scoped closeout rechecks18 independently reviewed source
pins,15 Go build-tree blobs and296 local document links. The repository-wide
`verify_bootstrap.py` is not rerun: its unrestricted Markdown rglob traverses
unrelated ignored trees. The scoped link check uses the same local-target
resolution rule and does not establish whole-repository bootstrap acceptance.
