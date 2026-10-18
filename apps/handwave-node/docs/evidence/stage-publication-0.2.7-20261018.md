# HandWave Node 0.2.7 stage publication

2026-10-18. The user explicitly authorized a stage commit, push and deployment.
This publishes Web research, reusable skills and indexed environment/project
knowledge for supervised use. The autonomous actual-model two-Session gate in
[the implementation evidence](environment-library-0.2.7-20261018.md) remains
**OPEN**. The four failed bounded model trials were not rerun or reclassified.
This stage publication is not full self-evolution acceptance or shared OMP parity.

## Exact sources and public packages

- Rust implementation: `5b6f78adbb34e89e0cd712c623fd4c08a8185d06`, pushed to
  `ZZray/ara` `origin/dev` and verified with the remote ref.
- Ray publication: `27349eff3ce33bf4de2312a654b2701a0c683756`, pushed to its
  existing `origin/dev/rust` and verified with the remote ref. Only the two
  source-install files and two new immutable package files were committed there.
- Windows and source packages were made from the exact Rust commit export.
  The final Ray binary was rebuilt from the exact Go commit export with the
  existing verified stable Web archive. Its 788 Go/module inputs matched the
  tested candidate; the resulting image SHA was identical. All 37 existing Go
  WIP file bytes were preserved. No unrelated root Rust/OMP WIP was staged.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Windows standalone EXE | 5,349,888 | `dba28da40a66c45a7025473876f2215c75598cdfc4ceb2246e781ce35d468535` |
| [Windows ZIP](https://118.25.103.226/ray/releases/handwave-node-windows-dba28da4.zip) | 3,740,823 | `2ef024d0c817ec6ff5a3fa556c3fc48ea10d28532a2724ac64c081ee2190428b` |
| [Source archive](https://118.25.103.226/ray/releases/handwave-node-source-324d4cbcbb6e66ad.tar.gz) | 578,953 | `324d4cbcbb6e66add330345b4a9844b27ef66319a1f58230d4fc940328b3e162` |
| Running Linux Ray | 55,906,466 | `59800122bdab0c1de055db7116d1de453c88f6ee286c7480edbd1e606902ff33` |

`/ray/install.sh` now pins this source URL/SHA. The underlying immutable installer
is unchanged (`fe4b10bee6a73659f6c5db78e624aa794bec0e0b57f91619d8be13c4f694094c`).
All 22 prior public artifacts remain unchanged; all 24 new/old artifact URLs
were fetched and hash-checked. A separate Windows-client public fetch checked
both new package hashes and default source pin with normal TLS verification.
Packages exclude live node.conf, enrollment credentials, identity and state.

## Checks on the published inputs

Evidence root: `D:/WorkSpace/tmp/handwave-deploy-027-20261018/`.
Versioned [publication receipt](receipts/stage-publication-20261018/publication.json)
and [independent Codex review](receipts/stage-publication-20261018/review.md)
retain the stage-only approval and the unaccepted autonomous-task boundary.

- Windows exact-export release build passed. The embedded Web prompt's trailing
  blank line was normalized before commit, so this is a newly pinned image,
  distinct from the earlier local `81ec...` candidate. It passed 201 tests with
  zero failures and one explicit ignored public-network trial, environment Host
  5/5 and existing Host regression 27/27 against the actual final EXE.
- Publication preflight found the old source packager omitted four required
  include_str Markdown payloads. The allowlist was fixed before publication.
  Independent inspection matched all 653 archive members to the exact export.
- GNU Linux WSL, existing ara-verify uid1000, compiled the actual new source
  archive offline in a fresh source directory and dedicated target. Knowledge
  14/14 and skills 12/12 passed. Actual Linux Node + signed Hub + controlled
  model Host 5/5 passed (32 requests, no fixture errors), including create,
  install, restart/new-Session reuse, retirement and Plan refusal.
- That Linux verification image is **debug**, 184,779,064 bytes, SHA
  `af8b407a38aaccfef6cba4de5e8276653c49bc442247819cdc11fae9c3fbc994`.
  It is not offered as a small Linux release binary. Full release/LTO,
  ARM64/musl/macOS and other distributions were not accepted by this trial.
- Ray package tests, vet, Linux stripped build, avatar DOM and complete-script
  parsing passed. Final JS bytes match the prior published brand build;
  unrelated authentication/readability UI WIP was excluded.

## Deployed state and rollback

Only Ray was restarted in the cloud. Final PID1142887 runs exactly the published
`59800122...` image. The Hub, OMP ingress/Gateway, RainFlow and Arena process
identities remained unchanged; all 59 enumerated authentication/configuration/
history/lease files remained byte-identical. Login/asset gates and old package
URLs remain protected or public as before. No browser visual or authenticated
real-task acceptance is claimed for this publication.

The installed Windows Node was originally stopped, with zero pending Run or
active Job and only canceled Goals. Its identity/configuration and all 107 state
file hashes were preserved at the binary switch and independently checked after
startup. PID26368 runs the exact `dba28...` EXE, fingerprint `U75VP37OFSBBK`,
and connects through verified public WSS while awaiting controller approval.
The existing launcher limits (12 calls / 120 seconds) were preserved. No device
approval or expiry was renewed, and no old task was replayed. Fresh packages
retain the product's own defaults; this machine's explicit launch limits are
not a claim about all installations.

Binary-only backups and raw private receipts stay outside Git. Cloud backup:
`/opt/ray-front/deploy-node-027-20261018/backup-1792331446949495257/ray-front`.
Local old image is in the ACL-protected
`%LOCALAPPDATA%/HandWaveNode/backup-stage-027-20261018/`.
Rollback must first compare the current image to this publication's SHA and
restore only the intended old binary/default package pin. Do not replace whole
user state, new knowledge or authorization tables from backups.

## Retained publication failures

The first local identity guard compared complete versioned banners, mistaking
the intended 0.2.5-to-0.2.7 version change for a fingerprint change. Before doing
anything further, both exact image hashes, the actual unchanged fingerprint and
all original state hashes were checked; only the unfinished launch continued.
The binary swap and unknown tool effects were not replayed.

The first Ray switch failed its immediate process-image check and automatically
rolled back to the exact old image. State/other services were independently
confirmed unchanged before another fresh preflight. The revised helper uses a
bounded readiness wait. Its actual probes observed the same PID first running
systemd-executor (`95c258398b954fd34f9dec8028c8051d3904806a32db1c7d7ddc8176704a826b`)
and then the exact new Ray image. The former hash independently matches the
host's `/usr/lib/systemd/systemd-executor`. This proves that a Type=simple start
can precede target exec in this deployment; the first failure did not record its
intermediate hash, so its exact intermediate image is not directly established.
The successful retry and original rollback receipt are both retained.

Linux verification retained a timed-out dependency-cache copy and a DrvFS utime
failure while exporting evidence. Neither was hidden as a product pass: the
build ran from the partially copied dedicated target, and the real Host used
the original ext4 binary after checking export byte equality. No product
assertion or test was disabled. Default DDG search remains network-dependent;
general autonomous consolidation, cost savings and the earlier real-model
two-Session gate remain unaccepted.
