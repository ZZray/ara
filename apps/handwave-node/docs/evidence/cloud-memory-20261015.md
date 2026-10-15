# Cloud memory M1 and contextual composer delivery

2026-10-15. Independent HandWave Host slice in dirty working trees; this does
not accept root ARA/OMP parity. No broad staging, commit, reset or history rewrite.

## Delivered scope

Native placeholder hints use local intent rules. Tab fills only an empty,
focused, idle composer; it never sends. Escape permits normal Tab navigation.
Draft, IME, modifier, busy and late-history gates have deterministic coverage.

Ray owns explicit text records, hashes, revisions, CAS, idempotent operation IDs
and revocation tombstones. Default scope is the real bound device; project joins
are explicit. Common Ray login means Ray-workspace sharing, not personal identity.
New APIs require a browser session, same-origin writes and a Hub-verified binding;
arbitrary machine Bearers cannot bypass this boundary.

New supported Runs receive at most 32 records/32 KiB serialized snapshot JSON.
Node verifies schema/device/hash, keeps the body ephemeral, and invalidates
cloud-derived working context on changed/missing/unavailable snapshots. Local
originals and raw audit remain. Queued follow strips an inherited cloud snapshot.

Explicit workspace/session keys map to separate native histories. Empty fields
keep the old path. Route mismatches still reject, without reset or silent fallback.
Outbound controls and inbound Run text/terminals keep captured ownership; switching
to B cannot persist late A output into B. Detached output remains in the node
audit; automatic writeback into the original cloud conversation is not claimed.
Explicit follow preserves its supplied ID, rejects another session key and
does not execute duplicate queued IDs twice.

## Final artifacts

| Artifact | Bytes | SHA256 |
|---|---:|---|
| Windows x64 static-CRT EXE | 4,518,912 | `40669969a4b84074674f767b38180901293d9adbeebb8263347ed3c3d4729f73` |
| Windows ZIP | 3,430,019 | `89aa1c0d27ebac6b147bbddab6c77f54c8d136759333d10f22633db0b423bcee` |
| Source archive, 580 files | 279,197 | `3ffb114f7b2e788c4f23a6658b7251861a6e381f79ee63c187d392ee4d5b96a9` |
| Linux x64 GNU source build | 5,289,672 | `70c8f2fcf3b966a0c1f7dbe319cabf70ad7170741d90651c37554a6d9d2453ab` |
| Ray Linux binary, retaining previous immutable releases | 17,277,090 | `d54e6d1dce9af4269517aa7e1e7e85f991ca6209369d48d27fd85b50e0073b02` |

The source archive contains preproduction design status; its Rust bytes match
the final code. These are the postdeployment receipts. ZIP has no real node.conf,
identity or state. Previously published b906/6874 source and c761 ZIP URLs remain
immutable. The short `/ray/install.sh` pins this new source and remains no-store.
The installer uses a fresh target; it does not configure or connect a new device.

The published tar is a Linux runtime-source subset, not the complete Git tree.
It omits evidence pages, Windows packaging and license-collection entry points;
some packaged documentation links therefore require the full Git checkout.
The full checkout includes them. The immutable tar has not been overwritten.

## Verification layers

- Final source: fmt, strict all-target/all-feature clippy, **39 unit tests** pass.
  These use the independent app manifest, not root parity. Cargo deny passed all
  four categories before the session-only delta, with unchanged lock and policy.
- Final EXE + isolated actual Hub + controlled provider: **native4** verifies
  two-session isolation, continuation, old-path compatibility, bad-key refusal,
  route fence, explicit follow ID/deduplication and wrong-key refusal. **Cloud5**
  verifies actual wire context, ephemeral body, invalidation/restart/failure,
  wrong-device/hash refusal and no queued revival.
- Earlier c761 context15 and adoption-cancel3 receipts remain separate; they are
  not relabeled as final-EXE runs.
- Final Ray: suggestion21, memory-UI20, transport owner gates and original
  upstream event/persistence functions pass in VM. Late A delta/end/out neither
  POST into B nor clear a newer B run. Go tests/vet/Linux build pass. Earlier
  memorystore race tests pass. VM is not a real browser/network acceptance.
- Packaged Linux source: shell syntax, installer fixtures and GNU x64 build pass
  in WSL Rust1.97.1, 61.894 seconds. Source hashes stay unchanged and both new
  behavior markers exist in the binary. Dependency target is reused but extracted
  Rust mtimes are explicitly advanced to force this package to recompile.
  The earlier normalized-mtime/cache result stays INVALID_STALE_CACHE, not pass.
  ARM, musl and other distributions are not runtime-accepted.

## Production and actual browser/model observations

Pinned binary-only Ray replacement passes, PID62689 at deployment, backup
`/opt/ray-front/backups/cloud-memory-20261015T043615Z`. Configuration/auth/TLS
hashes and other backend PIDs stay unchanged. Public artifact hashes, HTTPS login,
machine health and anonymous memory denial pass. RAYWORK PID75684 at upgrade
uses the final image, same configuration/identity/state and existing user grant;
backup `C:/Users/loveu/AppData/Local/HandWaveNode/backups/session-memory-20261015T043557Z`.

1. A manually saved synthetic device fact survives Ray restart and a new Web
   conversation. Run **RKQ4PVBTPQ** is asked for its value without giving the value,
   and returns exactly `RAY-MEMORY-3a8f2c716d`, with no tools in the native history.
2. Explicit revoke hides the active record; another new conversation, Run
   **LWWHRZIOMY**, returns `UNKNOWN`. Its different native path contains no marker.
   **It additionally calls reference_read once despite the no-tools instruction.**
   Memory revocation passes; that no-tools instruction does not. No effectful
   tool is called. This distinction is retained rather than weakening acceptance.
3. Pre-fix I7W3O7W2AQ route-fence failure remains a failure. Later success uses
   explicit session mapping, without clearing history or relaxing the fence.
4. Native route records `auto`, `openai-completions`; it does not identify the
   downstream model. No exact model ID, usage or measured speedup is inferred.

Actual UI: initial/focused hints, Tab fill without send, Escape then Tab moves
focus; light/dark; 1513/375px no horizontal overflow; 343px native dialog at left16
on narrow screen; binding update reloads card. Original draft `以及`, light theme
and normal viewport restored. No second real device or independent browser
profile is claimed; scope isolation/project sharing have controlled tests.

## Review and receipts

Independent Codex cloud_memory_arch_review reviews plan, Ray authority, transport,
the reproduced original-function cross-session defect and final artifacts.
composer_suggestions independently reviews store/Rust invalidation/cancellation.
Final independent audit verifies 39 Ray source/resource hashes, 13 packaged Rust
source/Cargo files, EXE-in-ZIP and running image, and final native/cloud receipts.
Parent verifies actual browser outcomes. No Claude/Bridge reviewer is used.

Detailed pins/logs/screenshots:
`D:/WorkSpace/tmp/handwave-cloud-memory-20261015/`:
`build-result.json`, `final-artifacts.json`, `final-deploy.json`,
`local-node-final.json`, `linux-final.json`, `final-native-release/result.json`,
`final-cloud-release/result.json`, `live-native-receipt.json`, `live-recall.png`,
`live-revoked-wide.png`. Final source/unit receipt:
`D:/WorkSpace/tmp/handwave-follow-owner-20261015/result.json`.

Portable, field-selected public receipts are committed in
[receipts/cloud-memory-20261015](receipts/cloud-memory-20261015/): artifact hashes,
Linux build, final Release native/cloud fixtures, Rust checks, deployment
booleans and the live recall/revocation distinction. Raw checkpoints, requests,
private identities, credentials and full operational logs are excluded.

Changed-owner documentation validation passes six pages/30 local links and
scoped diff whitespace checks. Root verify_bootstrap.py did not complete its
global recursive scan in 107 seconds and was stopped; it is not marked passed.
See bootstrap-validation.json. No unrelated files were fixed.

M1 delivered within these bounds. Personal principals, M2 outbox/bidirectional
sync, M3 automatic consolidation, M4 embedding/graph retrieval and cloud read
tools remain unimplemented. No self-learning/benchmark or unlimited journal claim.
