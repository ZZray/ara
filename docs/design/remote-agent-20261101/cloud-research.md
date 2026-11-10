# APP 权威数据源与远程 Agent 接线调查

调查日期：2026-11-01。只读源码调查，未连接线上服务或客户机器。

## 版本与范围

- APP 主仓：`E:/repos/App/qspin_cloud_services`，`main`，`5080ad79f333c86e50c6f4c1f0b8fded01014099`，工作区干净。
- APP 优化候选：`E:/repos/App/qspin-cloud-resource-opt`，`0ac80c0a4bde6b24f6a4a63cde9662e4a72909d4`，有既有未提交修改。本方案以主仓为事实依据，不把候选能力算作已上线。
- LabSlot：`E:/repos/qspin/QuantumSpin/tools/labslot`，QSpin `dev1`，`367090d213e3deb1a7d6cdf4b93834a6e02eae31`。LabSlot 范围没有 dirty；仓库其他 QSpin/Gateway 既有 WIP 未改。
- 图索引用于定位，关键结论均回源。主仓新建 fast 索引；覆盖工具提示 metadata_changed，已直接读取列出的源文件。未宣称穷尽所有接口或线上部署状态。

## 现有事实

| 事实 | 源码证据 |
|---|---|
| APP Device 主键是自增 BigInt，serialNumber 唯一；记录整机、客户归属、仪器状态和当前网关信息 | `prisma/schema.prisma:74-134` |
| GatewayInstallation 有独立安装 ID、machineHash、hostName；DeviceGatewayBinding 保存设备与安装实例的关联及 revokedAt | `prisma/schema.prisma:136-166` |
| LabSlot 的 devices.id 是用户自定义 varchar，不等于 APP bigint ID；SDK 路由默认 direct | `apps/api/src/database/schema.ts:103-150` |
| APP 用户列表 GET /devices 已分页，但只允许 admin/ops 会话 | `src/modules/device-master/device-master.controller.ts:19-64`；`device-master.service.ts:110-177` |
| APP 已有给售后系统用的外部只读详情接口，按 ID 或 SN 查询，使用 X-API-Key；此控制器没有列表接口 | `src/modules/device-master/device-open.controller.ts:12-63` |
| 详情可以复用组织、地址、网关、最新指标、样品、运行实验，含数据时间 | `device-master.service.ts:194-268,1772-1889,1909-1930` |
| device_report_logs 保存上报 JSON body，当前 rawHeaders 写 JsonNull；不是客户机 QSpin/Server 文本日志库 | `prisma/schema.prisma:412-427`；`src/modules/device-access/device-access.service.ts:283-302` |
| APP 有事件查询和 metrics/history、versions；现有鉴权为 APP admin/ops 用户会话 | `src/modules/event/event.controller.ts:8-23`；`device-master.controller.ts:19-64` |
| APP feedback 支持用户反馈及最多 3 个附件，属于客户会话下的独立业务数据 | `src/modules/feedback/feedback.controller.ts:51-129` |
| 设备注销保留 Device，清凭据并改 pending_activation；不是删除历史 | `src/modules/device-auth/device-auth.service.ts:468-510` |
| APP 只接受当前 activeGatewayTokenFamily；另一网关会话会被拒绝 | `device-auth.service.ts:1064-1076`；注册规则另见 `docs/05-gateway-integration.md` |
| LabSlot 已有 DB-backed 用户、组、设备可见性和独立操作权限概念 | `apps/api/src/common/security/lab-resource-scope.ts:24-60`；`modules/device/device.controller.ts:12-105` |

APP 路径均相对 APP 主仓；LabSlot 路径均相对上列 LabSlot 根目录。APP 全局前缀应沿发布配置核验，文中列控制器相对路由。

## 推荐边界

1. APP 继续拥有设备、客户和上报数据。由 APP 维护者增加窄的服务间目录/数据查询合同；LabSlot 经只读 adapter 访问，不双写 APP 表、不复制其凭据。
2. 按用户最新澄清，LabSlot 直接展示 APP 权威目录与数据，只持久化来源关联、诊断权限、任务和 Agent 元数据。无需导入设备主表或建设第二套同步主数据；短期缓存必须标来源时间与过期状态。
3. 优先复用 SDK/Gateway 已有连接和信令。客户 Agent 的任务与模型会话如何挂接现有连接，仍需核对实际方法合同；独立出站诊断通道作为隔离备选，不把另建传输当成既定要求。APP 与 LabSlot 同机部署可降低内部调用成本，但服务、身份和数据权限仍明确分开。
4. Agent 不复用 APP 设备登录/refresh 会话。新登录可能更换当前网关 token family，影响现有遥测上报。Agent 自身安装实例与凭据另发，关联 APP 资料时验证安装身份；机器名、序列号和 machineHash 都不能单独作为认证凭据。
5. 复用现有 LabSlot 用户认证和组管理实现新资源权限，明确“查看机器/导入目录/发起诊断/查看结果”；拥有预约仪器权限不自动拥有客户主机日志访问权。

## 直接使用 APP 数据源

APP 只读分页目录返回 sourceDeviceId（字符串）、serialNumber、组织标识、名称、型号、gatewayInstallationId、hostName、version、sourceUpdatedAt、active 状态；客户联系人等非必要字段不随目录批量返回。若需要按机器展示，由安装实例聚合设备关联，保留一台机器对应多台仪器的关系。缺安装 ID 的记录显示为待关联，不推断对应某台在线 Agent。现有 APP admin/ops 列表可作为实现参考，服务间读权限应明确；无需用户重复录入或导入设备。

LabSlot 目录将“可关联、已关联、身份冲突、源已停用、缺少安装信息”分开呈现；选择启用诊断时只提交关联和权限，核对最新来源身份，返回收据。APP onlineStatus 作为单独的“APP 最近上报状态”展示，不能替代 Agent 连接状态。此前拟议的导入预览/提交/增量同步不是本方案必需环节；仅在未来确有离线资产管理需求时再讨论。

来源映射建议唯一键 `(sourceSystem, sourceDeviceId)`；Agent 使用独立 `agentInstallationId`。若使用本地 `remoteMachineId`，让它作为机器资产的本地稳定标识，再关联安装历史和多个 APP 设备。不要用 SN 直接充当机器 ID，亦不要用主机名或仅 machineHash 自动合并两台机器。

APP 拥有名称/型号/客户归属等源字段，LabSlot 按需读取；LabSlot 拥有备注、诊断权限、任务和 Agent 状态。源缺失/停用标“来源不可用/已停用”，不级联删除诊断记录；身份或所属组织变化暂停旧关联的诊断授权，重新核验。这样可处理重装、换机、设备转移和短期 APP 故障。

## SDK 已有连接与日志能力

| 已核对的能力 | 证据与边界 |
|---|---|
| SDK 握手取得 Gateway 版本、signalingUrl、deviceId | `QuantumSpin/sdk/qspin-sdk-ts/v2/src/handshake.ts:11-38`；SDK 是 server，Gateway 主动接入，见同文件约 95 行。无需重新发明这一发现链路 |
| LabSlot 创建按设备的 SDK session、绑定路由、签发/验证安装身份 token | `apps/api/src/modules/device/qspin-sdk/qspin-sdk.service.ts:480-588,978-1074,1166-1189` |
| 网页调用已有信令查询，再用登录 token 连接 WebRTCTransport | `apps/web/lib/direct-channel.ts:340-377,601-636` |
| 已有 `/api/sdk-pair/devices/:id/signaling` | `apps/api/src/modules/device/sdk-pair/sdk-pair.repository.ts:310-350`；当前查 LabSlot devices 或 local ephemeral，再确保 session、返回 route 派生 URL。没有从 APP 目录自动兑换授权的接线证据 |
| Gateway 管理界面有日志 tail/query/stream | `QuantumSpin/tools/qspin_gateway_rs/internal/webui/server.go:318-320`；`observability_handlers.go:353-360` 的 tail 读取 Gateway 自身 LogsDir。这是管理 HTTP API，不能当作已提供给 SDK 的 QSpin/Server 日志方法 |

因此用户提出的“拿到 Gateway 信息，通过普通 SDK 连接”有现有实现基础。需补的是 APP 来源身份与已有路由的关联、具体远程 Agent 方法和诊断权限，不是复制设备表后从零建设连接。SDK 一旦连接，可调用的仍是已注册并授权的方法；当前调查未验证 Agent 任务、模型授权或 QSpin/Server 日志检索已作为 SDK 方法发布。当前 APP open 详情给安装/主机/版本等资料，不返回上述 LabSlot route token 或 signalingUrl。

## 上传数据复用

最小首期复用：机器/仪器资料、版本、最近上报时间、最新指标与运行状态快照。需历史分析时，按时间窗读取指标、事件、实验摘要。每份结果标来源、采样时间、抓取时间及截断/过期状态。

不把 APP report-log 当完整日志备份。QSpin/Server 本地日志仍由远端 Lite Agent 按允许目录检索、补读；向模型发送必要片段，并把引用和读取收据返回 LabSlot。APP 反馈附件可以另加只读入口，但须保留反馈可见性和附件权限；本轮不假定已有可供 LabSlot 下载的服务间接口。

现有 open 详情使用全局 key 且返回客户和实验资料。正式目录集成建议专用服务凭据及明确可读范围，不把全局 key 下发到浏览器或客户 Agent，不因“同一台服务器”直接开放全库。

## 架构评审结论

APP 作为实时权威数据源、LabSlot 只保存关联与诊断元数据，并优先复用现有 SDK/Gateway 连接，是本轮更小的方案。独立 Agent 出站通道保留为经过协议评估后的备选；现有 Gateway 上报和 RTC/direct 硬件执行语义保持。

需补的真实接口是 APP 服务间分页目录及所选数据查询、来源与 Agent/路由的身份关联、任务/事件/结果合同。读取目录、SDK 建连、Agent 在线、读取本地日志、调用模型应分别验收。

待验证：线上 APP 版本与域名/内部接口地址；实际记录是否包含安装 ID；一机多仪器和 Server 跨机分布；客户及组织归属到 LabSlot 权限的映射；模型与日志数据的授权范围；两服务部署同机这一用户提供事实的运行核验。未连接线上、未改产品、未运行集成测试。

## 方案复审：连接复用与维护成本

审查对象为同目录 `build-report.mjs`，主代理仍在修订，以下是需落实的设计约束，不是最终版验收结论。

优先方案应是 APP 直读、已有 SDK/Gateway 连接、新增 Agent capability、本地受控 IPC、独立 Windows 服务执行任务。它复用已有配对、连接发现、路由和握手，减少第二套连接/身份/重连协议的维护。Agent worker 直接向受控模型代理发请求，模型 grant 与 Gateway token 分开。

服务常驻与控制连接常驻是两个验收项。现有 Gateway 生命周期或 SDK 协议不满足时，再在“服务托管 Gateway”和“Agent 复用 SDK 协议独立连接”之间选择；不先建设整套新通信栈。前者影响既有网关生命周期，后者增加一个已知协议端点，需用实际改动量和部署验证比较。

复用还需处理三处具体约束：

- 当前 `qspin-sdk.service.ts:373-387` 的 `beginCloudOperation` 在 direct 模式拒绝云端请求。不可直接把新增 `agent.*` 塞进现有云 query 后声称可用。可以由浏览器 direct 提交、服务持久执行，或另定具备独立诊断权限的控制路由；任一方案均不自动改变硬件 direct/relay 选择。关闭网页/手机离线后的执行、取消和结果恢复须单独实测。
- 当前同文件 `resolveDeviceId:1087-1105` 拒绝一个 installationId 绑定不同非 local device。“一机多仪器”应由新的机器资源与多个 APP 设备关联表达，不宣称现有配对逻辑已经支持任意多对多，也不为展示聚合改旧路由身份约束。
- APP 当前权限依据 `Device.ownerUserId`、`DeviceMember` 及用户角色（`device-master.service.ts:1375-1404`），组织名是显示值。LabSlot 应使用显式批准的来源实例、稳定主体/设备范围映射；不能按组织名、主机名、邮箱等字符串自动继承诊断权限。过滤应发生在目录分页/计数、详情、任务、流、证据和模型输入边界。支持工程师的范围可由管理员明确授予，无需伪装成客户 APP 登录。

原稿将独立出站作为推荐，在上述成本和现有能力未比较前过早。修订应同时修改正文、接口表、流程箭头、SVG替代文本和图注，避免正文说复用、图仍承诺独立通道。

## 最终稿复核结果

2026-11-01 独立复核主代理修订稿，绑定文件 SHA-256：

- `build-report.mjs`：`e477fafca74f2ef31b657819a4ade5ae348185dfcb53a981317dd72ed86e92b3`
- `design.html`：`cce138835e8cb93155926a3e5373011fe2b52d8acfea4a6e05e04727659c72ac`

本轮提出的三项约束已落实，所审范围没有剩余阻断项：

1. 主稿 59–60 行明确新机器资源可关联多个 APP 设备，同时保留旧 SDK installation/device 配对限制；权限用稳定 owner/member/device ID 显式映射，分页、计数、详情、任务、证据及模型输入均过滤。
2. 32–34 行将 SDK/Gateway 复用列为优先，新增后台诊断控制路由明确为待扩展且独立授权，原 query/command 继续遵守 direct/relay；服务常驻与控制连接存活分项验收。
3. 100–104 行明确 start/cancel/resume 属于 command、不套 query 自动重试；页面关闭与控制路由断线分别处理，离线撤权受有限租约约束；共享链路仅承载有界控制与事件，模型流走独立 HTTPS。171–173 行的图已改为 SDK 诊断路由、IPC、HTTPS 模型请求，与正文一致。

此结论接受设计方向与上述文字的一致性，不代表协议实现、身份绑定、权限联调、安装、长期运行或产品端到端验收。未改主稿，未连接线上或客户机器。

## MCP 设计增补复核

2026-11-01 回源核对 V2/V3 MCP 与 Lite MCP，并审查主稿 `verification-capabilities`、`verification-loop` 中对应部分。V2 的 `src/mcp/tools.ts` 固定 11 项查询；`server.ts:103`、`tools.ts:144,176` 的 `console.info` 存在 stdio 输出污染风险，本轮未运行复现。V3 `mcp/src/tools.mjs` 使用 authenticated callable 非 UI 目录，8 项分页，operation 返回受理回执、原 intent 与 `completion_pending`，错误保留 `may_have_been_sent` 等恢复字段。Lite `client.rs:21-23` 上限为 32 页/512 工具，默认每页 8 项时先受 256 项限制；`lib.rs:235-244` 对截断仍返回 Connected。Lite `tool.rs:86-99` 将 MCP isError 转普通错误文本，不能自动把 V3 已返回的未知效果变成 Harness 对账状态。

新增两节已准确记录现有复用面和缺口：完整目录与必需工具门、CLI 页长尚未可配置的边界、通用错误截断前持久化/解释原始收据、真正未知效果的结构化阻断、原 operation 恢复、禁止自动 V2/UI 重放，以及安装端负责 SDK 身份配置。未将这些桥接能力写成已实现或插入即可用。所审 MCP 部分 0 个未解决发现；视觉能力、UI Agent 实现事实由另外视角审核，不属于本复核的独立源码覆盖。

本次绑定 SHA-256：`build-report.mjs = 0ac9b29d6488a185b93e3fc1dad440068bf4126481154d662e18c16f0edb52aa`；`design.html = 6f2a45e36e5af0d16ee287602e9aa31ecf708c8d6940572f79dacabc1e168ea1`。读取前后 hash 一致；未修改主稿、未运行产品或硬件。实现时仍须保持 V3 的 `qspin_close_operation`/stdio 退出只关闭本地消费、业务取消另发并观察终态的原合同。
