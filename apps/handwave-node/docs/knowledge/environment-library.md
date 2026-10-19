# Environment knowledge and reusable skills

Scope: compact Rust Node libraries introduced in 0.2.7, with the small 0.2.8
follow-up published on 2026-10-19. This is Host-owned
environment adaptation, separate from shared ARA Core/OMP parity and from cloud
or private conversation memory. Source contracts are in
`src/environment_library.rs`, `src/skill_library.rs`, `src/environment_tools.rs`,
`src/experience.rs`, `src/agent.rs` and `src/execution.rs`. Implementation is not
production acceptance; the final artifact's evidence must establish that boundary.

## Index first, content on demand

Knowledge uses an `INDEX.md` directory plus focused Markdown content pages.
At each Run, the Host supplies bounded index text and skill metadata; it does not
load every page or script into the model. The model uses `environment_catalog`,
`knowledge_search` and `knowledge_read` when the task needs a specific fact.
Both knowledge and skill guidance are untrusted sourced data, not new authority.

The project root is the nearest ancestor of `--cwd` containing `.git` (directory
or worktree marker file); without one it is `--cwd`. Defaults are:

| Data | Location |
| --- | --- |
| Environment technical knowledge | `<state-dir>/library/knowledge/INDEX.md` |
| Project technical knowledge | `<project-root>/docs/knowledge/INDEX.md` |
| Environment skills | `<state-dir>/library/skills/<name>/SKILL.md` |
| Project skills | `<project-root>/.handwave/skills/<name>/SKILL.md` |
| Suggested tool storage | `<state-dir>/library/tools/` (not automatically installed) |

`--knowledge-dir`, environment variable `HW_KNOWLEDGE_DIR`, or `knowledge_dir=`
in `node.conf` overrides only environment knowledge. This can reuse a maintained
local knowledge directory; it does not move project knowledge or skills. The
default state directory is documented in the README; it belongs to the actual
runtime account and device, not the developer's Codex installation.

Admitted non-Plan Runs attempt to initialize the two missing indexes, preserving
existing indexes and pages. Plan reads available directories without creating
them. Read-only or malformed roots produce warnings/tool errors; they do not
disable ordinary tasks. Native conversation checkpoints have their own storage
and may still be created in Plan mode.

## Sourced technical notes and retirement

`knowledge_write` creates an immutable `notes/<id>-<sha256>.md` revision and
updates only its managed segment inside `INDEX.md`. Human text outside that
segment is preserved. Existing ordinary root-relative Markdown index links are
searchable without importing or rewriting their bodies.

Writes carry current Run provenance and an explicit status:

- `draft`: no native evidence selected. A proposal to check, not an established fact.
- `observed`: selected successful native tool receipts were resolved by the Host
  in the current Run. Their authenticity does not automatically prove every claim
  in the note or accept the Task.
- `revoked`: retired knowledge; source files remain for audit.
- `unmanaged_unverified`: an existing human Markdown page outside managed notes.

`library_evidence` lists authentic successful current-Run receipt IDs and hashes
without copying output bodies. The model selects relevant IDs; supplied IDs are
validated against the native call/result pairs. Notes must state the environment,
conditions, observations, limits and sources rather than copying private chats,
credentials, cloud memory or a model's narrative as technical fact.

Updates require the current `expected_hash`. Revoked IDs cannot be reactivated,
even with the former content hash; a new observation needs a new ID. Repeated
revocation of the same hash is idempotent. Read/search reject revoked, superseded,
unpublished or hash-mismatched managed revisions as reusable knowledge. The
separate file tools can still inspect preserved audit artifacts explicitly.

Each knowledge root has its own nonblocking file lock for cooperating Node
writers. Content is published before an atomic index replacement. A failed index
publish retains the old index; an unreferenced immutable revision may remain,
and the error reports that boundary. Locks do not coordinate arbitrary editors;
the tool checks for external index changes before replacement.

## Skill carriers and environment adaptation

Embedded read-only `skill-creator` and `skill-installer` explain how to discover
missing tools, acquire sources, construct a focused bundle and verify its use.
They are guides for the Node's model, not dependencies on Codex's private skills.
The registered tools are `skill_read`, `skill_create`, `skill_install`,
`skill_verify` and `skill_remove`.

The small `SKILL.md` frontmatter supplies name and description for discovery.
Instructions are loaded only when selected. Resources such as `scripts/`,
`references/` or templates are read progressively, with bundle hash and actual
local file path returned. Skill creation or local-directory installation copies
files and licenses but never executes their contents. Existing names are not
overwritten. Remote archives must first be acquired and inspected with the
separately authorized web/download/script tools; no remote package hook runs
implicitly in the installer.

Successful create/install receipts return the final `bundle_dir`, after atomic
publication. Use it directly instead of scanning for the new bundle; resource
reads additionally return `file_path`. A returned path does not imply execution.

New or changed bundles are `unverified`. `skill_verify` requires the exact current
bundle hash and authentic successful current-Run native execution receipts. The
result `execution_evidence_attached` means those receipts are associated with that
hash. It does not prove by itself that the commands exercised the bundle. Actual
verification must inspect the invoked resource path, outputs and expected effects.
Changed resource bytes lose that association. `skill_remove` moves an owned
bundle into hidden `.retired/`, removes it from discovery and preserves its files;
it does not uninstall external software or remove unknown/manual directories.

## Permissions, bounds and evidence

Plan advertises only read-only discovery/read/search/evidence tools. Creation,
installation, verification, retirement, knowledge writes and scripts require
Execute/Goal permissions already held by the Run; skill text grants none. All
execution retains the runtime OS account's privileges. Allowlisting, link checks
and directory locks are not an OS sandbox against a hostile concurrent user.

Knowledge operations bound arguments, files, search coverage and JSON output;
partial results are explicit. Paths stay within the selected `project` or `global`
root, with relative Markdown paths, regular-file checks, reparse/symlink and
hardlink rejection. Skills have separate file/count/aggregate discovery budgets.
Exact active grant strings are redacted. These controls do not authorize storing
other secrets or indiscriminate transcript copies.

Regression sources: `src/environment_library_tests.rs`,
`src/skill_library_tests.rs` and `tests/verify_environment_library.py`. The latter
uses a real isolated Node and signed Go Hub with a deterministic provider, actual
script invocation, native receipt selection, process restart/new-session reuse,
license/resource installation, retirement and read-only Plan checks. It proves
Host effects when executed successfully, not autonomous model intelligence or
production deployment. The first candidate exposed the Windows canonical-prefix
issue recorded in [engineering cases](engineering-cases.md); corrected final
artifact acceptance must be recorded independently.

For this 0.2.7 candidate, local source review, 201 unit tests and 32 actual-process
Host cases passed. Selected autonomous actual-model trials exhausted their
explicit validation budgets before final settlement; two-Session model acceptance
remains open. See the [delivery evidence](../evidence/environment-library-0.2.7-20261018.md).
Do not infer general autonomous consolidation or production deployment from the
implemented library tools or their controlled Host checks.

## Current-Run efficiency observation

The 2026-10-19 follow-up candidate supplies a small advisory budget to ordinary
model requests: reservations already consumed, remaining calls including the
upcoming call, and sampled monotonic time remaining. Summary calls use the same
counter. `working`, `closing`, `last_call` and `exhausted` describe local allowance;
Goal/grant/cancellation gates may reject earlier. The static prompt asks the
model to reuse current directories and successful probes, then prioritize
verification and an honest final status as time/calls run low.

This observation is appended only to the provider Context after compaction and
before call reservation. It never enters native history, memory or summary input.
If the complete request cannot fit it, the exact original Context is retained.
Appending preserves the preceding message prefix for this new carrier only;
neither cache hits, lower costs nor better task completion are guaranteed.
The [bounded candidate evidence](../evidence/run-efficiency-20261019.md) records
actual Host checks and the separate open new-Session autonomous-reuse gate.
The later [0.2.8 publication](../evidence/stage-publication-0.2.8-20261019.md)
records exact committed packages and deployed images separately from that gate.
