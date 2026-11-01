# TaskSet 接入

2026-11-01。接口为 experimental；验证结果另见
[本轮证据](evidence/task-set-20261101.md)。

## 用途与分工

TaskSet 将 Host 的接待循环与具体任务的等待分开。Host 根据已授权请求创建
`TaskSpec`、独立工作区和 `RunRequest`，调用 `TaskSet::start` 后继续处理其他消息。
任务仍由原 Harness 执行和复核。`next_completed` 可放进 Host 的 `tokio::select!`；
另一个分支收到消息时可丢弃本次等待，再向明确的 Task 投递。

思想来源为 DeepSeek 的
[Super-agent 原型](https://github.com/deepseek-ai/dsh-superagent-prototype/tree/6337fc823ce20251c8de8085948165c81aaa701e)
中接待与执行分离、原任务处理后续消息的设计。此次使用已有 Rust Harness 实现，
没有移植其代码，也没有实现其全部系统。

本模块不运行额外 Router 模型。简单任务仍可直接调用 `start`；需要同时观察多项
工作时再使用 TaskSet。已有 `tools/ara-lite` 的持久任务、消息、租约和验收协议保持原样。

## 消息与完成

- Host 先检查消息发送者、来源和原任务范围，再调用 `guide(task_id, message_id, text)`。
  关联到 Task 不等于取得授权；CI 或工具输出也不能扩大权限。
- `Queued` 只表示入原 steering buffer。同 ID、同内容返回 `Duplicate`，不同内容拒绝。
  幂等范围是当前 TaskSet 中的该 Task，不是跨平台或跨进程保证。
- 每条消息最多 16 KiB UTF-8，消息 ID 最多 256 字节。每 Task 的消息数和整批 Task 数
  由构造参数限制；Core 的 16 条 pending 上限仍有效，满时明确报错，不记为已接收。
- steering 在原模型/工具轮次边界消费。阻塞工具不会因新消息立即中断。
- `next_completed` 等原 Task 和 Host 回调结算后清理剩余消息，返回
  `AppliedToContext`、`Unapplied` 或 `Unknown`。前者只证明上下文准入，不保证模型读取；
  后两者由 Host 处理，不能自动重放。该清理不改写原 `TaskOutcome` 或验收结论。
- 完成 ID 和消息摘要保留到整个 TaskSet 丢弃；完成项不释放批次槽。长期 Host 应将
  任务身份、结果、去重和恢复放在自己的持久协调层，按批次管理 TaskSet 生命周期。
- 不同 Task 不能共用 steering Arc；同任务续作必须沿用原 Arc。Host 将该 buffer 的
  投递和终态 `finish` 交给 TaskSet，不能另一个任务同时消费它。
- `cancel`/`cancel_all` 只请求取消；继续等待原完成结果，确认原工具已结算。
  丢弃等待 future 或 TaskSet 不会取消后台工作，也不能证明副作用已撤销。

## 运行双 Task 示例

在已存在的父目录下选择两个尚不存在的绝对目录，写入配置，例如：

```json
[
  {"kind":"reasoning","root":"D:/workspace/task-set-demo/task-a","max_calls":8,"max_runs":2,"max_seconds":60},
  {"kind":"reasoning","root":"D:/workspace/task-set-demo/task-b","max_calls":8,"max_runs":2,"max_seconds":60}
]
```

默认用受控 HTTP provider，执行真实工具、产物写入和 Host 校验：

```text
cargo run --manifest-path crates/ara-agent-lite-harness/Cargo.toml --example task_set -- CONFIG.json
```

示例在等待任务前输出 `host_available`，随后按完成顺序输出两个 `task_completed`。
各 Task 的独立 `host/` 目录保存原结果、消息回执和受控请求记录。
`workspace/plan.json` 是实际产物；Host 使用既有穷举校验判断排程。
这验证并发接待与执行机制，不证明模型推理能力或性能提升。

真实模型需显式加 `--run-live`，并在环境中配置 `ARA_CAPABILITY_MODEL_ENDPOINT`、
`ARA_CAPABILITY_MODEL_ID`、`ARA_CAPABILITY_MODEL_KEY`。先核对当前路由和模型能力，
再使用独立新目录和有界预算；不要将凭据写进配置或报告。该选项不启用通用 shell。

## 未覆盖边界

TaskSet 不是常驻调度服务，没有进程重启后的队列恢复，也不会接 GitHub、飞书等
外部事件。原 Core 的 NoProgress、Completion Gate、原 Session 恢复和预算规则保持
原行为。完整产品接入、跨任务记忆、自动分派与 Watchdog 均需单独设计和验收。
