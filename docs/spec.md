# ARA Agent Core 跨产品复用规格

状态：规划稿；本轮已补 steering execute 与 archived-summary fork 的受控证据，改动仍未提交或推送。
目标工作树：`E:\\repos\\ara-github`，分支 `dev`，当前为 dirty worktree；本规格引用调查到的源码、支持矩阵、本轮证据和已有验收规则。

## 1. 目标与完成标准

ARA 需要一套可供 SpinMind 核磁实验助手、代码 Review Agent、远程协助 Agent 复用的公共 Agent 内核。三个产品共享 Agent loop、Session、上下文、工具契约、事件、计划、回执、取消、恢复和 Memory provenance；产品身份、权限、设备与网络连接、模型凭据、UI/RPC/MCP 传输和业务验收保持在 Host 或工具适配器。

本规格完成的可观察结果：

1. 每个缺口都有源码和测试依据，并明确属于 Core、Harness、独立 Skill/Memory crate、Sidecar 或产品 Host。
2. `ara-agent-lite-core`、`ara-agent-lite-harness`、JSONL Sidecar 和 MCP 适配器有稳定的依赖方向与版本化边界。
3. 跨 Run 的 Task 验收、恢复、未知副作用、预算和证据不会被 Core 的单 Run loop 取代。
4. Skill 与 Memory 先形成可审计、可撤回、可隔离的最小闭环，不把一次 Run 自动升级为“自我进化”。
5. 三类 Host 都能用同一 Core/Harness 完成一个有界任务；真实模型、真实 MCP、真实 QSpin V3/设备属于独立验收层，不由离线测试替代。

## 2. 事实矩阵

依据：`crates/ara-agent-lite-core/docs/support-matrix.md`、同目录 `support-matrix.json`、Core/Harness/JSONL/MCP 源码与测试。支持矩阵当前依据 `dev` 上的 Lite 源码及工作树测试/文档，矩阵也明确指出部分条目仅核对了测试存在性，需以 delivered-code gate 重新执行。

| 缺口 | 当前事实 | 分类 | 关键证据 |
|---|---|---|---|
| MCP 能力部分支持 | `ara-agent-lite-mcp` 已把 stdio、Streamable HTTP 工具包装为 `AgentTool`；JSONL 有 `mcp_servers` 与 `mcp_status`。SSE/WS/OAuth/headersHelper、prompts/sampling/listChanged、Unix 进程运行路径仍有限制。 | 源码存在且有测试；能力仍部分支持；适配器/Host 责任 | support-matrix.md:26；`crates/ara-agent-lite-mcp/tests/{stdio.rs,http.rs,harness_mcp.rs}`；`jsonl/tests/test_mcp_servers.py` |
| Session fork 部分支持 | `session_fork::fork_session` 读取 Journal 的完整原始前缀，支持从摘要归档前缀（`active_base > 0`）重建 active_base=0 的两个 fork，并继续拒绝 pending Run、cwd 不匹配、非完整 exchange 等边界；目标前缀遵守 512 条和 8 MiB 检查点上限。 | 已有受控真实 compaction + fork 证据及两个超限拒绝测试；真实 Host/产品验收与跨进程并发写入仍未覆盖 | support-matrix.md:35；`src/session_fork.rs:37-117`；`tests/session_fork.rs` |
| Steering 部分支持 | Core 有 `SteeringBuffer` 与回执元数据；RunRequest/JSONL 有 steering 字段；当前已补受控 `execute` 端到端证据，但仍没有真实 sidecar/Host 的实时到达验收。 | 部分存在且有受控测试；真实 Host 观察未验收 | support-matrix.md:37；`src/steering.rs`；`src/runtime.rs:244-329`；`tests/steering_execute.rs` |
| Remote compaction 未支持 | Lite 只有自身摘要调用、本地 context/refresh；没有 provider 侧远程压缩契约、能力声明和失败/取消回执。 | 尚未实现；协议边界可由 Provider/Host 适配器承担 | support-matrix.md:46；`src/context_memory.rs`、`src/compaction_refresh.rs` |
| Per-call Host hooks 部分支持 | `RunRequest.call_hooks` 通过拥有数据的 DTO/trait 提供工具调用前的准入和调用后的观察；hook 错误会取消当前 Run，默认 `None` 保持旧行为。 | 已有受控 before deny、after success/error 和 hook failure 证据；异步 Host hook、取消/超时后的结算和真实产品 Host 验收仍未覆盖 | support-matrix.md:25；`src/call_hooks.rs`；`tests/call_hooks.rs` |
| JSONL 跨进程 resume 部分支持 | `resume`、`reconcile_request/reply`、原 Session 继续和未知效果保护已有实现与测试；授权改变、预算或单 Run 上限变大时只返回通用拒绝，Host 无法知道具体拒绝类别。 | 部分存在且有测试；Sidecar/Harness 契约未收敛 | support-matrix.md:75；`src/lib.rs:159-202`；`tests/test_jsonl_resume.py` |
| Harness stable API 部分支持 | `stability_surface`、消费者扫描和 stable 最小入口已有；事件、resume、停止复核、未知副作用记录、MCP 仍是 experimental；删除 stable 项后编译失败演示尚未接入正式门禁。 | 源码存在且有部分测试；独立稳定性验收未完成 | support-matrix.md:66；`docs/stability.md`；Core/Harness `tests/stability_surface.rs` |
| Harness delivery gate 部分支持 | `lite-eval` 有矩阵执行器属性测试，依赖方向已检查；尚无在 delivered commit 上完整运行 `lite-eval gate` 并保存结果的独立证据。 | 源码部分存在；尚无独立验收 | support-matrix.md:68；`tools/lite-eval/tests/gate_props.rs` |
| Runtime Skill Loader | Core 已有受限 `skill_read/create/install/verify/remove`、目录 catalog、SHA-256、来源元数据和路径/预算检查，且有 `skill_library_tests`。但没有独立 Loader crate、稳定 manifest/registry/version/source/permission/activation/rollback 合同。贡献者 `.ara/skills` 流程不能当运行时 Loader。 | 部分存在且有测试；需独立 crate 与 Host 策略 | `src/skill_library.rs:30-44,416-475,498-744`；`src/skill_library_tests.rs`；`.ara/skills/INDEX.md` |
| Memory provenance | `context_memory` 已有 Reference/Source/Memory、来源哈希、Run/experience/cloud stamp、store/projection/撤销等；`experience` 有 Evidence/Receipt 配对和验证测试。缺跨产品隔离、Host-owned persistence/consent/policy、独立稳定接口和真实跨进程 Host 验收。 | 源码存在且有测试；跨产品治理未验收 | `src/context_memory.rs:145-177,193-746,874-1045`；`src/experience.rs:18-504`；相关 Core tests |

“代码存在”只说明源码边界和受控测试存在，不代表真实模型、真实 MCP、真实设备或产品交付已经验收。

## 3. 分层设计

### 3.1 `ara-agent-lite-core`

Core 只负责一次 Agent Run 及其原生 Session 语义：

- 模型请求/响应循环、工具契约、调用 ID、工具回执、取消和错误传播。
- Session/Journal、上下文窗口、摘要、压缩刷新、事件和来源标记。
- 单 Run 调用预算、计划/Goal 原语、steering/fork、未完成调用的 fence 与重放来源。
- Memory provenance 原语：短期上下文、来源引用、哈希、状态和审计事件；不保存产品租户状态。
- 稳定的 Host hook/event 边界：只传结构化观察与取消信号，不做业务验收。
- 可选工具能力通过 `AgentTool` 注入；Core 不依赖 MCP、QSpin SDK、Gateway、UI 或产品 crate。

Core 不负责跨 Run Task 总预算、Task 接受、业务结果、模型凭据、设备状态、租户隔离、MCP 连接生命周期或远程压缩实现。

### 3.2 `ara-agent-lite-harness`

Harness 负责把多个 Run 组成一个可审查的 Task：

- Task/Run/Session/Job 身份、跨 Run 预算、截止时间、授权版本和 Session 独占。
- Candidate、ReviewReceipt、NeedsChanges/Verified/Unknown、续作和恢复准入。
- fence、未知副作用、Host 操作回执、证据目录和不可重放规则。
- `resume_checked`、跨进程恢复拒绝类别、delivery gate、stable API 检查和审查收据。

Harness 不实现 Agent loop，不拥有产品业务验收，不替换失败 resume 为新对话，不自动重放未知效果。

### 3.3 JSONL Sidecar 与 MCP 适配器

- JSONL Sidecar 只做跨进程帧、关联校验、取消等待、回执转发和兼容版本。
- `ara-agent-lite-mcp` 只做 MCP 配置解析、连接、工具 schema/结果映射、超时与未知效果标记。
- 两者都不持有产品身份、业务数据库或 QSpin 状态；这些由 Host 提供。
- 真实协议验收需使用真实 sidecar 进程和受控 MCP server；fake fixture 只能证明帧和故障语义。

### 3.4 产品 Host

Host 负责：

- 用户、租户、设备、项目和 Session key 身份；权限、审批、凭据和存储位置。
- 模型路由、模型能力声明、Gateway/Relay、限额和实际成本记录。
- 业务工具及其执行端校验、取消、超时、操作回执和效果结算。
- UI/RPC/MCP 连接、日志/截图/数据库读回、脱敏和业务验收。
- 产品 Memory/Skill 的租户隔离、同意、策略、发布和回滚。

SpinMind Host 负责 QSpin V3 SDK、实验/样品/设备授权、采集状态和真实设备验收。代码 Review Host 负责 Git 工作树、编译/测试/静态分析和补丁验收。远程协助 Host 负责 SSH/Starferry、现场授权、远端命令收据和 UI/API 联合验收。上述状态不得进入 Core。

### 3.5 独立 Skill 与 Memory crate

新增两个窄职责 crate，Core 先通过稳定 capability trait 消费：

- `ara-agent-lite-skill`：manifest、registry、发现、版本、来源、权限、激活/停用、审计、显式 verify、晋升和回滚。第一版只允许显式 Host 授权和人工/Host 审核后的晋升。
- `ara-agent-lite-memory`：短期上下文投影、持久记忆记录、来源哈希、证据引用、撤销、租户/产品命名空间和审计。Host 提供存储和同意策略，Core 不直接写产品数据库。

现有 Core `skill_library`、`context_memory`、`experience` 作为迁移来源和兼容实现，不在本规划阶段直接删除或扩大稳定级别。

## 4. 编号需求与验收条件

### R1. Core 公共边界

Core 不导入 QSpin、SpinMind、代码 Review、远程协助、Gateway、UI/RPC/MCP 连接或产品状态。`cargo metadata` 依赖图、源码扫描和树外消费者编译都证明该方向。

### R2. Run/Session/Task 分离

Core 的 Run/Session 状态与 Harness 的 Task/Job 状态有独立 ID、预算和终态。一个 Run 成功不能自动把 Task 标记 accepted；失败 resume 不得创建新 Session 或重放未知效果。

### R3. 端到端 steering

Host 能在模型 Run 等待下一个模型回合前注入带来源的 steering；事件、Journal、取消和迟到 steering 都有明确状态。受控 fake、真实 sidecar 和一个真实 Host 场景分别验证。

### R4. Session fork

已归档摘要前缀仍能按可验证的 Journal proof 分叉，或明确记录该限制属于版本边界。源 Session 与 fork 互不写入；跨进程恢复、失败清理和权限拒绝均有测试。

### R5. Per-call hooks

先提供工具调用级 experimental DTO/trait：before 返回 typed allow/deny，after 接收拥有数据的结果快照；hook 错误取消当前 Run。Core 不暴露 `rpi_agent` 的内部生命周期类型，Host 负责业务观察、审批和回执。无 hook 时默认路径字节和行为保持不变。模型调用、异步 hook、cancel/timeout settlement 和真实 Host 验收继续作为后续边界。

### R6. Compaction

本地 summary/refresh 保持现有语义。Remote compaction 作为 Host/Provider capability，不进入 Core 的默认路径。最小草案包含：能力声明（route、model、protocol、最大输入/输出字节）、请求（source prefix proof、active tail、deadline、cancellation）、结果（summary、covered prefix proof、provider receipt、usage 或 unknown）和失败类别（unavailable、rejected、timeout、canceled、partial）。能力未声明时，Core 继续本地压缩或返回类型化停止；事件不得把本地结果标成远程压缩。

### R7. Resume

Harness 和 JSONL 对每种拒绝提供稳定类别与可审计 receipt，至少区分 authority changed、budget enlarged、run limit enlarged、fence mismatch、settlement incomplete、evidence missing。恢复继续原 Session/Task，不执行未知调用。

### R8. Stable API 与 delivery gate

稳定接口有版本清单、编译消费者、删除演示和变更规则。`lite-eval gate` 在 delivered snapshot 上运行矩阵、fmt、clippy、Rust/Python/Node/MCP/consumer 检查；失败、跳过、零执行、缺失测试或无退出码都不通过。

### R9. Runtime Skill Loader

Skill manifest 至少包含 name、version、source、content hash、scope、required permission、status 和 rollback target。发现、安装、激活、verify、晋升、停用和回滚都有事件与审计记录。未经 Host 明确授权，Skill 不获得工具权限；一次成功 Run 不自动晋升 Skill。

### R10. Memory provenance

Memory 记录 namespace/product/tenant、source refs、content hash、creator Run/Task、consent/policy、status、撤销和审计事件。短期上下文可进入 Core；持久写入必须经 Host/Memory crate。跨 SpinMind、Code Review、Remote Host 的读取互相隔离。

### R11. Host 复用验收

同一 Core/Harness 版本完成三种有界任务：

- SpinMind：读取已授权实验上下文，调用模拟 QSpin V3 工具，生成实验计划，Host 检查设备/样品约束并保存 receipt。
- Code Review：读取指定 Git 工作树，调用编译/测试/静态分析工具，产生补丁候选，Host 复核 diff 和测试后决定继续或停止。
- Remote Assist：通过已授权 SSH/Starferry 工具读取现场状态，执行一项可回滚动作，Host 核对远端回执并处理断连/未知效果。

三者必须证明身份、权限、工具、Session/Task、Memory namespace 和失败路径相互隔离。

### R12. 交付证据

每个实现点必须在 delivered code 上有源码范围、测试命令、实际产物/Journal/receipt、失败路径、独立审查结果和未覆盖项。真实模型、真实 MCP、真实 QSpin/远端设备是单独证据层，不能用 fake 或 HTTP 200 代替。

## 5. Lite Core 作为 SpinMind v1 基础的判断

可以作为基础，但当前版本不能直接宣称 SpinMind v1 已就绪。v1 开始前至少补齐：

1. Host 授权、QSpin V3 工具适配、设备/样品状态回执和业务验收。
2. steering 的 execute 端到端语义和 UI/RPC 观察。
3. Harness resume/fence/unknown-effect 的稳定拒绝类别及交付门禁。
4. Memory/Skill 的产品 namespace、同意和审计边界。
5. 一次真实 Host 进程验收，以及受控 MCP 连接验收。

Remote compaction 不是 SpinMind v1 的前置阻断项，可先由 Provider/Host 能力声明为 unavailable，使用本地压缩并保留类型化结果。真实设备采集、射频/固件和实验正确性必须在 SpinMind Host 证据中单独验收。

## 6. 不在本规格范围

- QSpin 或 SpinMind 业务模型进入 Core。
- 自动自我进化、无审计的 Skill 学习或自动提升权限。
- 把工具白名单当作 OS 沙箱。
- Full Core/OMP 全量 parity、产品发布、部署和生产迁移。
- 未经单独授权的提交、推送、真实设备操作或真实模型调用。

## 7. 事实来源与未验证项

主要来源：

- `crates/ara-agent-lite-core/docs/support-matrix.md:25-26,35,37,46,66,68,75,78,91-93`
- `crates/ara-agent-lite-core/docs/knowledge/host-contracts.md:21-77,387-395`
- `crates/ara-agent-lite-core/docs/stability.md:15-24,102-128,167-204`
- `crates/ara-agent-lite-core/src/{session_fork.rs,steering.rs,context_memory.rs,experience.rs,skill_library.rs}`
- `crates/ara-agent-lite-harness/{README.md,src/runner.rs,tests}`
- `crates/ara-agent-lite-jsonl/{README.md,src/lib.rs,src/runtime.rs,tests}`
- `crates/ara-agent-lite-mcp/README.md` 与 `tests/{stdio.rs,http.rs,harness_mcp.rs}`

本轮未验证：真实 SpinMind/QSpin V3 设备、真实 MCP server、真实模型上的跨 Run 任务、远程压缩 provider、三类 Host 的产品 UI/RPC、跨产品 Memory 隔离、发布和部署门禁。上述内容在 `docs/tasks.md` 中作为后续任务。
