# Host boundaries

The requested product is a small standalone computer Agent controlled through
HandWave cloud messages, not completion of the full OMP port. The independent
workspace pins `bigfish1913/pi-rust` to
`8ec462672d90e62712d96074638efef89436cebe` and reuses its Agent/AI loop. Identity,
permissions, persistence, process cancellation, cloud grants and tools belong
to this Host. It does not replace the old Go node identity or storage.

Current HandWave contract evidence is `E:/repos/ara/apps/handwave`, with exact
build identity recorded in delivery evidence. The older `ai-handwave` checkout
has a different model-routing contract. Model grants from a modern Hub resolve
Auto through its actual default model/protocol and read the shared Gateway
directory. Smart mode uses the complete server-issued binding and association
headers; model/protocol/binding changes do not silently reuse old history.

This compact delivery speaks openai-completions/openrouter-chat. Other protocols
fail explicitly. It has no automatic grant renewal/replay: the directory, model
loop and tools share a bounded deadline below both grant and decision expiry.

Files may be absolute; cwd is a relative-path base, not an OS sandbox. Commands
run with the OS user's authority. Side effects already completed before cancel
remain. An unfinished persisted tool call blocks continuation until explicitly
investigated; reconnect does not erase that fence or replay unknown effects.

Windows uses static CRT and OS APIs for process trees. GNU Linux artifacts have
a measured libc requirement; a successful WSL run does not prove support on
older distributions, ARM, musl or macOS. Build time, local loop latency and
provider/model latency are separate measurements.
