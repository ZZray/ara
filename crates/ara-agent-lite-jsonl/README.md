# ara-agent-lite-jsonl

供 Python、Node 和其他语言接入公共 Agent 的 JSONL 子进程，版本 `0.1.0`。
它调用 `ara-agent-lite-harness`，由 Harness 调用 Lite Core 的公共 `execute`。
产品只实现业务工具、产物检查和反馈回调，不需要复制 Agent 循环。

## 接入边界

| 层 | 负责的行为 |
| --- | --- |
| Lite Core | 模型和工具循环、原生 Session、上下文、Run 预算和取消 |
| Harness Lite | 候选检查、Host 验证、反馈续作、Task 总预算、Session 独占和原始证据 |
| JSONL sidecar | 一次 Task 的跨进程消息、关联校验和有界传输 |
| 产品 Host | 身份与权限、工具、模型授权、状态目录、实际效果结算和业务验收 |

当前 profile 接受 `execute` / `plan` 和 OpenAI Chat 路由。通用本地工具、worker、
Goal 与 Job 没有通过该协议开放。Host 注册的工具仍由产品执行；白名单不是 OS 沙箱。
模型的推理水平、工具后端的能力与任务执行机制需要分别评估。

## Python 和 Node

Python 客户端只用标准库，要求 Python 3.11 或更新版本；已有 Python 3.13 和产品环境 3.14 的执行记录。
将 [ara_agent_lite.py](clients/python/ara_agent_lite.py) 所在目录加入模块搜索路径：

```python
from ara_agent_lite import run_task

outcome = await run_task(
    absolute_binary, start_task_frame, product_host,
    token=model_token,
    operation_directory=fresh_absolute_receipt_directory,
    cancellation=cancel_event,
    on_event=observe_ui_event,
)
```

`product_host` 实现三个异步方法：`tool_call(request, cancelled)`、
`verify(candidate, cancelled)` 和 `prepare(continuation, cancelled)`。
`cancelled` 是 `asyncio.Event`。阻塞的业务函数应交给线程或产品进程，并保留自己的
超时、取消、结算和收据。原件读取和身份检查在进入回调前进行。

Node 使用无第三方依赖的 ESM 客户端：

```js
import { startTask } from './clients/node/client.mjs';

const task = await startTask({
  binary: absoluteBinary,
  start: startTaskFrame,
  modelToken,
  operationDirectory: freshAbsoluteReceiptDirectory,
  callbacks: { tool, verify, prepare },
  onEvent,
});
const result = await task.wait();
```

Node 的 `tool` 收到原请求和 `{ signal }`；`verify` / `prepare` 收到候选或续作信息
和 `{ signal, frame }`。`task.cancel()` 同步发出取消请求，`task.wait()` 等待原回调结算。
`runTask(options)` 包含等待步骤。Node 返回的 `delivery` 与原 `outcome` 分开保存；
校验过终态后发生进程异常，也要保留原终态及传输失败信息。
更多接口见 [Node 说明](clients/node/README.md)。

## 消息和配置

一进程只接受一个 `start_task`。Host 应提前创建工作区和 Session 父目录，使用绝对路径；
Task 证据目录及客户端操作目录必须是新目录。初始化帧包含固定的 `task` 和 `run`。
完整字段和返回值见 [JSONL v1 协议](docs/protocol-v1.md)。

模型凭据仅通过子进程环境 `ARA_AGENT_MODEL_TOKEN` 传入，不放进 stdin、argv 或收据。
`model_resolution` 支持：

- `catalog`：按 Core 的模型目录协议读取模型能力，事件来源为 `hub_model_directory`。
- `host_declared_loopback`：产品持有的 loopback Relay 明确提供能力，事件来源为
  `host_model_configuration`。仅允许带端口的字面 loopback HTTP 地址和 `/v1` 路径。

sidecar 固定初始模型、工具、路径和授权版本。`prepare` 只返回新 `run_id_next` 和
包含实际反馈的 `objective`，不能更换 Session、授权、模型、工具或扩大预算。
产品使用 fallback Relay 时，应让同一个 Relay 和实际 attempts 账本覆盖整个 Task。
Core 的逻辑调用数与上游实际尝试数分别统计。

当前工作区支持可选 `run.generation_policy`：省略或 `"provider_default"` 保留原请求；
宿主已确认服务支持时，可显式设置 `"thinking_type_disabled"`，由 Core 在实际
OpenAI Chat 路由的主任务请求中发送 `"thinking":{"type":"disabled"}`。
该策略绑定整个 Task，反馈续作和同 Run 长度恢复沿用同一配置；`prepare` 无法改写。
未知字符串、null 和对象值在初始帧被拒绝，不接受任意 `extra_body` 或采样字段。
本地事件仅证明配置，服务端是否执行及最终任务正确性需另外核验。
旧程序会拒绝这一新字段，接入时应固定支持它的源码及镜像哈希。

`run.length_recovery` 默认为 `"stop"`。Execute Host 可以显式设置 `"replan_once"`，
允许 Core 在原 Run 内对完整、无工具调用的 `length` 进行一次有界重规划。
原消息、输出上限、调用计数、授权和截止时间不变；再次出现无工具调用的截断仍停止。
每个 Run 最多一次，多个反馈 Run 仍受原 Task 总预算约束。自动控制来源被独立记录，
不会成为新的人类确认。详见
[Core 长度恢复](../ara-agent-lite-core/docs/evidence/length-recovery-20261025.html)。

## 预算停止的显式检查入口

`budget_stop_review` 是 `start_task` **顶层**的可选字段，省略或 `"reject"` 时保持
原行为。Execute Host 如需检查预算耗尽时留下的闭合工具轮次，可设置：

```json
{"budget_stop_review": "review_closed_tool_round"}
```

这不会把原 Core Run 改为成功。它仍保留 `code=-1` 和失败事件；只有原生轮次完整
配对、保存成功，且 `agent_end.termination` 明确记录闭合调用预算原因时，Harness 才
可能交付 `BudgetStopSnapshot`。其中的 `diagnostic_output` 不是模型最终回答。
未知效果、未闭合工具、取消、超时、上下文及持久化故障仍阻止检查或释放。

启用前，Python Host 和 Node `callbacks` 都必须显式实现两个专用异步回调：

| 回调 | 接收内容 | 返回内容 |
| --- | --- | --- |
| `verify_budget_snapshot` | `basis/diagnostic_output/goal/termination` 快照 | 绑定完整 `snapshot.basis` 的 `ReviewReceipt` |
| `prepare_budget_stop_continuation` | `previous` 快照、`receipt`、下一 Run 序号、剩余调用数和原截止时间 | 仅 `run_id_next`、`objective` |

Python 方法签名为 `verify_budget_snapshot(snapshot, cancelled)` 和
`prepare_budget_stop_continuation(continuation, cancelled)`；Node 的第二个参数仍为
`{ signal, frame }`。SDK 在启动子进程前检查 Execute 模式和两个回调是否可调用。
预算回调不会转调普通 `verify` / `prepare`；旧 Host 默认不支持这条路径。

原生 RPC 同样独立：`verify_budget_request` 使用 `snapshot` 或 `snapshot_ref`，回复
`verify_budget_reply`；`prepare_budget_request` 使用 `budget_continuation` 或
`budget_continuation_ref`，回复 `prepare_budget_reply`。SDK 会在回调前后校验权威原件
和哈希，产品回调按已校验的对象返回原收据或新的 Run ID、实际纠错目标。

Host 仍须读取实际文件、测试或业务回执，保存证据并完成效果结算。有效 `Verified`
只让 Task 到达 `AwaitingHostAcceptance / task_accepted=false`，原 Run 失败记录不变。
有效 `NeedsChanges` 只使用原 Task 的剩余额度和截止时间；额度耗尽时停止为
`BudgetExhausted`，不补模型调用或重放旧操作。

快照和收据使用独立的 `run-N-budget-snapshot.json`、`run-N-budget-review.json`。
通过完整来源与收据校验后才提交 `run-N-budget-review-index.json`，其
`kind=closed_budget_snapshot_review`。当前预算检查的 outcome 保持
`last_candidate=null`，检查收据在 `last_receipt`；先前普通候选的文件仍保留。
终态可附带与当前收据精确关联的 `budget_review_ref`，不会回退引用旧 Run 的索引。

Python 客户端把校验过的索引另存到操作目录的 `budget-review.json`，并继续返回原
outcome；Node 在独立 `budget-review` ledger 记录索引，`result.outcome` 也保持原样。
需要重新校验引用时，可使用 Python `read_budget_review_reference` 或 Node
`readBudgetReviewReference`，传入引用、原 Task 证据目录、原 Task 配置及 outcome。

该入口保留原 Rust `RunResult`、`TaskSpec`、`Candidate`、`ReviewBasis`、
`Continuation`、`TaskOutcome` 的字段构造合同。旧客户端不启用时沿用旧协议；显式
新字段需要支持它的新 sidecar，旧程序会拒绝，不会自动降级。此入口的证据与限制见
[预算快照说明](docs/evidence/closed-budget-snapshot-20261025.html)，整体能力验收仍未完成。

## 验证、故障和恢复

工具回调返回 `value`、`is_error`、`settlement` 和非空 `operation_receipt`。
已知在执行前拒绝的操作可返回 `is_error=true / Settled`，但需要实际收据支持。
调用之后的异常、丢连接或超时可能已经产生效果，不能自动记成未执行。

Host 根据真实文件、测试、数据库回读或原业务回执检查候选，先保存证据，再返回
绑定完整 `candidate.basis` 的 `ReviewReceipt`。有效的 `NeedsChanges` 可以续作；
Task 总调用数和绝对截止时间保持原值。

`AwaitingHostAcceptance` 表示产物检查通过，公共层的 `task_accepted` 始终为 false。
最终业务 Task 仍由产品接受。取消会向原回调发送 `callback_cancel`，等待同一 RPC 的
原回复；本地 grace 到期只说明本地等待结束。外部操作是否结束、是否回滚由产品结算。

客户端不会重启子进程、创建新 Session 或自动重放工具。断连后先检查原 Task outcome、
Journal、fence 和产品操作收据。`released.json` 是释放后的终态原件；`outcome.json`
保存其他终态。UI 事件允许丢弃，不能用于重建完整账本或判断验收。

## 产品候选

[AI Code Review 示例](examples/ai_codereview_host.py) 在独立历史源码副本中调用当前产品的
真实 `Workspace` 和工具 schema。它只允许修改指定源码，固定七项回归测试；不接入
生产队列、发布或独立模型复审，也不改产品现有 Core pin。
没有实际补丁的候选会收到已保存的 `NeedsChanges` 反馈，明确检查测试进程尚未启动；
只有实际七项回归通过且测试源码未变时才返回 `Verified`。未知检查异常仍保留为待核查。

[LabSlot 示例](examples/labslot_host.mjs) 接收产品注入的真实工具、原 schema 转换、
授权闭包、数据库回读和持久收据。首候选仅开放 Todo 查询及选定 Todo 的优先级更新。
自动反馈 Run 沿用原用户、会话、人类消息序号及确认版本。普通 Node 测试中的注入
替身只验证接入合同，真实 TodoService/PostgreSQL 验收仍未完成。

## 本地验证

```powershell
$env:CARGO_TARGET_DIR = 'D:/Temp/ara-build/ara-github-target-20261005'
cargo fmt --manifest-path crates/ara-agent-lite-jsonl/Cargo.toml --all -- --check
cargo clippy --manifest-path crates/ara-agent-lite-jsonl/Cargo.toml --locked --offline --all-targets --no-deps -- -D warnings
cargo test --manifest-path crates/ara-agent-lite-jsonl/Cargo.toml --locked --offline --all-targets
cargo test --manifest-path crates/ara-agent-lite-jsonl/Cargo.toml --locked --offline --doc
node --test crates/ara-agent-lite-jsonl/clients/node/client.test.mjs crates/ara-agent-lite-jsonl/clients/node/labslot-todos.test.mjs
```

仓库内 Cargo 命令显式指定项目约定的 `--target-dir`。Python 实际进程测试需要
`ARA_JSONL_TEST_BINARY` 和新的 `ARA_JSONL_TEST_EVIDENCE` 绝对目录：

```powershell
python -m unittest discover -s crates/ara-agent-lite-jsonl/tests -p test_process.py -v
```

验证只覆盖记录的 Windows profile。长度恢复按上述显式策略单列验收；
[来源与连续性验证](../ara-agent-lite-core/docs/evidence/context-continuity-20261025.html)
另外覆盖同 Session 跨两个 Task、三个摘要及原文恢复。完整恢复、产品默认迁移、跨平台和发布保持 OPEN。
按既有 Lite 独立政策进行的 JSONL 离线依赖检查通过，公告库时效未知；历史根目录政策
审计失败的原件保留，没有修改政策或增加发布豁免。整体七项目标仍为 `0/7` 整体验收。

[生成策略与历史修复验证](docs/evidence/generation-policy-20261025.html)记录当前策略接线、
空补丁反馈，以及一次真实修复任务：最终源码在新目录通过七项测试，但原任务因调用预算
耗尽没有形成普通候选，仍未验收。这没有修改 AI Code Review 产品的旧 Core pin、Relay 或生产环境。
执行结果、独立审查和保留的失败见
[本轮报告](docs/evidence/product-host-adapters-20261025.html)及
[机器可读结果](docs/evidence/product-host-adapters-20261025.result.json)。
