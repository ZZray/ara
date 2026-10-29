# lite-consumer-check

需求 31 的树外消费者 crate（任务 58.1）。它有自己的空 `[workspace]`，不属于任何 Lite crate 的 workspace，只通过 `Cargo.toml` 的 path 依赖使用 `ara-agent-lite-core`、`ara-agent-lite-harness`、`ara-agent-lite-jsonl`，源码只引用 [Stability_Doc](../../crates/ara-agent-lite-core/docs/stability.md) 标为 stable 的项。

## 内容

- `src/lib.rs`：最小 Host `ConsumerHost`。
  - 三个工作区工具 `ws_read_file`、`ws_list_dir`、`ws_write_file`，经 `ara_agent_lite_core::rpi_agent::AgentTool` 实现。路径只接受工作区内的相对路径，拒绝 `..` 与绝对路径，并核对已存在祖先目录的规范路径仍在工作区内。这只是路径检查，不是 OS 沙箱。
  - 自带 `ModelHost`：`resolve` 委托给 `model::CatalogModelHost`，`generation_policy` 与 `length_recovery_policy` 返回 `Config` 里由运营方给定的值，用于按 Run_Policy 的 `generation.generation_policy`（如 `thinking_type_disabled`）和 `generation.length_recovery` 运行。这几项在任务 57.1 提升为 stable，见 DR-P4-3。
  - `HarnessHost::verify` 在工作区运行运营方给定的 public 检查命令。退出码 0 判 `Verified`，否则判 `NeedsChanges`，并把输出尾部写进反馈。每次检查写一份 `state/host/check-run-N.json` 作为回执引用。
  - `HarnessHost::prepare_continuation` 在同一 Session 中用新 Run ID 和反馈目标续写，复用同一组 ModelHost、steering 和工具 Arc（Harness 会核对它们的身份）。
- `src/main.rs`：命令行入口，经 Harness `start` 运行一个 Task，并把 `TaskOutcome` 与墙钟时间以 JSON 打印。
  - `--generation-policy` 取 `provider_default` 或 `thinking_type_disabled`，`--length-recovery` 取 `stop` 或 `replan_once`，即两个策略枚举的 serde 名，与 Run_Policy 的写法相同。
  - `--route-binding`、`--route-decision` 可选，填入 `Route.binding` 与 `Route.decision`，默认为空。
  - 凭据从 `--credential-env` 指定的环境变量读取，不打印。
  - 退出码只表示 Host 的 public 检查是否通过；任务是否通过由进程外的 Oracle 判定（需求 31.3）。
- `tests/loopback.rs`：受控 loopback provider 测试，共 3 项。
  - 第一个 Run 写错文件，public 检查失败后在同一 Session 续写，第二个 Run 修好后停在 `AwaitingHostAcceptance`；`provider_default` 下 4 个模型请求都不带 `thinking` 字段。
  - `thinking_type_disabled` 下每个模型请求都带 `thinking: {"type": "disabled"}`。
  - 工作区外的路径被读写工具拒绝。

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

- 不设 Host 输出上限（`ModelHost::output_ceiling` 是 experimental）。Core 按内置规则取请求值、模型声明与 8192 的最小值，Run_Policy 的 `max_output: 32768` 实际不超过 8192。这与 `lite-eval` 的 B0 配置（`Features::V1`，不设上限）一致。
- 不覆盖 `ModelHost::capabilities_source`（experimental），`model_capabilities` 事件的 `source` 写 `host_model_configuration`；`lite-eval` 的 Host 转发目录 Host 的值，写 `hub_model_directory`。这个字段只是来源标签，不改变请求。
- `ws_write_file` 写入失败时返回普通工具错误，没有标注结构化的未知副作用。
- 需求 31.3 的有界真实模型试验（任务 58.4）尚未运行；引用扫描与门禁检查（任务 58.2）由 `lite-eval gate --consumer` 实现，尚未接入。
