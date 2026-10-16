# Connection loss and recovery boundaries

This standalone Host recovers transport observation separately from execution.
WebSocket reconnection does not authorize replay, create a new native Session,
renew a lease, or prove that a previously issued operation had no effect.

The Node sends a Ping after fifteen seconds, with a fifteen-second deadline for
its matching Pong. A wrong, stale or expired Pong cannot renew liveness. Every
transport write has a ten-second bound and observes shutdown; dialing has a
twenty-second bound and observes shutdown too. Disconnect cleanup invalidates
the outbound generation and waits for owned task cancellation before another
connection can take ownership. Cleanup failure remains fatal and fences
reconnection; it is not a reason to replay an unknown operation.

Reconnect delay grows from one second to thirty seconds with bounded jitter.
It resets only after an established connection lasted twenty seconds and
received a valid Pong. Time spent stuck in a TCP/proxy/TLS/WebSocket handshake
does not count as healthy uptime. `--once` returns a failed connection result
as failure instead of silently reporting a successful process exit.

The maintained Hub uses nhooyr.io/websocket v1.8.17. Its Ping deadline after a
successful write returns an error without closing the concurrent Read. The
heartbeat error path explicitly closes that same connection to release Read
and invoke normal cleanup, preserving the existing approval ownership.

Foreground interruption is not a native completion receipt. The web host must
mark uncertain observations accordingly, preserve human drafts under their
original controller/device/Session scope, and stop dependent tool continuations.
Only explicit existing Goal authority can permit background work. Pending queue
texts retained for manual draft recovery do not become a durable execution queue.

The [recovery evidence](../evidence/recovery-0.2.3-20261016.md) records exact
artifacts, executable faults, browser observations and outstanding boundaries.
