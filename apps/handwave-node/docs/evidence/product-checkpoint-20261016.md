# HandWave product checkpoint — 2026-10-16

Decision: the compact Host has enough breadth for daily supervised coding and
system-troubleshooting use. Treat this as a usable trial release, with explicit
reliability and capability boundaries. It is not acceptance of the root Rust
Core/OMP migration or an unrestricted autonomous desktop agent.

## Available behavior and boundaries

| Area | Available | Remaining boundary |
|---|---|---|
| Distribution | Windows standalone static-CRT EXE; Linux source installer; model credentials remain in the cloud | Current Windows0.2.3 EXE is4,961,792bytes. Current source publication does not prove every Linux distribution/ARM/musl/macOS target. |
| Coding and troubleshooting | Filename/content search, bounded reads/log output, edits, commands, explicit Python/PowerShell/cmd/sh scripts, downloads and process cancellation | Uses installed interpreters and OS-user permissions. Arbitrary third-party installers and administrator operations are not universally validated. |
| Context | Source-preserving segmented history, bounded working view, summaries, original reference retrieval and context telemetry | Compaction is lossy as a working view; originals and receipts remain authoritative. Byte estimates are not exact tokenizer measurements or unlimited context. |
| Knowledge and memory | Project knowledge, explicit device/project/workspace cloud records, versioned references, adopt/edit/revoke, bounded recall | Shared workspace authentication is not separate personal identity. Vector retrieval, offline bidirectional sync and automatic consolidation remain deferred. |
| Tasks and Goals | Run checklist, read-only Plan, durable Goal steps/budgets/review, explicitly leased Jobs, bounded appointments and message queue | A completed Run/Goal proposal still requires review. This is not a global cross-session project-task database or an unlimited unattended scheduler. |
| Recovery | Node/Hub/browser heartbeat and bounded retry, scoped drafts, explicit interrupted/unknown results, no automatic effect replay | Controlled software faults and selected production restarts have separate evidence. Power loss and all physical network conditions are not established. |
| Efficiency | Bounded tool output, relevant reference expansion, summaries, stable instruction prefix and provider-reported usage/cache fields | No comparative task benchmark, billing reconciliation or percentage savings claim. Auto remains random among current eligible models. |
| Experience improvement | Tool-evidence-backed experience candidates, explicit review/adoption and subsequent recall | No automatic code, permission, Skill or model-weight modification. Unreviewed success claims must not become durable policy. |

The released evidence is indexed in [the knowledge index](../knowledge/INDEX.md),
including [0.2.0](version-0.2.0-20261016.md),
[queue0.2.2](message-queue-0.2.2-20261016.md) and
[recovery0.2.3](recovery-0.2.3-20261016.md). Historical tests retain their actual
image/version; passing a newer targeted regression is not full revalidation of
every historical feature on every target.

## Next investment

Keep the current interface and feature scope stable while collecting real task
cases. Prioritize incorrect state, stalled execution, wrong search results,
unhelpful failure recovery and faulty memory recall before new controls.

Use a small representative corpus: fix a Python script, inspect a system log,
find a file and its caller, install an isolated environment, and resume a bounded
Goal. Record verified artifact/result, intervention count, elapsed time,
reported tokens/cache and failure/recovery behavior. Compare against the same
task and model before claiming an efficiency improvement.

Then improve retrieval and experience reuse from reviewed cases. Desktop
Computer Use (screenshots, UIA/accessibility, clicking and verification) remains
a separate capability point; declared model multimodality/1M capacity alone
does not implement image transport or desktop interaction.
