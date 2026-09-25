# Implementation evidence

Create one file per bounded point, named `<point-id>.md`. Commit the receipt with the tested code. Do not put credentials, private prompts, or account data here; retain safe reproducible inputs and artifact checks.

```text
Point / requirement / exclusions:
Upstream SHA and source/test location or ARA decision:
Delivered commit or exact worktree snapshot:
Rust entry and host chain exercised:
Environment and sanitized commands:
Expected vs actual normal result, exit code, artifact and state:
Expected vs actual failure/cancel/recovery result:
Streaming observation before completion, if applicable:
Real-model route, exact ID/protocol and bounded calls/turns/tokens/time, if applicable:
Observed output, tool effects, usage and latency (unknown where unavailable):
Independent review and semantic findings:
Unrun/blocked checks and impact:
Decision: tested / changes requested / accepted
```

Use [acceptance rules](../acceptance.md) and [audit procedure](../audit.md). A receipt describes executed evidence; filling a template does not itself pass a gate.
