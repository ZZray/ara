# Core steering execute 受控证据

状态：已通过受控测试，未完成真实 sidecar/Host 实时验收。
目标快照：当前 `dev` 工作树，HEAD `9a65b7992179ecf91f6a440ff250a4bd7e0fe295`，包含本次未提交测试与矩阵登记。

## 要求与范围

验证 Host 在第一个模型请求期间提交 steering 后，Core `execute` 是否把消息纳入下一次模型请求，并为上下文准入产生 `applied` 回执。`applied` 只证明消息进入当前执行上下文，不证明模型已经消费该消息。

## 入口与命令

测试入口：`crates/ara-agent-lite-core/tests/steering_execute.rs::host_steering_is_admitted_into_the_next_execute_turn_with_a_receipt`。
上游：测试内受控 loopback OpenAI-compatible provider。
命令：

```powershell
$env:CARGO_TARGET_DIR='D:/Temp/ara-core-reuse-target-20261110'
cargo fmt --manifest-path crates/ara-agent-lite-core/Cargo.toml --all -- --check
cargo clippy --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test steering_execute --no-deps -- -D warnings
cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline --test steering_execute -- --nocapture
```

## 结果

- fmt：通过。
- clippy：通过，无 warning。
- test：`1 passed, 0 failed`，执行耗时约 0.05 秒；首次构建耗时约 58.43 秒。
- 观察到至少两个模型请求；第二个请求体包含 `Host steering: include the updated sample state.`。
- `SteeringBuffer::outcomes_snapshot()` 返回一个 `id=steer-1`、`status=applied` 的回执。

## 审查

独立只读 diff review 覆盖测试、Support_Matrix Markdown/JSON 和本证据文件。首轮发现并已修正两项：Cargo 测试目标从错误的 `test` 改为 `steering_execute`，测试增加 `RunResult.code == 0` 断言。修正后重新运行 fmt、clippy、JSON 解析和 targeted test，结果全部通过。OCR delegate 曾因不接受 `--paths` 参数返回命令错误，未作为审查结论使用；实际审查回到 Git diff 和目标文件。

## 限制

- 本测试不证明真实模型消费 steering，不证明真实 sidecar 的实时事件到达，也不证明 UI/RPC 展示。
- 本测试未覆盖取消、迟到 steering、上下文写入失败和跨进程恢复；这些保持在 `docs/tasks.md` 的后续任务。
- 本证据未接受产品 Task；真实 Host 业务验收仍由 Harness/产品 Host 完成。
