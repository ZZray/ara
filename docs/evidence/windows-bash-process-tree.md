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
