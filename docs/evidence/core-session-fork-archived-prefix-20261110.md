# Core Session fork 归档前缀证据

日期：2026-11-10

## 结论

Lite Core 在源 Session 已安装摘要、Journal `active_base > 0` 时，仍可从 Journal 的原始记录前缀创建两个独立 fork。新 fork 以完整前缀作为热窗口，`active_base` 为 0；源 Session 的文件摘要保持不变。

## 实施

- `session_fork::fork_session` 不再把 `active_base > 0` 作为拒绝条件。
- Journal 仍通过 `inspect`、链完整性校验和 `raw_prefix(0..K)` 提供原始记录。
- `create_fork` 将前缀原始字节重建为新 Journal，并保持新 fork 的 `active_base=0`。
- 为保持目标 Session 当前检查点契约，分叉前缀仍受 `CHECKPOINT_MESSAGES=512` 和 `CHECKPOINT_BYTES=8 MiB` 限制，超限在写入目标前拒绝。
- 测试先运行两次受控 Execute。第二次因上下文预算触发摘要，确认源 `active_base > 0`，再从 `K=active_base` 分叉。

## 验证

命令：

```text
cargo fmt --manifest-path crates/ara-agent-lite-core/Cargo.toml --all
cargo clippy --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test session_fork --no-deps -- -D warnings
cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test session_fork -- --nocapture
cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test session_fork archived_summary_prefix_can_be_forked_from_raw_journal -- --nocapture
```

结果：

- fmt 通过。
- session_fork clippy 通过。
- session_fork 全量测试通过，8 passed、0 failed。
- `archived_summary_prefix_can_be_forked_from_raw_journal` 通过，1 passed、0 failed。
- 测试核对了 `active_base > 0`、两个 fork 的原始记录等于源 `0..K` 字节前缀、目标 Journal 的 `active_base=0`，以及 source digest 未变化。
- `oversized_archived_prefix_refuses_before_target_creation` 通过，确认 514 条归档前缀在创建任何目标前被拒绝。
- `oversized_archived_prefix_bytes_refuse_before_target_creation` 通过，确认 512 条但超过 8 MiB 的归档前缀在创建任何目标前被拒绝。

## 未覆盖

本证据使用 loopback provider 和受控 compaction，不代表真实模型、真实 Host、产品流程或跨进程并发写入验收。pending Run、Harness fence、cwd 不匹配、非完整 exchange 和目标占用的既有拒绝测试继续保留。
