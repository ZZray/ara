# ara-agent-lite-harness

供多个产品复用的任务执行层，版本 `0.1.0`。它通过公共 `execute` 接口调用
`ara-agent-lite-core 0.1.1`，在一次 Run 之外提供产物检查、反馈续作、累计预算、
Session 独占和证据记录。它与 Lite Core 分属独立 workspace。

## 各层负责什么

| 层 | 职责 |
| --- | --- |
| 模型 | 根据上下文推理、生成代码、选择工具和修正答案 |
| Agent Core Lite | 模型与工具循环、原生 Session、上下文维护、取消和 Run 预算 |
| Harness Lite | 检查 Run 是否形成完整候选、调用宿主验收器、按反馈续作、保持 Task 总预算 |
| 产品 Host | 用户身份、工具授权、工作区、模型入口、实际产物检查和业务 Task 验收 |
| 工具后端 | 编译器、测试、源码查询、反汇编或具体业务操作 |

AI Code Review 可以提供源码、差异、检查和复审工具；HandWave 可以提供获授权的
设备操作；其他应用可以使用自己的业务 API。它们复用同一个 Agent 循环与任务执行层，
不需要分别实现多轮调工具和反馈续作。

运行库不能替代模型本身的推理能力。工具是否可用、一次实际任务是否通过、
多类任务的成功率是否提高，需要分别验证。

## 接入

Rust 产品使用路径依赖，并在交付时固定实际 Git 提交或源码清单：

```toml
[dependencies]
ara-agent-lite-harness = { path = "../ara-github/crates/ara-agent-lite-harness" }
```

通过 `ara_agent_lite_harness::core` 使用同源 Core 类型，避免把不同版本的
`RunRequest` 混入同一个接口。当前库尚未发布到 crates.io。

接入者需要实现两项 `HarnessHost` 回调：

- `verify`：检查实际文件、工具收据或业务结果，先保存检查证据，再返回绑定当前
  `Candidate.basis` 的 `ReviewReceipt`。
- `prepare_continuation`：根据有效的 `NeedsChanges` 反馈创建下一 Run，请求沿用原
  Session 和授权，使用新的 Run ID。Harness 会限制剩余调用数和原截止时间。

然后创建 `TaskSpec` 和初始 `RunRequest`，调用 `start`。返回的 `TaskHandle`
支持 `subscribe`、`cancel` 和 `wait`。UI 订阅是可选观察者，慢订阅不会阻塞 Core。
完整可运行实现见 [capability_host.rs](examples/capability_host.rs) 和
[业务 Host 示例](examples/capability/host.rs)。

初始运行前，Host 应创建工作区、Session 父目录和证据父目录，提供绝对路径；
每个 Task 的 `evidence_dir` 必须尚不存在。所有续作必须复用同一模型 Host、工具和
steering 的 Arc 身份。工具实现内部的授权变化仍由产品管理。

Python、Node 和 C++ 等应用可通过独立的
[JSONL 适配进程](../ara-agent-lite-jsonl/README.md)转发业务工具和事件。
它直接依赖本包，提供 Python/Node 客户端；本 crate 的公开合同仍是 Rust API，
C ABI 尚未提供。已有产品也可以保留消息适配层，将任务推进部分接到这个公共包。

## 验收和停止

`AwaitingHostAcceptance` 表示 Host 检查通过，`task_accepted` 始终为 false；
产品决定是否接受业务 Task。返回码为零或模型说“完成”不能直接通过验收。

只有绑定当前候选、已保存、已结算且带有效反馈的检查结果才能启动下一 Run。
预算不会因新 Run 重置。错误收据、未知工具效果、损坏或变动的证据都会阻止续作。
`length` 默认保守停止。Host 可以显式启用
[Core 的有界长度恢复](../ara-agent-lite-core/README.md#输出长度恢复)，在原 Run 内重规划一次；
Harness 不把截断回复当普通候选，也不拼接输出或重放工具。

## 预算停止的可选产物检查

默认 `start` 仍要求普通的模型最终 Stop。最后一次模型调用虽然完成了工具操作，
但调用预算已耗尽、没有最终 Stop 时，Core 原 Run 仍返回失败：`code=-1`、
`agent_end.status=failed`。满足严格闭合条件时，原 `agent_end` 可附带
`termination`，其中 `version=1`、`cause=call_budget_exhausted`、
`native_round_closed=true`、`model_final_stop=false`。它证明原生消息与工具回执
已配对并保存，不证明外部效果结算、产物正确或业务 Task 完成。

Execute Host 可在 Task 准入时显式启用独立的预算快照检查：

```rust
use ara_agent_lite_harness::{
    start_with_options, BudgetStopReviewPolicy, TaskOptions,
};

let options = TaskOptions {
    budget_stop_review: BudgetStopReviewPolicy::ReviewClosedToolRound,
};
let handle = start_with_options(spec, initial_request, host, options)?;
```

这条路径使用新增类型和回调，原 `start`、`TaskSpec`、`Candidate`、`ReviewBasis`、
`Continuation`、`TaskOutcome` 和 Core `RunResult` 的公开字段保持不变，原有 Rust
结构体构造及 Host 实现可继续使用。两个新回调默认拒绝，不会转调旧回调：

- `verify_budget_snapshot(BudgetStopSnapshot, cancellation)`：检查实际产物及操作收据，
  返回绑定完整 `snapshot.basis` 的 `ReviewReceipt`。`diagnostic_output` 是原 Run 的
  诊断文本，可以为空，不能当作模型最终答案。
- `prepare_budget_stop_continuation(BudgetStopContinuation, cancellation)`：仅在有效
  `NeedsChanges` 且原 Task 仍有调用、Run 和时间额度时调用。`previous` 是预算快照，
  其余字段提供原检查收据、下一 Run 序号、剩余额度和原绝对截止时间。

Host 检查通过后，Task 只能进入 `AwaitingHostAcceptance`，`task_accepted` 仍为 false，
原 Core Run 的失败记录保留。若产物需要修改但原 Task 额度已经用尽，则停止为
`BudgetExhausted`，不调用续作回调，也不补发一次模型请求。该选项在 Task 内固定，
不会随新 Run 重置预算或延长截止时间。

预算路径单独保存 `run-N-budget-snapshot.json`。所有返回的检查收据先保存到
`run-N-budget-review.json`；只有回调已结束、收据已提交并结算、basis 相同且原件
复核通过后，才写 `run-N-budget-review-index.json`。索引的类型为
`BudgetStopReviewRecord`，包含 `version=1`、`kind=closed_budget_snapshot_review`、
快照的 `path/sha256` 和检查 `receipt`。可用 `budget_snapshot_path` 与
`budget_review_index_path` 取得这些固定路径。

预算快照进入检查时会清除当前 outcome 中的旧 `last_candidate` 和旧收据，先前各 Run
的证据文件保留。预算检查的收据放在 `last_receipt`，不能与旧普通候选配对；应读取
对应 Run 的索引并核对快照哈希。Harness 在检查及续作回调前后复核快照和 Journal
来源。索引固定的是审核来源，实际工作区、测试结果和外部效果仍由 Host 保存证据。

未知效果、未配对工具、取消、超时、上下文或持久化故障不会因 opt-in 获得放行。
已知、已结算的工具错误可以交给 Host 判断，仍须以实际产物决定 `Verified` 或
`NeedsChanges`。该入口只支持 Execute；Plan、Goal 以及等待或未知 Job 等停止路径保持
原门禁。接入和验收边界见
[预算快照说明](../ara-agent-lite-jsonl/docs/evidence/closed-budget-snapshot-20261025.html)。

## 取消和证据

取消会等待原 Core 和已经启动的 Host 回调结束。回调必须响应取消并完成结算；
不合作的回调可能让等待持续，Host 应在工具实现中提供可观察的超时与清理。
丢弃一个 waiter 不会取消任务。

同一 Session 在 Task 期间持有跨进程 advisory lock，并保留磁盘 fence。
崩溃或未知效果后，后续准入会拒绝；库没有自动清除 fence 或重放的接口。
先检查原操作、Journal 和收据，再由产品决定恢复方式。

`outcome.json` 保守记录释放前状态；`released.json` 是成功释放的最终记录。
`FencePresence` 区分 Present、Released 和无法确认的 Unknown。

工具白名单、文件目录和 advisory lock 不构成 OS 沙箱。Host 应把控制状态放在工具
授权范围之外；执行不可信代码需要产品另外提供账户、容器或等效隔离。

## 既有验证记录

2026-10-25 的 Windows 限定执行结果：

- 14 项回归通过，包括同 Session 文件纠错、累计调用预算、取消等待工具结算、
  证据写失败、原生未知效果和禁止重放。
- 普通进程验证 Session 竞争、取消、崩溃 fence 和正常释放。
- 三类受控案例均完成两 Run 反馈纠错。
- 三类真实模型案例通过实际产物检查：C++ 60 项边界、排程穷举最优解、
  去符号对象恢复的 64 项输入。
- 独立 workspace 消费者仅依赖公共 Harness，实际完成一次真实模型 C++ 修复。

这些结果只覆盖当前未触发压缩的 Execute profile。真实 summary 与 agent 共享预算、
完整 Goal/Job 恢复、prepare 回调取消矩阵、跨平台和更多陌生工程仍未验收。
没有同模型、同任务、同预算的增强前后对照，不能据此给出泛化能力提升比例。

`cargo deny` 按根目录现有政策运行，离线 advisory 检查通过，bans、licenses、sources
检查失败；依赖与发布门禁保持未完成。AI Code Review、HandWave 等产品未因本次
验证升级或部署。详见 [验证报告](docs/evidence/lite-harness-20261025.html)。

## 本地验证命令

```powershell
cargo fmt --manifest-path crates/ara-agent-lite-harness/Cargo.toml --all -- --check
cargo clippy --manifest-path crates/ara-agent-lite-harness/Cargo.toml --locked --offline --all-targets --no-deps -- -D warnings
cargo test --manifest-path crates/ara-agent-lite-harness/Cargo.toml --locked --offline --all-targets
cargo test --manifest-path crates/ara-agent-lite-harness/Cargo.toml --locked --offline --doc
cargo build --manifest-path crates/ara-agent-lite-harness/Cargo.toml --locked --offline --examples
```

仓库内验证应按项目约定显式指定 `--target-dir`。普通示例执行使用新目录；默认是
受控 provider，真实模型必须显式加 `--run-live`，并通过环境变量提供模型配置。
配置字段及工具范围见示例入口注释。不要复用已有试验目录或删除原 fence 来重试。
