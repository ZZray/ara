# Problem-solving tools and compact execution

2026-10-15. This independent Node uses the same pinned rpi revision; it does not
change the root Shared Core OMP ledger. Runtime evidence and exact image identity
are in [the delivery record](../evidence/problem-tools-20261015.md).

## Tool selection and evidence

Use `find_files` for filename/relative-path literals, `search_files` for content,
and `read_file` ranges/tail for relevant text. Discovery only reads metadata;
text search retains its existing generated-directory, link and scan limits.
Every partial scan is reported. Absence inside inspected data cannot establish
absence outside it. Long matching lines now return an original UTF-8 window near
the match, with byte offset and truncation markers. Unicode lowercase offsets
must be mapped to original characters before slicing (for example capital İ).

`system_info` is a bounded, read-only PATH candidate inventory: OS, architecture,
cwd and fixed executable names. It never starts candidates or reports an invented
version. Found files are not execution proof. Its 4 KiB JSON, two-second wait,
64-directory and 512-probe limits deliberately exclude unrestricted environment,
clipboard and window enumeration. OS account file access remains authoritative.

`run_script` selects Python, PowerShell, cmd or sh explicitly. The Node ships no
interpreter. Windows Python is `python`; Unix Python is `python3`; PowerShell is
`powershell.exe` / `pwsh`. No fallback replays an operation through a different
interpreter. User source is at most 64 KiB; default deadline is 60 seconds,
maximum 3600 subject to the Run deadline. Output payload defaults to 8 KiB
(maximum 128 KiB), divided between stdout/stderr with head and tail retained.
UTF-8 lossy decoding is bounded again; labels/error metadata add a small overhead.
The legacy `exec` output contract is retained.

Temporary scripts use random create_new files, Unix mode0600, explicit cleanup
and Drop fallback. PowerShell source retains param/using declarations; a separate
bootstrap sets UTF-8/error handling and invokes source via a path argument.
Windows passes a process-local ExecutionPolicy Bypass; machine configuration is
unchanged and Group Policy remains authoritative. cmd disables AutoRun and delayed
expansion and uses native path quoting. Unsupported cmd temporary path characters
are rejected before execution. Script argv is not nested into an outer shell.

Both shell and script tools share process groups (Unix) / Job Objects (Windows).
Success, nonzero exit, timeout and cancellation settle after descendants are
terminated. Cancellation cannot undo completed file changes. Nonzero exits are
errors with bounded output, not successful Agent steps. No OS sandbox is claimed.

## Efficiency boundary

The system policy recommends narrow retrieval, explicit multiline interpreters,
checking state after unknown effects, and verifying actual modified artifacts.
These are mechanisms to reduce irrelevant output and quoting retries, not a
measured global speedup. The real-model trial completed with one recoverable
CRLF exact-edit failure. Supplier tokens were unavailable; no token-saving
percentage or general intelligence benchmark is claimed.

## Computer use next slice

Current tools do not provide desktop observation or coordinate actions. The
pinned model defaults to text input; current Host event conversion emits text,
and the Gateway route has no trusted vision-capability contract. Merely exposing
a screenshot or setting a model name is insufficient.

The next bounded Windows slice should select an explicit PID/window, return a
limited accessibility text tree, and isolate potentially hung UI Automation
calls. Screenshot/action support then needs trusted vision admission, bounded
artifact identity/hash/time/DPI/region, the same observation in model and UI,
action binding to that observation and an independent post-action check. Locked
desktop, UAC and stale observation must fail explicitly. Windows acceptance cannot
stand for Wayland/macOS. Full UIA/vision/action execution remains unimplemented.
