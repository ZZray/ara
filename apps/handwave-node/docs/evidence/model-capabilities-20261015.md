# Model declarations and dynamic context — 2026-10-15

The existing Hub directory is the declaration owner, not a model-name heuristic.
CAS and B.AI already declare deepseek-v4.1-flash as1M/text+image. Gateway editor
add/edit now exposes context, output, text/image and manual/follow-upstream
ownership. That separate Gateway binary is deployed; no Hub/Ray or installed
Node replacement belongs to this point.

Delivered Windows EXE4,650,496 bytes, SHA256
`f893522ad241c6622102c45024e3ba2f0e522f1a83dc112fbac272b887eed6cd`.
Workspace fmt/clippy(all targets/features,locked,deny warnings) and tests pass:
Windows86, Linux84 (offline compile, actual ara-verify test image, source hashes
match). No dependency/lockfile changed; dependency audit not rerun.

Actual binary+Hub+controlled upstream `verify_model_caps.py` passes five groups:
actual326604-byte request exceeds old192KiB without summary;1M+text/image event
but text-only Host;4096 fails before provider without pending fence and restored
1M continues same Session;32768 shrink preserves audit/route; fresh small context
succeeds; malformed capacity rejects before provider. Existing `verify_context`
15, `verify_modes`5 and `verify_evolution`5 groups pass on this image. The context
fixture explicitly uses unknown capacity to retain the legacy192KiB regression;
the new capability fixture tests known capacities through real HTTP separately.

Fresh public HTTPS/private SSH real DeepSeek trial passes Plan3/Goal6 call
reservations in34.483s, actual Python/report/readback/goal progress, same Session
across restart, explicit no-model goal acceptance and no memory self-adoption.
Temporary device approval revocation is confirmed. Actual model_capabilities
receipts for both Runs declare context1000000/max_tokens16384/input[text,image],
with projection922176/fullrequest987712 byte allocations and host_input[text].
Supplier usage and actual HTTP counts remain unknown; no speedup percentage,
1M-token capacity trial or image transport acceptance is claimed.

Independent Codex model_metadata_editor POST found capacity validation occurred
after pending_run save and could strand a Session. Validation/receipt admission
was moved before the fence;4096→1M real Host regression verifies recovery.
Full Context gates account for system, schemas and summary JSON re-escaping;
summary rejected before HTTP does not reserve another model call. Existing
256-message/16MiB history,4MiB sidecar and source limits remain.

Raw output/artifact/source manifests under
`D:/WorkSpace/tmp/model-capabilities-20261015/` are kept outside Git. New package
does not update installed RAYWORK or current public download URLs. Gateway
editor is verified in the actual authenticated browser; its production config
and other service processes were preserved. The earlier unknown-Hub-source
blocker remains a separate deployment boundary.

Changed/index relative links pass. The full root bootstrap scan exceeded a
30-second bound and is NOT COMPLETED. Final independent Codex readback verified
actual request sizes, capability events, native goal/pending state, image hashes
and deployment pins and accepted only the declared bounded scope.

## Subsequent installed and public activation

The f8935 Node and its public Windows/source packages were subsequently
[activated and verified through the real Ray browser](production-activation-20261015.md).
The earlier statements about unchanged installation/public URLs describe that
point's checkpoint.1M-token stress and image transport remain unaccepted.
