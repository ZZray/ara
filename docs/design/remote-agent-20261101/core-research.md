# 远程诊断 Agent：Core、模型授权与恢复边界

调查日期：2026-11-01。仅检查本地源码与已有验收记录；未连接客户机或核对线上部署，也未运行本轮产品测试。

方案已按用户后续澄清调整：APP 直接提供资料；优先复用 SDK/Gateway 连接并扩展独立诊断路由，再经本机 IPC 调用 Agent 服务。独立 Agent 控制连接只作为既有生命周期或协议无法满足时的备选；模型请求仍由 worker 使用独立 HTTPS 短期授权。本文初稿的“独立出站控制”和“断线即取消”建议已由下文明确租约方案替代。

## 版本与证据范围

- ARA Lite / Rust HandWave Node：`E:/repos/ara-github`，HEAD `e229fb9ab800b46313047b89b73ac41e60c9efb4`。13项既有 HandWave/通用知识 WIP 保留。本次引用的 Core、Harness、gateway.rs、transport.rs、node_identity.rs 不在这些既有修改路径中。
- 现代 HandWave Hub：`E:/repos/ara/apps/handwave`，所属 ARA HEAD `0b9cb7c3eabf1e4d82bfc1d6147786c2d883721c`。auth、Hub、Web、Ray 等既有 WIP 未改；引用的 model_grant.go、gateway_proxy.go 未列入 dirty。
- 图索引为发现线索。HandWave Hub 索引为 2026-10-16，Rust Node 为 2026-10-18，Core 为 2026-10-25；本次 coverage 对关键路径均提示 metadata_changed 或 missing，因此关键结论回读当前文件。图返回行号有漂移，下文采用当前文件实际行号。

## 建议采用的分工

客户机运行独立 Windows 服务与完整 Lite Agent 循环。LabSlot 管理机器、任务、权限、事件和结果；APP 继续作为机器资料与已上传数据的权威来源。优先复用现有 SDK/Gateway 连接，新增独立授权的诊断控制路由，通过受控本机 IPC 调用服务。原设备调用继续遵守 direct/relay 选择。Agent 控制连接只有在现有链路不能满足常驻需求时才单独设计；模型请求走独立 HTTPS 授权代理。

LabSlot 后端提供短期、按诊断任务绑定的模型访问授权。客户机只接收代理地址、实际模型/协议元数据与短期访问码；供应商长期密钥只存在云端。客户不用填写模型配置。LabSlot 与 APP 同机部署不能代替接口鉴权或数据归属约定。

## 可以复用的能力

| 能力 | 当前源码证据 | 接入方式与边界 |
|---|---|---|
| 完整模型—工具循环 | `crates/ara-agent-lite-core/src/agent.rs:48`，`apps/handwave-node/src/main.rs:1` | Rust Host 直接依赖 Core；不复制 HandWave 的循环，也不另写一套模型轮询 |
| 自定义模型 Host | `crates/ara-agent-lite-core/src/model.rs:61–93` | ModelHost 提供实际 route、HTTP client、能力与 headers；身份和凭据仍由产品 Host 管理 |
| 免客户端选择模型 | `apps/handwave-node/src/gateway.rs:9–64`、`:117–130` | HandWave 可由本次 Hub grant 解析默认模型/协议；目录未配置时明确失败。可复用契约思路，不能将模型未配置显示为在线可诊断 |
| 原生 Session、收据、预算、取消 | `crates/ara-agent-lite-core/docs/knowledge/host-contracts.md:7` 起 | Core 保存原生状态，Host 选择隔离路径、维护任务与机器归属、校验结果 |
| 多 Run 与恢复 | `crates/ara-agent-lite-harness/src/types.rs:295–307`、`runner.rs:409–415` | Harness 让 Host 复核产物再续写，保持同 Session、ModelHost、steering 和工具 registry；不是远程消息队列 |
| 有界连接恢复 | `apps/handwave-node/src/connection.rs:10–12`、`:37`、`:102–108`；`transport.rs:70–80` | 已有 Ping/Pong、连接/写入超时与退避范式；取消清理没有结算前拒绝交给新连接，重连不重放执行 |
| 安全传输与机器证明参考 | `apps/handwave-node/src/transport_security.rs:5–12`、`node_identity.rs:41–74` | 公网 HTTPS/WSS；连接挑战签名。需要与 APP 机器身份对接，不能拿显示指纹、机器名或序列号直接授权 |

## 不能直接照搬的部分

1. HandWave Node 当前在 `transport.rs:520` 启用 `enable_local_tools=true`。Core 的安全嵌入默认是 `false`（`agent.rs:88–99`）。QSpin 首期只注册日志检索、分窗读取、进程/版本/服务状态等工具，不注册 shell、文件编辑、脚本下载执行或硬件控制。现有内置 `read_file/search_files` 接受绝对路径（`local_tools.rs:28–34`），不适合作为受限客户日志接口的原样替代品。
2. HandWave 旧 default/explicit grant 与 smart grant 的隔离能力不同。`model_grant.go:139–155` 发短期码并记录 FP/Run；`gateway_proxy.go:166–200` 的 Run、binding、协议和模型检查仅适用于 smart 路径。新诊断代理必须执行任务绑定检查，不能把短期码本身当作完整隔离。
3. Core `agent.rs:811–823` 的 redact 只替换当前 grant 字符串。它不是客户日志脱敏器。诊断日志必须在工具返回给 Core 前按字段处理，避免秘密进入模型请求、Session、事件或云端日志。
4. 现有 Rust Node 按 grant TTL 与 decision expiry 收紧本次 Run 截止（`transport.rs:458–525`），不自动续权。长诊断的授权续期是新 Host 契约，不能因服务常驻就永久授权执行。
5. Lite 的成功 Run 不等于诊断业务已解决。Harness `verify` 要检查实际证据（`types.rs:295–298`）；诊断结果可标记“证据已收集、待人工确认”，不能仅凭模型答复标记客户问题已解决。

## 建议的授权与运行协议

以下字段为设计建议，不是已发布 API：

- 机器资产保留 `source=app` 与 APP 原始主键；Agent 安装有独立 `installation_id` 和公钥身份。一台机器可关联多台仪器。重装或换钥应显式重新绑定，不能静默继承旧安装的执行权。
- 任务包含 `task_id`、`machine_id`、`installation_id`、发起人、组织/项目范围、目标、工具策略版本、日志源范围、调用预算、返回字节预算与绝对截止。下发前与执行前分别校验。
- 模型代理把短期码映射到上述任务及实际 `run_id`、模型/协议绑定；不向客户机下发供应商长期密钥。每次请求重新检查撤销、截止、配额和所属关系。
- 下发带稳定消息 ID；接收端先落盘再确认。重复消息返回原任务状态，不新建 Run。云端保留任务收据，Agent 保留执行收据；断线时查询原任务再继续观察。
- 取消分为“已请求取消”和“已停止并结算”。连接断开不证明任务未执行。收敛方案为服务接收后按有限任务租约执行；页面关闭只停止观察。控制路由断开时不接新任务，已有任务只在原执行/模型租约内运行并保存收据，恢复后按 Task/seq 补读。离线撤销的延迟上界为原短租约剩余时间；完全断网不能调用云模型。若内部原型只有浏览器 direct，则关闭页后的后台新派发、持续云端归档仍不具备，必须明确标记。
- resume 沿用原 Task/Session 和预算；Host 核对开放调用收据。不确定效果保持 `ReconciliationRequired`，不换 Session 或重发命令来消除状态。
- 模型输入中的日志是外部数据，不能扩大工具权限或改变诊断目标；工具执行端严格执行准入策略。

## 日志与带宽

原始日志默认留在客户机；命中片段、摘要、统计与诊断证据会经云端传递，模型推理也会接触脱敏后的片段。因此只能承诺减少整包传输，不能承诺零日志出网。

日志工具宜返回 `source_id`、机器/实例、文件身份或轮转代际、采集时间、时间范围、字节范围/行号、截断状态、续读游标、脱敏规则版本与证据引用。原始哈希与脱敏后哈希应分开，不能对脱敏结果标一个原件 hash 后声称字节一致。读取时发现轮转应使旧游标失效并提示重新检索。

先在本地按时间、级别、组件和关联 ID 筛选，再给模型有限窗口；总返回预算跨工具与跨 Run 累计。无匹配、未读到、权限拒绝、扫描被预算截断是不同结果。只有已扫描范围可报告无匹配。

## 性能与服务权限建议

- 先以每个安装单活跃诊断任务落地。对日志扫描设置独立的扫描字节/耗时/并发上限；模型慢不应拖住 QSpin 主进程。
- LocalService + 独立服务 SID/ACL 是可评估候选。必须在真实安装目录与用户日志目录验证读取范围；读不到时报告缺少哪个源，不能自动升为 LocalSystem。
- 服务状态目录与凭据文件只允许服务身份和管理员访问。日志根只读，状态目录可写；不把 QSpin 的全部用户目录授权给服务。
- 首期不提供通用 shell。后续若要远程改配置、重启或硬件操作，应有操作级授权、稳定操作 ID 与效果结算，不能通过“扩展日志工具”隐式加入。

## 必须验收的场景

| 场景 | 可观察验收 |
|---|---|
| 首次接入 | 已导入资产无Agent时显示未安装；新安装完成身份证明后才显示可用；拒绝错绑与重复安装冒用 |
| 正常诊断 | 无客户端模型配置，多步检索和补读完成；每条判断可打开对应证据；明确实际上传字节与模型调用数 |
| 日志权限与轮转 | 无权限、日志缺失、轮转、截断、编码错误分别报告；不将未覆盖范围判为无问题 |
| 断网/重连/重复下发 | 不创建重复任务或重放工具；状态与已有收据一致；超期不继续模型调用 |
| 取消/进程终止 | 取消终态只在清理后产生；服务强杀/重启保留 fence，按收据恢复，无法结算时阻断 |
| 授权与模型代理 | 越租户/机器/安装/Task/Run的请求被拒绝；撤销、过期、额度耗尽无后续调用；长期密钥不出云端 |
| 资源影响 | 在内部机记录空闲与诊断期间 CPU、RSS、磁盘读量、网络字节及 QSpin 响应，阈值经实际测量后确定 |

## 当前验收限制

`crates/ara-agent-lite-core/docs/evidence/lite-capability/handoff-20261030-codex.md:71–83` 记录：任务61仍未通过；两项 symlink 权限测试保留 WinError 1314；最新差异未重跑整套11命令及115项Python全量。`:35–39` 区分受控长任务机制与真实长任务不足。可据这些能力开展内部试点设计，不能称远程客户服务已具备整体验收或发布资格。

## 三视角交叉意见

赞同 APP 为直接资料源、LabSlot 管理调度、SDK/Gateway 优先复用、独立 Agent 服务及 Lite 完整循环的分工。对初期仅做受限日志/状态工具没有异议。保留条件是：APP 关联不能授予执行权限；安装身份要有可信证明；服务账户日志访问需安装验证；脱敏必须发生在 Core 收到片段之前；断线恢复、模型续权和未知结果需按明确契约验收。SDK 的 agent.start/cancel/resume 属于命令，不能套用只读 query 自动重试。任务控制与事件应有界分页限流，模型流不占 SDK 设备事件通道。

## 最终设计复审

审查者：独立 Codex 子代理 `/root/core_protocol_design`，性能与安全视角。2026-11-01 复核主稿与实际生成 HTML，并与数据源、Windows 服务调查代理交叉检查。最终绑定：

- `build-report.mjs` SHA256：`acc59c7f8cce62acec30e279de37bf9df6a89e3e3a0fad25d324a21b4ce3349b`。
- `design.html` SHA256：`d15cfd2b79b0f734d6864ee471738fe1d081c148f8ee2aabe72f3a797db6c952`。

本视角无未解决的设计阻断。初审的独立出站与 SDK 优先冲突、Task 控制误用 query 重试、页面关闭与断线策略矛盾、IPC 身份边界不明确、工具窗口过大及共享 SDK 链路资源争用，均已在最终稿给出明确约束。生成 HTML 包含命名管道 ACL、服务验证云端签发的安装/任务/操作者/日志范围/期限、有限租约、默认 8 KiB 窗口和模型独立 HTTPS 路由；图与正文采用相同分工。

结论仅接受设计交付，不代表 SDK 已发布 agent.*、服务安装通过或客户现场可上线。新诊断路由、Gateway 生命周期、任务幂等与撤销窗口、真实日志 ACL、性能影响和产品端到端仍按主稿验收清单执行。此次复审检查源码和文档内容，没有运行浏览器视觉验收或产品测试。
