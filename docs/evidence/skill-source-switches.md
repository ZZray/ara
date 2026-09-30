# Compatible Skill source switches

## Requirement and scope

User decision, 2026-09-30: provide four independent, host-persistable Skill
source switches. Defaults are Agents, Claude and Codex on, OpenCode off;
each switch controls user and project paths together. The reference CLI
uses that default and accepts an exact enabled set via `--skill-sources`.

This is an ARA configuration addition, not a claim of additional OMP parity.
The loaders remain grounded in fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/discovery/{agents,claude,codex,opencode}.ts`
and `extensibility/skills.ts`. OpenCode's user directory remains
`~/.config/opencode/skills`, rather than the screenshot's
`~/.opencode/skills`. See [configuration and SDK usage](../skill-sources.md).

Excluded: host UI/persistence service, live RPC settings commands, plugin or
managed Skills, new directory aliases, Skill provenance, provider policy,
context-file behavior, priority/dedup changes and Session lifecycle.

## Snapshot and file mapping

Base: `fb992dc2d57b5e19aea0e954a9e4864ff085ae7b` on `dev`.
The verification worktree contains only this feature on that base. Concurrent
system-prompt WIP, its old E2E hunk and problem-solving knowledge edits are
excluded. `C:/Temp/ara-skill-source-switches/snapshot.json` records SHA256
of the tested source/test files and the exact snapshot path.

| Files | Requirement mapping |
| --- | --- |
| `ara-discovery/src/skills.rs`, `src/lib.rs` | Serializable switches, selected defaults, legacy `None`, exclude disabled loaders and opt enabled user sources in for this Skill load only |
| `ara-discovery/Cargo.toml`, `Cargo.lock` | Direct dependency on existing serde for host-owned persistence |
| `ara-cli/src/main.rs` | Typed CLI selection with default; same prepared discovery result feeds prompt, tools and REPL/RPC setup |
| `ara-discovery/tests/skill_sources.rs` | 16 combinations, JSON/CSV, scanning witness, policy veto, same-name fallback, unaffected native/custom/GitHub/context controls |
| `ara-cli/tests/e2e.rs` (new tail only) | Actual CLI/provider/tools/Session request assertions and optional raw receipts |
| `docs/skill-sources.md`, knowledge context/index, this record | Host usage, compatibility boundary and executed evidence |

Key source SHA256:

These hashes identify the tested Windows checkout bytes. Git normalizes mixed
CRLF/LF to LF in three files; all ten snapshot paths were independently checked
against normalized staged bytes. The committed LF blob of
`ara-discovery/src/skills.rs` is
`c1a801a6a4e8a368d7c73e14abd864d8e29e8f5ebb1955ccdbfe0b2728243291`.

- `ara-discovery/src/skills.rs`: `713dccfed3614799ceaebf66814117b3cffe3616a8e032d0eb532cea3ab2dcb2`
- `ara-cli/src/main.rs`: `32daa12ed29247d21963556fc0db489723ead027041cef8413ed666cec4e12b6`
- Isolated `ara-cli/tests/e2e.rs`: `90e5a20961b2bfb99572f5801447ee866a4255e14088359b2f17c783a497106b`
- `ara-discovery/tests/skill_sources.rs`: `6201640e5b563668a977d5079f0de33bc715bff0e7ddd01915d897373f6155a4`

## Focused execution

- `cargo test -p ara-discovery --tests`: **58 passed, 0 failed**, including
  new **7/7** and the existing legacy-source fixtures.
- `cargo test -p ara-cli --test e2e skill_sources`: **5 passed, 0 failed**.
  These preliminary checks ran in the main dirty worktree. The final gate
  below runs the isolated delivered snapshot, including raw receipt export.
- The first parser test failed on empty comma-separated entries. The parser
  now ignores them, matching existing CLI `--skills`/`--tools`; unknown
  nonempty IDs still fail. The oracle was retained and the focused checks
  rerun successfully.

## Actual host oracle

The new E2E cases spawn the real Rust `ara` process and controlled HTTP
upstream with temporary home, project and Session storage. They inspect the
outbound system prompt, execute real `read skill://...`, inspect its durable
tool result, and compare the same content forwarded to the next model call.

- Default: six Agents/Claude/Codex user/project Skills are listed, OpenCode
  is absent; Codex user read succeeds and OpenCode read fails.
- OpenCode-only: both OpenCode levels load, other compatible sources are
  absent; OpenCode user read succeeds and Agents read fails.
- Empty selection: native Skill remains available; Claude user is unknown.
- Total gate: `--no-skills` removes native and compatible Skills.
- Name filter: only selected Codex project Skill appears and resolves;
  filtered Codex user and disabled Agents return `Unknown skill`.
- Invalid source: three actual CLI processes exit **2** with the flag and
  reason, **zero model requests and zero Sessions**.
- All five normal processes still carry the independent Claude context file.

The normal cases contain **11 tool receipts: 4 successful and 7 failed**.
Failures are intentional oracle inputs and remain failure receipts; the
controlled model then terminates the Run. Their success does not automatically
accept a Task. No model-dependent behavior changes here, so a new real-model
trial is not required for this source-selection point. Product UI remains
host-owned and has not been exercised by this repository.

## Final gates and independent review

Windows final isolated-snapshot command (the runner saves raw output and totals):

```powershell
$env:CARGO_TARGET_DIR='E:\repos\ara-github\target'
$env:ARA_SKILL_SOURCE_RECEIPTS='C:\Temp\ara-skill-source-switches\receipts'
python C:\Temp\ara-skill-source-switches\run_check.py --cwd C:\Users\loveu\.codex\worktrees\skill-source-verification\ara-github backend-final python scripts/verify_backend.py
```

- **PASS**, exit 0: formatting, strict workspace Clippy, target tests and
  documentation tests; **1,133 passed, 0 failed, 1 ignored, 87 suites** in
  **86.395 seconds**. CLI E2E **92/92**, RPC **21/21**, Discovery **58/58**.
- `cargo deny check`: **PASS**, exit 0 in **2.780 seconds**, advisories,
  bans, licenses and sources ok. The first fetch failed on RustSec TLS;
  one unmodified retry succeeded. Existing policy warnings (one missing
  license field covered by policy and six duplicate-package warnings) remain.
- `python scripts/verify_bootstrap.py`: **PASS**, 31 required files, fixed
  marker, key placeholders, links and contributor Skills.
- `python scripts/omp_inventory.py check`: **PASS**, inventory consistent.
- `git diff --check`: **PASS** on the isolated feature diff.

Receipt root: `C:/Temp/ara-skill-source-switches/`. Final `receipts/` contains
**34 files** across default, OpenCode-only, empty, total-off, name-filter and
three invalid-input cases. `receipts-audit.json` records artifact SHA256 and
raw-oracle checks; `backend-final.log`, `backend-final-result.json` and
`dependency-retry.log` preserve command output. The parent independently
checked the full Skill lists, all **16 requests**, the **11 journal receipts**
and their forwarded model content; source snapshot hashes match.

The first isolated full gate passed 1,133/0/1 in 109.256 seconds, but raw
default-list inspection found that a sibling temporary home/project without
`.git` let the existing ancestor search reach real user Skills. The final
fixture creates a project boundary and asserts the *entire* Skill list and
failure receipt's `Available` set. Production bytes did not change; the
entire final gate was rerun. The first receipts remain separately preserved
under `receipts-first/` and are not substituted for the final oracle.

Independent Codex reviewer `skill_sources_plan_review` applied
`ara-git-review` + `ara-rust-core-review`: pre-plan **APPROVE**, implementation
review **APPROVE**, 7 code/test/dependency paths reviewed, zero confirmed
defects, zero skipped; concurrent WIP explicitly excluded. Final independent
Windows point audit **APPROVE**: **11/11 staged paths**, **34/34 artifact hashes**,
**10/10 snapshot hashes**, all 16 raw requests and 11 call/receipt/forwarded
results, **five distinct Sessions**, three usage-error processes and raw gate
totals independently verified. No unresolved findings. Linux CI remains
pending at this checkpoint; the code checkpoint is labeled unfinished until
that platform check closes.

P0/V1, P1-P6, the full OMP marker and unrelated RPC/Skill provenance work retain
their previous acceptance boundaries.
