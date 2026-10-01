# OpenAI daily use

This Host slice supports custom OpenAI Chat Completions/Responses and OpenAI
account login with Codex Responses over SSE. It is a bounded daily-use path;
complete OMP Provider parity remains in [the provider plan](provider-plan.md).

## Custom endpoint

Save JSON or YAML at `$ARA_HOME/agent/models.yml`. Without `ARA_HOME`, the
default is `$HOME/.ara/agent/models.yml`, falling back to
`$USERPROFILE/.ara/agent/models.yml` on Windows. `--models-config PATH` selects
an explicit file; a missing or invalid explicit file fails before model work.

Example JSON (valid in `models.yml`):

```json
{
  "providers": {
    "custom": {
      "api": "openai-completions",
      "baseUrl": "https://example.com/v1",
      "apiKey": "MY_MODEL_API_KEY",
      "models": [{"id": "MODEL_ID", "input": ["text"], "maxTokens": 8192}]
    }
  }
}
```

Set `MY_MODEL_API_KEY` privately in the process environment. Replace the endpoint
and `MODEL_ID` with values supported by that service, then run:

```powershell
.\target\debug\ara.exe --provider custom --model MODEL_ID --repl
```

Use `"api": "openai-responses"` for a service implementing Responses. CLI
options override selected configuration. Unsupported execution fields fail
explicitly rather than being silently discarded. The daily authentication
subset supports private environment/config keys and explicitly keyless routes;
it does not yet implement the complete OMP AuthStorage precedence or commands.
Custom configuration can coexist with the legacy Anthropic/proxy flag route.

## OpenAI account

```powershell
.\target\debug\ara.exe login
.\target\debug\ara.exe --provider openai-codex --model MODEL_ID --repl
.\target\debug\ara.exe logout
```

`login` prints the official device authorization URL and a short user code.
Complete authorization in your browser, then wait for the CLI confirmation.
Use a model actually available to that account. Login/logout operate even when
model configuration is invalid. A custom-provider-only configuration file does
not prevent selecting `openai-codex`.

The Host keeps credentials in `$ARA_HOME/agent/auth.db`, using the same default
home rules above. Keep the home private; Windows SQLite files inherit its ACL.
Refresh occurs per request when required, and logout disables stored accounts.
An interrupted refresh with unknown outcome requires login again; the Host does
not automatically resend the old grant. Normal exit waits for settlement;
hard termination/power loss or a completely unwritable database remain limits.

Print/JSON and REPL support tools, original Session resume, `/new` and the local
summary `/compact` path. Codex `/new` rebinds request identity to the new Host
Session. Summary output is checked against a local adoption budget; the endpoint
does not receive an output-token cap, so this is not a server cost limit.
Codex RPC, browser callback login, WebSocket/Lite/native provider compaction and
full multi-account selection are recorded later work. Actual OpenAI account
authorization and a subscription-model task require separate live evidence.

## Module verification

```powershell
python -X utf8 scripts/verify_openai_daily.py --module all --full
```

Use `--module config|auth|sse|cli` for development. The four suites cover whole
flows; `--full` runs the shared backend, inventory, dependency and build gates.
The deterministic runner uses synthetic credentials and never reads local
account tokens. Selected live tasks use separately bounded, private credentials.
See [delivery evidence](evidence/openai-daily.md).
