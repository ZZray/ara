# Windows Bash process-tree and deadline investigation (open)

## Observed behavior

On local Windows with `C:\Program Files\Git\bin\bash.exe` on `PATH`, the
current `ara-tools::bash` call uses a Unix process group only under
`cfg(unix)`. Windows calls `child.start_kill()` on the direct Bash process;
descendants can keep the merged output pipe open and continue effects, even
after the tool reports a normal exit. The
module's whole-group lifecycle contract is therefore not satisfied on
Windows. This is a tool correctness issue and blocks the full backend gate.

The stable Bash lifecycle is outside the preceding Agent/AI/compaction code
slices. The repository's `AGENTS.md` requires explicit scope confirmation
before changing a stable lifecycle; a Windows-only Job Object repair has
been proposed. No process-lifecycle code has changed in this investigation.

## Reproduction on `dev` at `1b29745`

- `cargo test -p ara-tools --test tools
  dropping_a_bash_call_kills_its_group -- --nocapture` failed at
  `crates/ara-tools/tests/tools.rs:249`: the child wrote the `after-drop`
  marker after the Bash future was dropped. Exit 1.
- `cargo test -p ara-tools --test tools
  bash_timeout_kills_process_group -- --nocapture` failed at
  `tools.rs:166`: elapsed 3.008 s against the existing 2.8 s bound (1 s
  command timeout plus the 2 s reader wait). Exit 1.
- `cargo test -p ara-cli --test e2e
  deadline_during_a_tool_and_zero_budget_exit_nonzero -- --nocapture`
  failed at `crates/ara-cli/tests/e2e.rs:892`: the real CLI output capture
  exceeded its 4 s deadline, with the focused test taking about 5.5 s.
  Exit 1. This test drives fake HTTP upstream → Agent → Bash → CLI output.

Independent Codex investigations read the Bash, Agent and CLI paths. One
confirmed with a separate local process probe that killing Git Bash left
descendant Bash and `sleep` processes alive, then cleaned up those probe
processes. Another observed `ara.exe` reporting `Deadline exceeded` before
the descendant's inherited pipe handles reached EOF. This distinguishes
Agent cancellation from incomplete Windows process-tree termination.

At the pinned OMP commit, `packages/coding-agent/src/exec/bash-executor.ts`
uses native `Shell.abort()` on abort/timeout. Its `crates/pi-shell` SpawnRegistry
and Windows process-tree code track child identity and terminate descendants.
ARA's direct Bash launcher is an intentional implementation difference, but
its single-process Windows kill does not meet the documented effect boundary.

## Focused Windows recheck on `4080ca5`

An independent Codex investigator repeated the focused checks with Git Bash
on `PATH`. The real CLI deadline test failed again (5.43 s versus its 4 s
limit). The Bash timeout test failed (3.019 s versus 2.8 s), and the
background-child reap test failed (2.183 s versus 1.5 s). The dropped-call
test passed this one run; its earlier post-drop marker failure above remains
unresolved. Without Git Bash on `PATH`, the CLI test instead failed quickly
because Bash could not start, which is not a deadline pass. The full verifier
supplies Git Bash on `PATH`. These isolated failures make full-suite load an
insufficient explanation. No lifecycle code changed during this recheck.

| Focused command with Git Bash on `PATH` | Observed |
| --- | --- |
| `cargo test -p ara-cli --test e2e deadline_during_a_tool_and_zero_budget_exit_nonzero -- --exact --nocapture` | Failed at the `< 4 s` assertion; test ran 5.43 s. |
| `cargo test -p ara-tools --test tools bash_timeout_kills_process_group -- --exact --nocapture` | Failed at the `< 2.8 s` assertion; test ran 3.019 s. |
| `cargo test -p ara-tools --test tools bash_keeps_stream_order_and_reaps_background_children -- --exact --nocapture` | Failed at the `< 1.5 s` assertion; test ran 2.183 s. |
| `cargo test -p ara-tools --test tools dropping_a_bash_call_kills_its_group -- --exact --nocapture` | Passed once; earlier delayed-marker failure remains relevant. |

## Proposed bounded repair and gate

Confine the repair to Windows process containment in `ara-tools::bash`, its
Windows dependency if needed, and existing Bash/CLI regression tests. A
per-call Job Object must contain Bash from process creation, so descendants
inherit membership, and terminate them on normal
exit, timeout, cancellation and dropped execute future. Job assignment
failure must be reported before an uncontained command runs, not treated as
successful containment. Assigning a Job only after ordinary spawn leaves a
startup race. Keep the
Unix process-group path and CLI deadline semantics unchanged.

Microsoft documents that child processes inherit a parent's Job membership
by default and that `PROC_THREAD_ATTRIBUTE_JOB_LIST` can assign Jobs during
process creation (Windows 10 / Server 2016 or newer):
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[UpdateProcThreadAttribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute).

After authorization, rerun the three focused cases above, the unfiltered
`ara-tools` suite, real CLI e2e and full workspace verification. Confirm both
elapsed time and absence of delayed marker effects. The prior gate remains
open until the delivered code passes these checks and independent diff review.
Estimate after scope confirmation: 2–5 working days, including the unfiltered
verifier and independent review.

## Authorized Windows-only repair (WIP)

The user explicitly authorized this bounded Windows Bash lifecycle change on
2026-09-28. `ara-tools::bash` now starts Git Bash with `CREATE_SUSPENDED`,
assigns it to a per-call Job Object with `KILL_ON_JOB_CLOSE`, locates its one
initial thread through ToolHelp, and resumes only after assignment succeeds.
The existing Tokio command builder still owns quoting, environment, cwd and
the merged output pipe. A failed Job assignment has no uncontained fallback;
the suspended child is killed. Closing the Job on normal exit, timeout,
cancellation or Future drop ends descendants before the output reader waits
for EOF. The Unix process-group path and CLI deadline logic were not changed.
The only new dependency is the already locked `windows-sys` crate, enabled
for Windows in `ara-tools`.

Microsoft documents suspended creation and Job inheritance/termination:
[suspended threads](https://learn.microsoft.com/en-us/windows/win32/procthread/suspending-thread-execution),
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[assignment](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject).

Executed on Windows with Git Bash on `PATH`, `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`,
test/dev debug info and incremental builds disabled, two build jobs:

| Check | Observed result |
| --- | --- |
| `cargo test -p ara-tools --test tools bash_ -- --nocapture` | 5/5 pass: normal merged output/background cleanup, timeout, cancellation plus delayed marker, dropped Future plus delayed marker, ordinary success/error/env/cwd. |
| `cargo test -p ara-cli --test e2e deadline_during_a_tool_and_zero_budget_exit_nonzero -- --exact --nocapture` | 1/1 pass in 1.39 s; previously 5.43 s and failed the `< 4 s` bound. |
| Direct `Git\bin\bash.exe -c 'kill -9 $$'` | Windows process exit code 2304. The test now checks this observed Git Bash code on Windows while retaining the Unix 137 assertion; production exit-code mapping is unchanged. |
| `cargo test -p ara-tools --lib failed_job_assignment_never_runs_suspended_bash -- --nocapture` | 1/1 pass after bounding child reaps at 2 s: an active-process-limited Job with an occupied slot rejects a second suspended Bash; no `uncontained` file is written. |
| `python scripts/verify_backend.py` on the final repair snapshot | Owned format and workspace Clippy passed. CLI e2e 51/51, proxy discovery 6/6, ara-tools `tools` 16/16 and the failed-assignment unit passed. Full workspace test then failed in the existing vendored `pi-edit` `hashline_streaming_preview_cases_preserve_partial_and_final_contracts` case (9/10 in that binary, exit 101). The assertion was `streaming: recovers the bare header onto its nested file instead of blanking`, observed 0 vs expected 1. This is outside the approved Windows Bash scope and was already listed in the handoff. |
| `cargo test --workspace --doc --all-features --quiet`; `cargo deny check`; `python scripts/omp_inventory.py check`; `python scripts/verify_bootstrap.py`; `git diff --check` | All exited 0. Doc tests contain no cases; `cargo deny` retained existing duplicate-version and license-field warnings. |

Independent Codex plan reviewer recommended the suspended spawn/Job/ToolHelp
path and fail-closed assignment. Independent Codex post-diff review covered
Cargo.lock, `ara-tools` manifest, Bash implementation and tests; it found no
confirmed production defect. The independent post-diff reviewer also checked
the failed-assignment test and found no blocker; its suggestion to bound
`wait()` calls was applied and retested. The full backend gate is not green,
so this repair remains WIP and no point acceptance is claimed.
