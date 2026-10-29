# Lite 公共接口稳定性规则

本文是需求 29 的 Stability_Doc，规定外部产品可以长期依赖哪些 Lite 公共项。

- 适用版本：`ara-agent-lite-core 0.1.1`、`ara-agent-lite-harness 0.1.0`，另有 `ara-agent-lite-jsonl`、`ara-agent-lite-mcp` 各 0.1.0。表格以这些版本的代码为准：分支 `dev` @ `8e9b123`，四个 Lite crate 的 `src/` 与 `c061062` 相同。
- 状态：任务 57.1 定稿。初稿审查见 [DR-P4-2](evidence/lite-capability/design-records.md#dr-p4-2-公共接口初始稳定级别审查)，定稿结论（含 `policies` 字段与 P0–P4 新公开项）见 [DR-P4-3](evidence/lite-capability/design-records.md#dr-p4-3-稳定级别定稿任务-571)。
- 校验：Core 的 `tests/stability_surface.rs` 与 Harness 的 `tests/stability_surface.rs` 读取本文表格，逐项使用其中的 stable 项，并核对 stable 项数与测试使用数相等。
- 本文不规定 JSONL 协议帧的兼容性，帧格式与版本以 [JSONL v1 协议](../../ara-agent-lite-jsonl/docs/protocol-v1.md) 为准。

## 稳定级别与变更规则

| 级别 | 含义 | 变更规则 |
|---|---|---|
| stable | 外部产品可以长期依赖 | 新增字段必须可选：按本文规定的 `RunRequest` 构造方式编写的既有调用方无需修改即可编译，且不设置新字段时行为与新增前一致。删除、重命名、改变类型或改变已记录语义（含默认值）必须升级主版本；版本号低于 1.0.0 时，升级左起第一个非零版本位（例如 `0.1.1` 升到 `0.2.0`） |
| experimental | 可以试用，不承诺兼容 | 可在任意版本（含补丁版本）变更，不受 stable 规则约束 |
| internal | 不属于外部产品可依赖的接口 | 随时可变；外部产品引用即视为违反本文 |

补充规则：

1. 构造方式：`RunRequest` 只能先调用 `RunRequest::new(id, objective, RuntimePaths::new(..))`，再逐字段赋值。不得用结构体字面量构造；字面量构造的调用方在新增字段时需要自行修改，不受第 2 条“无需修改即可编译”的保护。
2. 判定按路径进行，取最长匹配：具体项（如 `ara_agent_lite_core::model::Route`）优先于所在模块的 `模块::*` 行。同一个类型经不同路径可见时，只有表中列出的路径享有该路径的级别；例如 `RunRequest` 经 crate 根是 stable，经 `ara_agent_lite_core::agent::RunRequest` 属于 `agent::*` 的 experimental。
3. 未列入本文的 Lite 公共项按 internal 处理。树外消费者引用 experimental、internal 或未列出的项时，交付门禁判失败（需求 31.7）。
4. `rpi_agent`、`rpi_ai` 两个重导出为 stable，外部 Host 实现工具必须经由它们（`rpi_agent::AgentTool`、`rpi_ai::Tool` 等）。经这两个路径访问的全部项按所在重导出计级。它们来自固定 rev 的上游 `pi-rust`；升级固定 rev 导致其中被使用的类型变化时，按 stable 规则升级主版本。
5. stable 字段的类型属于该字段的契约。字段改用其他类型、类型改名，或删除、重命名该类型已公开的字段或变体，都算“改变类型”。字段类型中没有单独列为 stable 的方法、trait 默认方法与字段按 experimental 处理；例如 `ModelSelection` 是 stable 的类型名，它的字段不是。
6. 由 Lite 构造、交给调用方读取的 stable 类型（`RunResult`、`TaskOutcome`、`Candidate`、`ReviewBasis`、`Continuation`、`EventEnvelope`）新增字段不需要升级主版本；调用方不得用结构体字面量构造它们，解构时须带 `..`（测试代码除外）。由 Host 以字面量构造的 stable 类型（`TaskSpec`、`ReviewReceipt`、`ToolRegistration`、`RuntimePaths`）新增字段、stable 枚举新增变体，都按“删除或改变类型”处理，必须升级主版本。

## Core：RunRequest 字段

`RunRequest` 共 31 个 `pub` 字段，与 crate 根的 2 个 `rpi_*` 重导出合计 33 项（需求 29.1）。

| 项 | 级别 | 说明 |
|---|---|---|
| `ara_agent_lite_core::RunRequest.model_host` | stable | 模型路由：`Arc<dyn model::ModelHost>`，默认 `CatalogModelHost` |
| `ara_agent_lite_core::RunRequest.worker_executable` | experimental | 后台 Worker 入口，需目标程序支持 Worker 参数 |
| `ara_agent_lite_core::RunRequest.extra_tools` | stable | 工具注册：`Vec<ToolRegistration>` |
| `ara_agent_lite_core::RunRequest.context_snapshot` | experimental | 外部云端快照，格式仍随 HandWave 兼容需求调整 |
| `ara_agent_lite_core::RunRequest.knowledge_roots` | experimental | 业务知识／技能根目录，`environment_library::Roots` 尚在演进 |
| `ara_agent_lite_core::RunRequest.enable_local_tools` | stable | 工具注册：是否启用内置本机工具，默认 false |
| `ara_agent_lite_core::RunRequest.mode` | stable | 模式：`RunMode`，默认 Plan |
| `ara_agent_lite_core::RunRequest.goal_criteria` | stable | 模式：Goal 验收标准 |
| `ara_agent_lite_core::RunRequest.goal_id` | stable | 模式：Goal 标识 |
| `ara_agent_lite_core::RunRequest.goal_lease` | experimental | Goal 租约，与调度外壳一起演进 |
| `ara_agent_lite_core::RunRequest.resume_job_lease` | experimental | 后台 Job 恢复租约，由 Host 核对取消 fence |
| `ara_agent_lite_core::RunRequest.session_key` | stable | 身份：宿主确认的 Session 归属 |
| `ara_agent_lite_core::RunRequest.expected_revision` | stable | 身份：Goal 命令的期望修订号 |
| `ara_agent_lite_core::RunRequest.reviewed_run_id` | stable | 身份：Goal 命令所审查的 Run |
| `ara_agent_lite_core::RunRequest.id` | stable | 身份：Run ID |
| `ara_agent_lite_core::RunRequest.device_fp` | stable | 身份：设备归属 |
| `ara_agent_lite_core::RunRequest.objective` | stable | 本次任务目标 |
| `ara_agent_lite_core::RunRequest.model` | stable | 模型路由：模型 ID |
| `ara_agent_lite_core::RunRequest.endpoint` | stable | 模型路由：端点 |
| `ara_agent_lite_core::RunRequest.token` | stable | 模型路由：内存中的凭据 |
| `ara_agent_lite_core::RunRequest.route` | stable | 模型路由：`model::Route` 的 4 个序列化字段 |
| `ara_agent_lite_core::RunRequest.max_output` | stable | 预算：单次输出上限，默认 4096 |
| `ara_agent_lite_core::RunRequest.cwd` | stable | 路径：工作目录 |
| `ara_agent_lite_core::RunRequest.state_path` | stable | 路径：Session 文件 |
| `ara_agent_lite_core::RunRequest.environment_state` | stable | 路径：运行环境状态目录 |
| `ara_agent_lite_core::RunRequest.global_knowledge` | stable | 路径：覆盖默认全局知识路径 |
| `ara_agent_lite_core::RunRequest.brief` | stable | 附加说明，原样交给 Core |
| `ara_agent_lite_core::RunRequest.max_calls` | stable | 预算：调用上限，默认 12 |
| `ara_agent_lite_core::RunRequest.max_seconds` | stable | 预算：时间上限，默认 120 秒 |
| `ara_agent_lite_core::RunRequest.steering` | stable | 运行中插话缓冲区：`Arc<Mutex<steering::SteeringBuffer>>`；Harness 续写时核对其 Arc 身份 |
| `ara_agent_lite_core::RunRequest.policies` | experimental | P2 的 Run 策略（Completion_Gate、Stuck_Detector、Compaction_Refresh、Length 停止证据），默认全部关闭；B0 基线与 P2 对比完成前不提升，见 DR-P4-3 |

## Core：crate 根重导出

| 项 | 级别 | 说明 |
|---|---|---|
| `ara_agent_lite_core::rpi_agent` | stable | 上游 `rpi-agent` @ `8ec462672d90e62712d96074638efef89436cebe`，见补充规则 4 |
| `ara_agent_lite_core::rpi_ai` | stable | 上游 `rpi-ai` @ 同一 rev，见补充规则 4 |

## Core：其他公共项

下列 stable 项是外部 Host 写出最小集成（构造请求、注册工具、按 Run_Policy 选择生成策略、运行与读取结果）所需的项，以及 stable 字段的类型。

| 项 | 级别 | 说明 |
|---|---|---|
| `ara_agent_lite_core::RunRequest::new` | stable | 唯一受保护的构造方式；默认 Plan、不开本机工具、无 Worker 与快照 |
| `ara_agent_lite_core::RuntimePaths` | stable | 字段 `cwd`、`session_path`、`environment_state` |
| `ara_agent_lite_core::RuntimePaths::new` | stable | 三个路径的构造函数 |
| `ara_agent_lite_core::ToolRegistration` | stable | 字段 `tool`、`allow_in_plan` |
| `ara_agent_lite_core::RunMode` | stable | 变体 `Execute`、`Plan`、`Goal` |
| `ara_agent_lite_core::RunResult` | stable | 字段 `text`、`code`、`error` |
| `ara_agent_lite_core::execute` | stable | `execute(RunRequest, CancellationToken, mpsc::Sender<serde_json::Value>) -> anyhow::Result<RunResult>`，事件为 JSON 值 |
| `ara_agent_lite_core::model::Route` | stable | `route` 字段的类型，字段 `model`、`protocol`、`binding`、`decision` |
| `ara_agent_lite_core::model::ModelHost` | stable | `model_host` 字段的 trait；必需方法 `resolve(Route, &str, &str, &str) -> anyhow::Result<ModelSelection>`。两个默认方法另行列为 stable；`capabilities_source`、`output_ceiling` 按补充规则 5 为 experimental |
| `ara_agent_lite_core::model::ModelHost::generation_policy` | stable | `fn generation_policy(&self, &Route) -> anyhow::Result<GenerationPolicy>`，默认 `ProviderDefault`。每个 Run 在 `resolve` 之后、首次模型请求之前按实际解析的路由调用；必须是纯函数，不做 I/O；返回错误时该 Run 不发模型请求 |
| `ara_agent_lite_core::model::ModelHost::length_recovery_policy` | stable | `fn length_recovery_policy(&self, &Route) -> anyhow::Result<LengthRecoveryPolicy>`，默认 `Stop`。调用时机与纯函数要求同上；`ReplanOnce` 只在 `RunMode::Execute` 下允许，其他模式在模型请求前报错 |
| `ara_agent_lite_core::model::GenerationPolicy` | stable | 变体 `ProviderDefault`（默认）与 `ThinkingTypeDisabled`；serde 名 `provider_default`、`thinking_type_disabled`。`ThinkingTypeDisabled` 要求解析后的协议为 `openai-completions`（其他协议在模型请求前报错），请求中加 `thinking: {"type": "disabled"}`，不证明远端如何执行。方法 `validate_route`、`apply` 为 experimental |
| `ara_agent_lite_core::model::LengthRecoveryPolicy` | stable | 变体 `Stop`（默认）与 `ReplanOnce`；serde 名 `stop`、`replan_once`。`ReplanOnce` 允许闭合的纯文本 Length 响应后再发一次原生请求，不增加输出或调用预算 |
| `ara_agent_lite_core::model::CatalogModelHost` | stable | `RunRequest::new` 默认使用的 ModelHost：单元结构体，实现 `Default` 与 `ModelHost`，`resolve` 读取端点的 `/models` 目录并核对模型与协议。自定义 Host 可以把 `resolve` 委托给它；它对 `capabilities_source` 的覆盖值按 experimental 处理 |
| `ara_agent_lite_core::model::ModelSelection` | stable | `ModelHost::resolve` 的返回类型。只承诺类型名：Host 把 `CatalogModelHost` 的 `resolve` 结果原样返回即为受保护用法；字段 `route`、`client`、`capabilities`、`headers` 按补充规则 5 为 experimental，以字面量构造它不受保护 |
| `ara_agent_lite_core::steering::SteeringBuffer` | stable | `steering` 字段的类型；只承诺类型名与 `Default` 构造，方法按补充规则 5 处理 |
| `ara_agent_lite_core::LiteEvent` | experimental | 需求 28 的版本化事件枚举；`model_result` 与同轮 `text_delta` 的相对顺序不确定（DR-P4-1），不提升 |
| `ara_agent_lite_core::EVENT_SCHEMA_VERSION` | experimental | 事件枚举版本号，当前为 1 |
| `ara_agent_lite_core::GoalState` | experimental | Goal 持久状态 |
| `ara_agent_lite_core::fresh_id` | experimental | 随机 ID 工具函数 |
| `ara_agent_lite_core::agent::*` | experimental | 经 crate 根重导出的项以根路径计级；其余如 `InterruptedRun`、`ToolCallReconciliation`、`BUILTIN_TOOL_NAMES` |
| `ara_agent_lite_core::model::*` | experimental | 上面单列为 stable 的项以外的部分，如 `resolve_catalog`、`resolve_catalog_with_headers`、`validate_endpoint`、`plaintext_loopback` |
| `ara_agent_lite_core::steering::*` | experimental | `SteeringNote` 及 `SteeringBuffer` 的方法 |
| `ara_agent_lite_core::execution::*` | experimental | Goal 与 checklist 工具 |
| `ara_agent_lite_core::events::*` | experimental | P4 新增：`LiteEvent` 各变体、`EventDecodeError` |
| `ara_agent_lite_core::session_fork::*` | experimental | P0 新增：`fork_session`、`ForkReceipt` |
| `ara_agent_lite_core::effect_status::*` | experimental | P3 新增：结构化未知副作用判定 |
| `ara_agent_lite_core::stuck_detector::*` | experimental | P2 新增：`StuckRule`、`StuckPolicy` |
| `ara_agent_lite_core::completion_gate::*` | experimental | P2 新增：`CompletionGate`、`GateRequest`、`GateVerdict`、`Registration` |
| `ara_agent_lite_core::compaction_refresh::*` | experimental | P2 新增：`RefreshPolicy`、`FileToolKind` |
| `ara_agent_lite_core::output_limit::*` | experimental | P2 新增：`EffectiveOutputLimit`、`OutputLimitSource` |
| `ara_agent_lite_core::run_policies::*` | experimental | P2 新增：`RunPolicies` |
| `ara_agent_lite_core::termination::*` | experimental | `ClosedBudgetTermination`、`StopEvidence`（P2 新增） |
| `ara_agent_lite_core::goal_runtime::*` | experimental | Goal 租约 |
| `ara_agent_lite_core::context_memory::*` | experimental | 上下文、摘要与外部快照 |
| `ara_agent_lite_core::session_journal::*` | experimental | Session Journal 格式与读取 |
| `ara_agent_lite_core::model_caps::*` | experimental | 模型能力声明 |
| `ara_agent_lite_core::model_telemetry::*` | experimental | 调用遥测 |
| `ara_agent_lite_core::local_tools::*` | experimental | 内置本机工具 |
| `ara_agent_lite_core::analysis_tools::*` | experimental | 源码符号与二进制分析工具 |
| `ara_agent_lite_core::script_tools::*` | experimental | 脚本工具 |
| `ara_agent_lite_core::web_tools::*` | experimental | 联网工具 |
| `ara_agent_lite_core::file_discovery::*` | experimental | 文件查找工具 |
| `ara_agent_lite_core::managed_jobs::*` | experimental | 后台 Job |
| `ara_agent_lite_core::terminal_process::*` | experimental | 终端进程 |
| `ara_agent_lite_core::terminal_emulator::*` | experimental | 终端仿真 |
| `ara_agent_lite_core::system_capabilities::*` | experimental | 系统信息工具 |
| `ara_agent_lite_core::environment_library::*` | experimental | 知识与 Skills 根目录 |
| `ara_agent_lite_core::environment_tools::*` | experimental | 知识与 Skills 工具 |
| `ara_agent_lite_core::skill_library::*` | experimental | Skills 库 |
| `ara_agent_lite_core::experience::*` | experimental | 经验记录 |
| `ara_agent_lite_core::project_rules::*` | experimental | 项目规则读取 |

## Harness_Lite

| 项 | 级别 | 说明 |
|---|---|---|
| `ara_agent_lite_harness::start` | stable | `start(TaskSpec, RunRequest, Arc<dyn HarnessHost>) -> anyhow::Result<TaskHandle>` |
| `ara_agent_lite_harness::TaskHandle` | stable | 方法 `cancel`、`subscribe`、`wait` |
| `ara_agent_lite_harness::TaskSpec` | stable | 8 个字段：`task_id`、`criteria_revision`、`authorization_revision`、`kind`、`evidence_dir`、`max_runs`、`max_calls`、`deadline_unix_ms` |
| `ara_agent_lite_harness::TaskKind` | stable | 变体 `Coding`、`Reasoning`、`ReverseAnalysis`，只作提示，不授予权限 |
| `ara_agent_lite_harness::HarnessHost` | stable | 必需方法 `verify`、`prepare_continuation`；其余默认方法属于 experimental 的停止复核与 resume 能力 |
| `ara_agent_lite_harness::Candidate` | stable | 字段 `basis`、`output`、`goal` |
| `ara_agent_lite_harness::ReviewBasis` | stable | 审查依据，Host 原样回填到回执 |
| `ara_agent_lite_harness::ReviewReceipt` | stable | 字段 `basis`、`committed`、`settlement`、`verdict`、`evidence` |
| `ara_agent_lite_harness::Settlement` | stable | 变体 `Settled`、`Unknown` |
| `ara_agent_lite_harness::Verdict` | stable | 变体 `Verified`、`NeedsChanges { feedback }`、`Unknown` |
| `ara_agent_lite_harness::Continuation` | stable | 字段 `previous`、`receipt`、`next_run_number`、`remaining_calls`、`deadline_unix_ms` |
| `ara_agent_lite_harness::TaskOutcome` | stable | 除下面两个字段外的全部字段；`task_accepted` 恒为 false |
| `ara_agent_lite_harness::TaskOutcome.unknown_effects` | experimental | P3 新增的未知副作用记录 |
| `ara_agent_lite_harness::TaskOutcome.budget_stop` | experimental | P3 新增的摘要预算停止原因 |
| `ara_agent_lite_harness::TaskStop` | stable | 7 个停止原因 |
| `ara_agent_lite_harness::FencePresence` | stable | 变体 `Present`、`Released`、`Unknown` |
| `ara_agent_lite_harness::EventEnvelope` | stable | 字段 `task_id`、`run_id`、`seq`、`event`（JSON 值） |
| `ara_agent_lite_harness::core` | experimental | Core 的整体重导出；外部 Host 应直接依赖 Core，经此路径引用 Core 项会被消费者检查报告 |
| `ara_agent_lite_harness::start_with_options` | experimental | 带 `TaskOptions` 的启动入口 |
| `ara_agent_lite_harness::TaskOptions` | experimental | 停止复核开关 |
| `ara_agent_lite_harness::BudgetStopReviewPolicy` | experimental | 闭合预算停止的复核策略 |
| `ara_agent_lite_harness::LengthStopReviewPolicy` | experimental | P2 新增的 Length 停止复核策略 |
| `ara_agent_lite_harness::BudgetStopSnapshot` | experimental | 预算停止快照 |
| `ara_agent_lite_harness::BudgetStopSnapshotReference` | experimental | 预算停止快照引用 |
| `ara_agent_lite_harness::BudgetStopReviewRecord` | experimental | 预算停止复核记录 |
| `ara_agent_lite_harness::BudgetStopContinuation` | experimental | 预算停止续写 |
| `ara_agent_lite_harness::StopSnapshot` | experimental | P2 新增的 Length／NoProgress 停止快照 |
| `ara_agent_lite_harness::StopSnapshotReference` | experimental | 停止快照引用 |
| `ara_agent_lite_harness::StopContinuation` | experimental | 停止复核续写 |
| `ara_agent_lite_harness::TruncatedMessageRef` | experimental | 被截断消息的 Journal 绑定 |
| `ara_agent_lite_harness::resume` | experimental | P1 新增：恢复被 fence 的 Task |
| `ara_agent_lite_harness::resume_checked` | experimental | P1 新增：带 fence 哈希与上限检查的 resume |
| `ara_agent_lite_harness::ResumeReview` | experimental | resume 时交给 Host 的现场 |
| `ara_agent_lite_harness::ResumeReceipt` | experimental | Host 的 resume 结算回执 |
| `ara_agent_lite_harness::ResumeChecks` | experimental | `resume_checked` 的检查项 |
| `ara_agent_lite_harness::ResumeRefusal` | experimental | resume 拒绝原因 |
| `ara_agent_lite_harness::InterruptedRun` | experimental | Core 类型的重导出 |
| `ara_agent_lite_harness::OpenToolCall` | experimental | Core 类型的重导出 |
| `ara_agent_lite_harness::ToolCallReconciliation` | experimental | Core 类型的重导出 |
| `ara_agent_lite_harness::ToolCallSettlement` | experimental | Core 类型的重导出 |
| `ara_agent_lite_harness::UnknownEffectRecord` | experimental | P3 新增 |
| `ara_agent_lite_harness::EffectReceiptRef` | experimental | P3 新增 |
| `ara_agent_lite_harness::EffectEvidence` | experimental | P3 新增 |
| `ara_agent_lite_harness::HostReceiptRef` | experimental | P3 新增 |
| `ara_agent_lite_harness::BudgetStopDetail` | experimental | P3 新增 |
| `ara_agent_lite_harness::BudgetStopTrigger` | experimental | P3 新增 |
| `ara_agent_lite_harness::budget_snapshot_path` | experimental | 证据文件位置 |
| `ara_agent_lite_harness::budget_review_index_path` | experimental | 证据文件位置 |
| `ara_agent_lite_harness::stop_snapshot_path` | experimental | 证据文件位置 |
| `ara_agent_lite_harness::stop_review_index_path` | experimental | 证据文件位置 |

## JSONL_Sidecar 与 MCP 工具来源

| 项 | 级别 | 说明 |
|---|---|---|
| `ara_agent_lite_jsonl::main_entry` | experimental | sidecar 进程入口；跨语言接口是 v1 协议帧，不是 Rust API |
| `ara_agent_lite_mcp::*` | experimental | 需求 42 新增的 crate（`McpSession`、`McpConfig`、`McpOptions` 等），版本 0.1.0 |

## 项数

- Core：`RunRequest` 31 个字段加 2 个 `rpi_*` 重导出，共 33 项（需求 29.1）；stable 共 43 项，其中字段 25 项、重导出 2 项、其他公共项 16 项。
- Harness_Lite：stable 共 15 项。
- 两个 `stability_surface` 测试分别核对 stable 集合与测试实际使用的集合相等。

## 未覆盖

- 删除一个 stable 项后编译失败的演示（需求 29.5，任务 35.3）由 `lite-eval gate` 在临时副本上执行，尚未接入。
- `policies`、事件枚举、resume、停止复核、未知副作用记录与 MCP crate 仍为 experimental，提升条件见 DR-P4-3。
