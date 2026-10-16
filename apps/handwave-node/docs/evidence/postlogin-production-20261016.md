# 0.2.0 authenticated production Goal/Job acceptance

Date:2026-10-16, Asia/Taipei. This is a new user-authorized production trial,
separate from the retained [release-time failures and gaps](version-0.2.0-20261016.md).
No product code, binaries, services, provider settings or old device identity
were changed. Unrelated Rust Core and legacy Go/auth/browser WIP remain excluded.

## Authorization and actual surface

The user explicitly renewed RAYWORK/U75VP37OFSBBK for24hours and reported normal
Ray TOTP login completed. The existing in-app browser displayed the authenticated
HandWave workspace, controller BJENA3VEQEUQG. The real device panel's Allow button
was used; its pinned stable frontend sends ttl86400. Read-only cloud approval
inspection confirmed that exact controller/device and expiry
**2026-10-17 08:45:04 +08:00**. No root-token browser login, TOTP bypass, config
rewrite or direct authorization-file edit was used.

Page device details showed rust-0.2.0 and connected RAYWORK. Installed PID90172
still runs the standalone EXE SHA256
`cb676ce827a0a496a23decd05246d0f78d5e24e0f6b5c8f569dc1e7eb5e304b1`.
Ray/Hub publication artifacts remain those in the release record.

## Single bounded production task

Created one fresh web/native Session and fixture directory
`D:/WorkSpace/tmp/handwave-production-020-20261016/work` with input.json `[6,12,24]`.
The normal composer selected Goal, explicit deepseek-v4.1-flash /
openai-completions, background continuation and managed-process permission.
Gateway Auto/random configuration was not changed. This trial uses a fixed
approved Python script, so it does not measure arbitrary code-generation quality.

The visible Host policy is1hour/48calls/8slices, clipped to the device grant;
Job timeout30seconds. The task additionally requested one Job and termination
within3minutes. Actual usage is **4 model calls,2 settled slices,1 Job**.

The script independently reads input.json, sleeps2seconds, writes report.json
and prints `LIVE_GOAL_REPORT sum=42 count=3`. Effects are restricted by task
instructions to this fixture; those instructions are not an OS sandbox. The
coordinator inspected actual outputs and sources after the run.

Production task receipts:

- Goal GOTR2T6FAE, native revision5/criteria revision1,3/3 steps done.
- Final native state **awaiting_review**, no pending Run; last settled Run
  `aacec3fe9d051842e6e28e48`. It is not automatically accepted.
- One job_start, op_id prod020-report-20261016, Job
  `5e6ace116ad641a2d22d36a50bfbe2a6c58e1651d3a2e58ee27eed2c473875ad`.
- Worker receipt succeeded, exit_code0, effects_unknown=false, stdout33bytes,
  stderr0bytes. The actual source.py and owner/spec hashes were checked.
- Actual job_read_log and read_file tool receipts match stdout and report.json.
  Report is `{"sum":42,"count":3}`. Stdout SHA256
  `a0b3a0f3559497bdcdd9568fb65cae7c1fa8c79586877f18824fc2aee4d10187`.
-11 original messages retain a validated journal hash chain/head checksum,
  checkpoint suffix and two settled Run indexes. Four assistant responses agree
  with root calls_reserved4 and slices_started2. No job_status model polling.

The real UI showed live delegation, collapsed public process, actual result,
Goal **待你验收 · 3/3**, persisted completion criteria/version1 and per-step
evidence. Confirm Complete was deliberately left to the user. This tests that
the Agent proposes completion without silently accepting the Goal.

## Preservation and independent review

The original7 native Sessions and configuration/identity are byte-identical to
the actual pre-upgrade backup. This is an endpoint comparison with that earlier
backup, not a fresh pre-task file trace:16/17 pre-existing durable files match.
The one changed shared receipt file preserves all12 old records unchanged and
only appends the two Runs from this trial. No old transcript was replaced.

Independent Codex `/root/context_modes_audit` reviewed native state/journal,
actual worker source/receipts/logs, report, budget/identity, UI DOM/screenshot
and original-state preservation. See the [structured post-login audit](postlogin-production-20261016-review.json).
The coordinator also inspected these primary receipts independently of the
model's natural-language report.

Private evidence is in `D:/WorkSpace/tmp/handwave-production-020-20261016/`:
native-private.json/journal, job-private, goal-final.dom.txt,
goal-awaiting-review.jpg, original-files-endpoint-comparison.json,
global-receipts-append-check.json and installed-exe-sha256.txt. Raw private
transcripts/state/worker nonce/configuration are not committed.

## Closed gaps and limits

This trial closes the release-time **pending device authorization**, **final
standalone actual Goal/Job task**, and **authenticated browser Goal workflow**
gaps for this task/version. Earlier failed fixtures remain failed historical
evidence; none is rewritten as PASS. The accepted result is not inferred from
the old dynamic-CRT candidate.

It does not certify every browser/responsive layout, one-hour page-close
continuation, full1M stress, cache savings/billing, arbitrary installers or
autonomous code self-modification. Final screenshot tidying encountered another
browser-automation timeout after the successful task/DOM capture; no task was
replayed and native state remained settled. The saved evidence predates that
automation-only timeout. No extra model retries or production restart followed.
