# Harness 与 JSONL 恢复证据

日期：2026-11-10
交付快照：`47388eacb2d19904b817d9119d92ac57df6a649e`

## 范围

本轮回源检查了现有 `resume_checked`、`ResumeRefusal`、JSONL `rejection_category`、stderr 输出和恢复回调。代码已在交付快照中具备这些路径，本轮没有重复实现。验证使用受控 loopback provider、fake Host 和本地 sidecar 进程。

Harness 的固定拒绝类别包括 `authority_changed`、`budget_larger`、`run_caps_larger`、`fence_mismatch`、`settlement_unknown`、`evidence_empty`、`call_mismatch`，并保留 `no_fence`、`fence_unreadable`、`task_terms_mismatch`、`deadline_elapsed`、`budget_exhausted`、`host_callback_failed` 和 `receipt_uncommitted`。`ResumeRefusal` 通过 `Serialize` 输出 snake_case，`Display` 不包含输入值。

JSONL 在恢复失败时只输出固定类别到 stderr。Host 回调之后写入 `jsonl-resume-rejected.json`，内容包含类别和 `no_replay` 标记。Harness 在 Host 回调之前拒绝时不创建新的证据目录，也不发送上游请求。成功恢复继续原 Task 和原 Session，写入恢复回执后再发送后续模型请求。

跨 Run 续作由 `Authority::check` 同时核对声明哈希、`ModelHost` 注册表、steering owner、工具注册表和 completion gate。TaskSet 为每个 Task 保持独立的 steering buffer，并把 `task_set_id`、`task_id` 和消息 ID 写入回执。停止复核和结构化 unknown-effect 路径继续使用原 Session、原 Task 预算和原工具结算，成功 Run 仍停在 Host 验收边界。

## 源码与测试入口

- `crates/ara-agent-lite-harness/src/runner.rs::resume_checked`
- `crates/ara-agent-lite-harness/src/types.rs::ResumeRefusal`
- `crates/ara-agent-lite-harness/tests/resume.rs`
- `crates/ara-agent-lite-jsonl/src/lib.rs::resumed` 与 `rejection_category`
- `crates/ara-agent-lite-jsonl/src/runtime.rs::ExternalHost::reconcile_resume`
- `crates/ara-agent-lite-jsonl/src/rpc.rs::strict_resume_receipt`
- `crates/ara-agent-lite-jsonl/tests/test_jsonl_resume.py`
- `crates/ara-agent-lite-jsonl/tests/test_resume_process.py`
- `crates/ara-agent-lite-jsonl/tests/test_protocol_doc_examples.py`
- `crates/ara-agent-lite-jsonl/tests/test_resume_cross_language.py`
- `crates/ara-agent-lite-jsonl/tests/test_python_resume.py`

## 执行结果

| 检查 | 结果 |
|---|---|
| Harness `cargo test --all-targets` | 69 passed, 0 failed |
| Harness resume refusal matrix | 7 passed, Host callback 未被前置拒绝路径调用 |
| JSONL Rust `cargo test --all-targets` | 12 passed, 0 failed |
| JSONL sidecar resume/process/protocol Python tests | 12 passed, 0 failed |
| JSONL Python/Node cross-language resume tests | 6 passed, 0 failed |
| JSONL generation-policy tests | 7 passed, 使用当前 sidecar SHA `2103dbf693b6ab4a71835526351e04c7fe2e1cfc32f50b0b5ea78fa745dbf3b9` |
| Harness/JSONL fmt 与 clippy | 通过 |

首次从仓库根目录调用 `test_generation_policy.py` 时，测试导入路径未包含测试目录，7 项均未执行并报告 `controlled_provider` 导入错误。切换到 `crates/ara-agent-lite-jsonl/tests` 后，按测试要求设置当前二进制 SHA，7 项全部通过。这是测试入口和环境变量问题，未修改测试代码。

## 未覆盖边界

上游仍是受控 fake provider，未覆盖真实模型、真实产品 Host、跨进程并发写入、真实 MCP server、设备操作和发布门禁。阶段 2 的恢复证据不代表三类产品 Host 验收完成。
