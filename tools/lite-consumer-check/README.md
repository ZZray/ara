# lite-consumer-check

需求 31 的树外消费者 crate（任务 58.1）。它有自己的空 `[workspace]`，不属于任何 Lite crate 的 workspace，只通过 `Cargo.toml` 的 path 依赖使用 `ara-agent-lite-core`、`ara-agent-lite-harness`、`ara-agent-lite-jsonl`，源码只引用 [Stability_Doc](../../crates/ara-agent-lite-core/docs/stability.md) 标为 stable 的项。

## 内容

- `src/lib.rs`：最小 Host `ConsumerHost`。
  - 三个工作区工具 `ws_read_file`、`ws_list_dir`、`ws_write_file`，经 `ara_agent_lite_core::rpi_agent::AgentTool` 实现。路径只接受工作区内的相对路径，拒绝 `..` 与绝对路径，并核对已存在祖先目录的规范路径仍在工作区内。这只是路径检查，不是 OS 沙箱。
  - `HarnessHost::verify` 在工作区运行运营方给定的 public 检查命令。退出码 0 判 `Verified`，否则判 `NeedsChanges`，并把输出尾部写进反馈。每次检查写一份 `state/host/check-run-N.json` 作为回执引用。
  - `HarnessHost::prepare_continuation` 在同一 Session 中用新 Run ID 和反馈目标续写，复用同一组 ModelHost、steering 和工具 Arc（Harness 会核对它们的身份）。
- `src/main.rs`：命令行入口，经 Harness `start` 运行一个 Task，并把 `TaskOutcome` 与墙钟时间以 JSON 打印。凭据从 `--credential-env` 指定的环境变量读取，不打印。退出码只表示 Host 的 public 检查是否通过；任务是否通过由进程外的 Oracle 判定（需求 31.3）。
- `tests/loopback.rs`：受控 loopback provider 测试。第一个 Run 写错文件，public 检查失败后在同一 Session 续写，第二个 Run 修好后停在 `AwaitingHostAcceptance`；另有一个测试覆盖工作区外路径被拒绝。

消费者不直接向模型 provider 发请求，也不自己实现“模型调用—工具执行—结果回填”循环，这个循环在 Core 的 `execute` 中运行。

## 构建与检查

```powershell
$env:CARGO_TARGET_DIR = 'D:\workspace\env\cache\rust-target-c'
cargo build
cargo clippy -- -D warnings
cargo test
```

`Cargo.lock` 以 `crates/ara-agent-lite-jsonl/Cargo.lock` 为起点解析。

## 已知限制

- Run_Policy 规定的生成策略 `thinking_type_disabled` 要靠自定义 `ModelHost::generation_policy` 实现，而该默认方法以及 `model::ModelSelection`、`model::CatalogModelHost`、`model::GenerationPolicy` 目前是 experimental。所以本 crate 只能用 provider 默认策略，还不能按 Run_Policy 完整运行 `consumer_task`。要提升哪些项为 stable，需要用户决定（任务 58.1 的阻断条件）。
- `ws_write_file` 写入失败时返回普通工具错误，没有标注结构化的未知副作用。
- 需求 31.3 的有界真实模型试验（任务 58.4）尚未运行。
