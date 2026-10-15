# 目标模式、长任务与上下文：开源实现调研

研究日期：2026-10-15。结论是**保留小型 Rust Node，补齐持久任务执行层**，
优先借鉴 Pi durable、Codex Rust Goal 和 LangGraph 的具体机制。
不是更换框架，也不是把一个 Run 的超时放大成数小时。本页是经交叉审查的
设计建议，描述研究开始时的基线；上游测试仅阅读，未执行。后续实现与验收
单独记录在[长任务交付证据](../evidence/long-task-runtime-20261016.md)，不能把
研究结论直接当作新增生产或模型验收。

## 本地事实：已有能力与真正缺口

基线 `0ddc39ef80607f6c404dc079f4a63fd3c56c0a80`，研究开始时 Node 子目录
干净。根 OMP/Core 的无关工作不属于本次范围。以下已回源，不把已有功能
重新当作待开发：

| 事实 | 当前源码 | 对长任务的含义 |
|---|---|---|
| 目标标准、步骤、revision、明确 accept/pause/resume 已有 | [execution.rs](../../src/execution.rs):86–129；[agent.rs](../../src/agent.rs):365–398,863–882 | 正常 Run 进入 awaiting_review；目标进度持久化不等于自动取得下一 Run |
| 每条 MessageEnd 重写整个 checkpoint；256 条消息入口限制、16 MiB 文件限制 | agent.rs:52–115,167–188,420 | 模型窗口再大也不能绕过 Host 历史上限；直接删限制会增加写入、内存与恢复成本 |
| 原始来源、派生摘要、工具配对、撤销 watermark、pending fence 已有 | agent.rs:87–90,119–151；[context_memory.rs](../../src/context_memory.rs):311–331,450–589 | 应扩展存储和历史检索，保留已有不盲目重放的保护 |
| 动态模型容量与完整请求预算已实现 | [model_caps.rs](../../src/model_caps.rs):60–108；agent.rs:469–476,591–634 | 不能重复建议“去掉固定192 KiB”作为新增；当前仍是字节估计、有资源上限、文本 Host |
| reference_read/search 读取用户来源，experience_search 读取已采纳经验 | context_memory.rs:605–712 | 压缩后旧工具细节虽在 raw history，尚无对应的模型按需历史收据检索入口 |
| 活跃 Run/跟进队列在内存，连接结束取消并等待清理 | [transport.rs](../../src/transport.rs):24–32,61–73,286–325,337–343 | 当前不具备网页关闭/Node重启后自动续跑的任务合同 |
| 每个 Run 结束发送 model-grant-end，新 Run 单独请求授权 | transport.rs:145–147,475 | 旧 token 未到 TTL 也不代表授权仍有效；调度权不能代替授权权 |
| 前台进程工具清理子孙进程 | [local_tools.rs](../../src/local_tools.rs):868–940 | 不能删除清理逻辑来假装后台长进程；长编译需要独立 Job 合同 |

12 次调用/120 秒是上一轮实际安装参数，源码默认是24/600
（[config.rs](../../src/config.rs):23–27）。实际 Run 还受 grant/decision
有效期约束（transport.rs:213–235）。当前并非硬编码“目标只能两分钟”。

## 固定的开源来源

各 SHA 取自本轮 GitHub 官方仓库，文件原文与 SHA256 保留在本地研究缓存；
[来源清单](../evidence/long-task-research-sources-20261015.json)随本页保存。
这些是源码快照，不是已安装版本或经我们验证的性能排名。

| 上游 / 许可证 | 固定 SHA | 已核验的机制与适配取舍 |
|---|---|---|
| [Pi](https://github.com/earendil-works/pi/tree/eddebaeebf8168dfc31d6712b7a021e4df3b4e6a) / MIT | `eddebaeebf8168dfc31d6712b7a021e4df3b4e6a` | 独立 pi-durable 已有 scheduler、tool/generation phase、存储、压缩及恢复测试源码；不能据此说整个 Pi CLI 已迁移或此版本已生产成熟 |
| [Codex](https://github.com/openai/codex/tree/3ee47c10513f1bb57d869ea650aba28638e3d11c) / Apache-2.0 | `3ee47c10513f1bb57d869ea650aba28638e3d11c` | Rust Goal 在 Run 外管理目标、预算和 idle continuation；最直接的 Rust 对照，但依赖其 state/extension/thread runtime，不适合整包搬入 |
| [LangGraph](https://github.com/langchain-ai/langgraph/tree/5eb60aef6f3837a1fb632a8abc6cf8e8da81785b) / MIT | `5eb60aef6f3837a1fb632a8abc6cf8e8da81785b` | 独立 task writes/checkpoint，恢复复用成功子任务；借合同，不引入 Python runtime |
| [OpenHands SDK](https://github.com/OpenHands/software-agent-sdk/tree/f5f849f5ab51502cdfff97870f29de95307882a4) / MIT | `f5f849f5ab51502cdfff97870f29de95307882a4` | EventLog 与模型 View 分离；condenser 保留协议批次边界。Goal judge 外循环不等于持久调度或人工验收 |
| [OpenCode](https://github.com/anomalyco/opencode/tree/81200db2a3a86fc72a60dff996557f59865a6f24) / MIT | `81200db2a3a86fc72a60dff996557f59865a6f24` | restart attempts 持久计数、恢复通知、结构化 summary；源码明确 at-least-once，不保证外部效果仅一次 |
| [Anthropic autonomous-coding 示例](https://github.com/anthropics/claude-quickstarts/tree/9ec32b91df50b0d4906cae64b13c6298055ff40d/autonomous-coding) / MIT | `9ec32b91df50b0d4906cae64b13c6298055ff40d` | initializer、feature_list、进度文件、每片完成一个功能。示例错误后开启 fresh client 重试，不能直接移植成我们的恢复策略 |
| [RLM](https://github.com/alexzhang13/rlm/tree/d04208afbad29ca675ab13478c40ee8bebc84bfe) / MIT | `d04208afbad29ca675ab13478c40ee8bebc84bfe` | 长输入作为外部数据，按需程序化切片并子调用；借检索思想，暂不引入常驻 Python REPL/递归模型调度 |

Claude Code CLI 官方仓库的
[许可证](https://github.com/anthropics/claude-code/blob/main/LICENSE.md)
是保留权利、按商业条款使用；不能将其误称为可自由移植的开源内核。
本轮实际读取的是官方 MIT 长任务示例和公开工程文章。

## 最值得移植的实现细节

### 1. 目标在模型 Run 之外持续存在

Codex 的 [GoalRuntimeHandle::continue_if_idle](https://github.com/openai/codex/blob/3ee47c10513f1bb57d869ea650aba28638e3d11c/codex-rs/ext/goal/src/runtime.rs#L436)
先读持久目标/状态，持 permit 防止并发修改，然后通过 start_turn_if_idle
启动续轮。[extension.rs](https://github.com/openai/codex/blob/3ee47c10513f1bb57d869ea650aba28638e3d11c/codex-rs/ext/goal/src/extension.rs#L165)
把 resume/idle/turn-stop/abort/usage 连接起来；
[SQL](https://github.com/openai/codex/blob/3ee47c10513f1bb57d869ea650aba28638e3d11c/codex-rs/state/goals_migrations/0001_thread_goals.sql)
保存目标状态、tokens_used、time_used_seconds 等。
借鉴“目标外循环+持久预算+空闲派发”，而不是只复制 continuation prompt。
Codex 可由模型更新 complete；本产品保留“提出完成→证据核验→人工 accept”的
已发布边界。其缓存 token 计费公式也不能直接套到未知用量的供应商。

### 2. 恢复点记录 phase 与意图，不自动重跑未知脚本

Pi [ToolTask](https://github.com/earendil-works/pi/blob/eddebaeebf8168dfc31d6712b7a021e4df3b4e6a/packages/durable/src/harness/tool.ts#L164)
先提交最终参数与 execute phase，默认 replay=unsafe；恢复时要求存储策略和
当前工具策略都为 safe，否则报告 interrupted/可能部分执行。其 generation
另存 prepare/request/retry/poll/tools 的阶段信息。
我们保留 pending fence，将其从整个 Session 的粗粒度阻断扩展为可审的
操作收据和显式恢复，不把“未知”改写为“未执行”。

LangGraph 的
[PregelRunner.commit](https://github.com/langchain-ai/langgraph/blob/5eb60aef6f3837a1fb632a8abc6cf8e8da81785b/libs/langgraph/langgraph/pregel/_runner.py#L605)
在 task 返回后保存 writes；成功 sibling 可以复用，但“外部效果成功、收据尚未
保存”仍有重复窗口。幂等存储不等于幂等工具，sync checkpoint 也不消除此窗口。
只读重放可能看到新文件内容，必须标为新观察并校验来源，不能冒充旧收据。

### 3. 历史存储与模型上下文分开，大输出留可检索原件

Pi [shell 输出](https://github.com/earendil-works/pi/blob/eddebaeebf8168dfc31d6712b7a021e4df3b4e6a/packages/durable/src/tools/bash.ts#L87)
与 execution env 支持保留窗口和溢出文件；OpenHands 的
[Condensation](https://github.com/OpenHands/software-agent-sdk/blob/f5f849f5ab51502cdfff97870f29de95307882a4/openhands-sdk/openhands/sdk/event/condenser.py#L11)
只转换模型 View，不据此删除 EventLog。
建议原 stdout/stderr/工具结果存为受控 artifact，模型获得有限摘录、hash、大小、
截断标记和读取范围；复用现有 read/search，不重建一套文件工具。
临时文件路径不等于长期可恢复产物，需规定所有权、保留期限和磁盘配额。
旧 raw history 保留的是当时实际收到的工具回执；如果工具当时已经截断输出，
它不能恢复从未保存的中段 stdout。完整输出须在执行时留存，不能靠后续摘要补造。

### 4. 压缩是可验证投影，不是覆盖原始事实

Pi [selectCut/summaryText](https://github.com/earendil-works/pi/blob/eddebaeebf8168dfc31d6712b7a021e4df3b4e6a/packages/durable/src/harness/compaction.ts#L262)
保持 tool-call/result 边界，拒绝 length 截断、空或携工具调用的摘要。
Codex [compact.rs](https://github.com/openai/codex/blob/3ee47c10513f1bb57d869ea650aba28638e3d11c/codex-rs/core/src/compact.rs#L393)
有 replacement history 与失败处理；远端专用 compact API 不是我们 DeepSeek
链路的可复用依赖。当前 Node 已有摘要来源 hash/水位和闭合旧轮压缩，后续
重点是把摘要映射到可检索 journal 范围，而非再追加一段“请记住所有内容”。

### 5. 长任务交接与模型策略不要写死

Anthropic [2025长任务方法](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
借 feature list、进度产物和逐项验证连接多个上下文窗口；示例的
[agent.py](https://github.com/anthropics/claude-quickstarts/blob/9ec32b91df50b0d4906cae64b13c6298055ff40d/autonomous-coding/agent.py#L146)
不是持久恢复队列，也没有可直接借用的外部效果去重。
[2026 harness研究](https://www.anthropic.com/engineering/harness-design-long-running-apps)
讨论 planner/generator/evaluator；
[后续工程文章](https://www.anthropic.com/engineering/managed-agents)
又指出为某模型添加的强制 reset 对另一个模型可能变成额外负担。
因此不默认每片新建会话、不默认每步再问一个 judge；只有测得收益才引入。

[RLM论文v3](https://arxiv.org/abs/2512.24601v3)的外部上下文切片可启发历史与
日志检索，但论文长文任务效果不证明我们的系统排障效果。源码中的 persistent
REPL 也不等于跨进程持久 Session。该方向列实验项，不承诺更省 token。

## 对本项目的最小方案与实施次序

保持不动：独立 Rust EXE、固定 rpi rev、native Session 身份、原始来源与撤销
规则、前台命令清理、设备授权边界、目标人工验收、根 OMP ledger。
本阶段建议：线性任务分片、原始 journal、收据读取、根预算、版本化控制。
明确后续：完整 DAG/多机迁移、第二云调度器、自动续授权、常驻递归 REPL、
向量库、自动 Skill 晋升。不能用研究报告声称这些已获得。

### P0：先建立可恢复的数据合同

沿现有 SessionState/GoalState 扩展，不另造平行对话身份：

- 同 Session 分段 append journal；有界 checkpoint 引用已确认 seq/hash，
  只加载近期完整工具轮与索引，不每次全量重写或启动时把全部历史装进内存。
- 旧数据迁移先备份与校验，保留 native key/route/来源水位；已有 pending/未知
  效果不能借迁移或新建会话被清掉。
- 同时定义 Goal 根预算、max_slices、绝对期限、累计调用预留、last_settled_run、
  criteria revision，以及验收的 goal_id/expected_revision/reviewed_run_id。
  P0 只保存这些合同，不自动开始后台执行。
- 原始工具收据按 id/hash/范围读取与检索；校验 controller/device/Session/scope，
  纠正和撤销仍使派生投影失效。来源内容仍是数据，不是执行授权。
- intent+调用预算预留落盘→确认当前授权/取消 fence→执行→receipt 落盘→更新
  checkpoint→ACK。receipt 丢失的已执行操作保持 Unknown，先核外部状态。

日志 append、原子 rename、async flush 各有不同保证。Pi JSONL 默认 fsync=false，
其可选 sidecar flush 也不能证明每次 main commit marker 已同步；不能照抄后
宣称掉电安全。本地已有 sync_all/Unix父目录同步的边界不可回退。进程 kill、
OS 崩溃和实际掉电需分开验证；关键 intent/receipt 必须明确持久确认时点。

### P1a：在既有授权链内进行有界分片

Node 是唯一 GoalRunner，与手动 Run 共用派发仲裁；Hub 镜像状态并中继命令，
继续掌管授权，不引入第二个自动调度器。一个 Goal 对应同 native Session 的
多个短 Run；Run 的结束不等于 Goal 的结束，分片不重置根预算。

分清 yielded、waiting_job、waiting_provider、waiting_authorization、
budget_limited、failed、cancelled、awaiting_review；等待/预算用尽不是完成。
无进展应根据产物、日志、回执和已确认存活的 Job 判断；重复状态文案不算进展。
只在真实可用事件/退避截止时续片，不让模型用 sleep/poll 消耗轮次。

每个新 Run 重新按 controller→Hub 协议取得对应 grant；无新授权就等待。
新的独立任务/Session 可以按 Auto 选择；同一 Goal 的续片默认继承既有 route，
新 Run 取得新 grant 不自动解除模型/协议/binding 的约束。切换需要另立明确的
兼容与确认合同；已派发操作/请求的恢复不得悄悄重新随机路由或更换原生身份。
验收修订检查必须在多 Run 启用前完成，不能只用同一个 goal_id 接受后来未看过
的 Run 结果。模型提出完成、确定性检查、用户确认是不同阶段。

### P1b：真正脱离网页的后台任务是独立合同

当前断连会取消工作；不能先删掉这个行为。用户显式创建后台长任务时，才定义
Goal 级授权/期限/预算与暂停、撤销、离线语义。Hub 可以按此合同为新 Run
签发短授权，但仍不是第二 GoalRunner；Node 不能自授或自动延长权限。
模型服务不可达时等待，不盲目调用另一账户。取消先记录请求并停止新派发；
Node 持久化取消 fence、核实现有操作状态后回执，未知副作用仍保留 Unknown。
该阶段需要自己的协议和真实断网/关页验收。

### P2：受管长进程 Job 与按需独立评估

增加显式 Job start/status/read_log/cancel，让编译、下载等跨模型分片等待；
不改变现有前台 exec/run_script 的清理语义。Job 用稳定 id、进程出生身份、
所有权和增量日志 cursor；Windows 仅 PID 不足，进程死亡不是可恢复的计算。
重连应查询原 Job，不能以新 start 替代 poll。
完成标准关联实际 file/hash、工具收据与测试覆盖；必要时单独、有预算地复核，
避免每一轮都额外调用 judge。失败→修复跨 Run 的经验候选可随后复用同 Goal
收据链，但仍须审阅采纳，不放宽当前 current-Run 来源检查来冒充进化。

## 效率指标与必须做的验收

先用固定任务集做前后对照：脚本生成与读回、长编译、日志中段错误定位、
多文件修改后重启继续、压缩后纠正来源、预算/授权耗尽。保留相同模型/路由与
任务输入，报告可重复产物正确率、重复副作用、恢复位置、调用预留/实际已知
usage、耗时、请求字节、峰值 RSS、写盘量、日志配额与 EXE 大小；缺 usage
就是未知，不能填零。无测量不承诺省多少 token 或执行包体积。

| 故障/边界 | 必须观察到的结果；以下均为验收设计，尚未运行 |
|---|---|
| 超过256条/16MiB，跨多个分段重启 | 同 Goal/Session，可回读原件，活动内存与写入开销有界；不是删掉限制后恰好没崩 |
| effect 已成功而 receipt 未落盘 | Unknown，可核目标状态；通用脚本不重复运行 |
| receipt 已落盘、checkpoint 未更新 | 从 journal 重建状态，不重做该 effect |
| 两 Node 进程、旧回执迟到、Hub镜像乱序 | OS 独占锁+writer epoch，镜像按epoch/revision单调；旧镜像不能回写执行状态 |
| 命令 ACK 丢失、重复 start/cancel/resume | 同 command_id+expected_revision 返回已存结果，不生成新任务 |
| 分片续跑、summary、重试、重启 | 根预算预留持续存在；未知消费不因超时自动退还；新Run有新grant |
| 暂停/取消/授权失效与工具派发交错 | 停止新派发；已发生副作用如实记录；不自行续期或将取消等同回滚 |
| 大输出、中段报错、磁盘满、半UTF-8 | 有限摘录+完整受控产物或明确保存失败；查得到已留存的中段错误 |
| 压缩时有工具批次、撤销、源文件变化 | 不切断配对；原件可回读；新观察不冒充旧hash；已撤销内容不复活 |
| 目标旧卡片accept与新Run完成交错 | expected_revision/reviewed_run冲突，不确认未展示的新结果 |
| 错误产物但进程exit0，模型自报完成 | 标准仍未验证，不能自动 accepted |

跨机器/跨进程方案以后再增加分布式 lease。本轮单机仍需 OS writer lock、
启动 epoch、预算/命令/回执的提交顺序；单凭心跳超时不能证明旧进程已死。
云镜像先只同步状态、摘要、必要哈希；本地产物是否上传、何时可用和设备隔离
另有确认，不能把镜像记录当作完整原始日志的云端副本。

## 独立审查与研究边界

三路 Codex 独立视角已各自回源再交叉审查：context_modes_audit 审本地实现与
务实接入；agent_research 审 Pi/OpenCode；context_cloud_audit 审 LangGraph/
OpenHands、持久化与并发。共识是单 Node owner、先数据合同再自动续片。
交叉修正包括：root budget schema 前置、验收 revision 前置、每Run grant结束
不能复用、safe read不保证同一观察、fsync/rename不等于通用掉电保证。
主代理核对上述关键本地源码及上游工具恢复、task writes、Goal循环代码。

本轮没有复制上游运行代码、升级 rpi、修改服务、运行模型或上游测试；保留
许可证的源码缓存仅用于研究。文档/链接校验不是新运行能力验收。后续先落
P0，再用一个真实编码目标验证 P1a，之后再启用后台合同和长进程。

文档校验：本轮3页的40个相对链接全部存在，限定 diff 检查通过；独立审查
重新从官方固定 SHA 核对选入清单的29份源码，字节数与 SHA256 全匹配。
根 verify_bootstrap.py 在20秒有界扫描内未完成，记 NOT COMPLETED，不记通过；
未因纯研究文档变动启动 Cargo、上游测试或真实模型任务。
