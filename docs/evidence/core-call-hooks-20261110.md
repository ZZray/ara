# Core per-call Host hooks 证据

日期：2026-11-10

## 结论

Lite Core 新增 experimental `HostCallHooks`。Host 通过拥有数据的 DTO 接收工具调用信息，不需要依赖 `rpi_agent` 的回调生命周期类型。`RunRequest.call_hooks=None` 时保持原有路径。

## 契约

- `before_tool_call` 接收 `call_id`、工具名和已校验参数，返回 allow/deny 与可选原因。
- `after_tool_call` 接收调用参数、文本输出、details 快照和 `is_error`。
- hook 返回错误时，Core 记录失败并取消当前 Run；before hook 失败不会派发工具。
- duplicate native ID 会在 Host before hook 之前由 Core 拒绝，避免把已使用的调用重新交给 Host 审批。
- 取消/超时后的 Host 结算、异步 hook 和业务审批仍由后续 Host/Harness 验收负责。

## 验证

```text
cargo fmt --manifest-path crates/ara-agent-lite-core/Cargo.toml --all -- --check
cargo clippy --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test call_hooks --no-deps -- -D warnings
cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test call_hooks -- --nocapture
```

结果：fmt、clippy 通过；`call_hooks` 5 passed、0 failed。受控测试覆盖 before deny 不派发工具、after success/error 观察、after hook failure 停止下一次模型请求、无 hook 回归和 before hook failure 传播。

测试使用 loopback provider 和 `ScriptedTool`，不代表真实 Host、真实模型或产品审批流程验收。
