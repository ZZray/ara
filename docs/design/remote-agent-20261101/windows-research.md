# QSpin 远程 Agent：Windows 安装与本地日志调查

调查日期：2026-11-01。仅源码与安装脚本阅读；未安装服务、构建、修改业务、连接客户机或核验线上部署。

## 版本与证据边界

| 工作区 | 分支 / HEAD | 工作区情况 |
| --- | --- | --- |
| `E:/repos/qspin` | dev1 / `367090d213e3deb1a7d6cdf4b93834a6e02eae31` | 有大量既有 WIP；本次引用的 maintenance_service、mem_trim、MemoryTrimService、日志知识页无差异 |
| `E:/repos/qspin2` | rc / `d49abe08ee092ddd2572886a7f442dccf59e9d69` | 调查起点 status 为空；只用索引找线索，关键结论回读 dev1 当前源 |
| `D:/WorkSpace/build_package` | QSpin / `55a7e5492c93c661c3cc1abe165ed63a8889bea9` | app.7z、app.nsh、nim_setup.nsi、skin.zip 等已有修改；本次引用 ui_nim_setup.nsh、config_qspin*.json 无差异 |

图索引 E-repos-qspin2 为 2026-10-26 fast；覆盖检查明确 tools 子树 excluded，MemoryTrimService metadata_changed。故安装服务结论全部读取实际文件，未把图查询无结果当成不存在。

## 已有的服务安装闭环

用户说的“内存整理程序安装成服务”有直接对应实现：SCM 托管 `qspin_maintenance_service.exe`，它收到本机请求后启动 `qspin_mem_trim.exe`。两者职责分开。

| 事实 | 当前来源 |
| --- | --- |
| 服务名 `QSpinMaintenanceService`，环回端口 56592 | `QuantumSpin/tools/maintenance_service/MaintenanceRuntime.h:21` |
| 原生 SCM 入口，接受 STOP / SHUTDOWN，报告 START_PENDING、RUNNING、STOP_PENDING、STOPPED | `QuantumSpin/tools/maintenance_service/main.cpp:18`、`:32`、`:43`、`:99` |
| 支持 `--console` 开发模式；SCM 模式独立于 QSpin GUI | 同上 `:75`、`:101` |
| 维护服务只监听 127.0.0.1，2 个 HTTP 线程 | `QuantumSpin/tools/maintenance_service/MaintenanceRuntime.cpp:214` |
| 数据目录 `%ProgramData%/CIQTEK/QSpin/Maintenance`，token 与日志在其下 | 同上 `:286`、`:293`、`:307` |
| 维护目录 SYSTEM/Admin 全权、Users 读执行；token Users 可读。这是当前本地维护调用契约 | 同上 `:303`、`:340` |
| helper 固定从服务 EXE 同目录启动，CREATE_NO_WINDOW，无任意 shell；一次只运行一个 trim | 同上 `:454`、`:466`、`:487` |
| helper 执行上限 180 秒，停止等待 15 秒 | 同上 `:23` |
| Program Files 独立保护目录、复制 service / helper / hv.dll | `D:/WorkSpace/build_package/package/SetupScripts/nim/ui_nim_setup.nsh:16`、`:188` |
| SCM create/config 为 auto，失败恢复为 5 秒、15 秒重启，后续不再重启，重置周期一天 | 同上 `:214`、`:221`、`:226` |
| 启动后验证 RUNNING，再查本地 health HTTP 200 | 同上 `:240`、`:258` |
| 升级先等待旧服务 STOPPED，最多 60 秒，失败中止安装；服务安装失败提示但继续安装 QSpin | 同上 `:153`、`:615`、`:638` |
| 卸载先停止并删除 SCM 服务，确认消失后移除程序目录 | 同上 `:830`、`:865`、`:932` |
| 包配置要求 maintenance、mem_trim 等产物，构建目标已有接线 | `D:/WorkSpace/build_package/config_qspin_dev.json:15`、`:34` |

安装脚本的 `sc create` 没有指定 `obj=`，按 Windows 默认语义，新建服务使用 LocalSystem；这里是脚本与系统契约推导，未查询任何现场服务账户。`sc config` 不重设账户，已有服务实际账户仍须单独核验。

上述文件为流程复用参考，不建议把远程 Agent 加入维护服务进程。维护服务具有系统内存操作目的，其本地 token 允许普通用户读取，不应直接变成云端 Agent 凭据或远程工具权限。

另一个明确边界：`QuantumSpin/tools/qspin_gateway_rs/README.md:130` 说明 Go Gateway 未实现原生 SCM handler，桌面部署由 QSpin 经 `--parent-pid` 启动。按用户后续澄清，首期优先复用现有 SDK/Gateway 连接，扩展独立授权的诊断命令与事件路由，再经受控本机 IPC 接入 Agent 服务。Agent 自有 SCM 生命周期，控制连接在 QSpin 退出后是否继续可用须另外验收。若既有生命周期或协议不能满足，再评估 Gateway 受服务托管，或 Agent 复用 SDK 协议建立独立受限连接；不把新网络栈或 Gateway 生命周期改造预设为首期前置。

## 本地日志并非全是文本文件

`QuantumSpin/app/database/database.h:246`、`:274` 通过 `useClientConfigStore()->configPath("db/QSpin.db")` 取得数据库位置。不要用 SCM 账户的 AppData 推断客户 QSpin 的配置目录；安装或 enrollment 时应登记经验证的实例根目录。

当前 dev1 的 `LogService.cpp:58` 建立 `t_nmr_log`，`:101` 建立 `t_server_log`，`:123` 将历史混表中 source=server 的记录迁到服务端表。远程 Agent 要识别实际 schema，读取既有数据库而不调用产品迁移/建表逻辑。旧客户端可能仍是混表。

日志知识页 `QuantumSpin/docs/knowledge/server-log-channels.md:72`、`:80`、`:84`、`:85` 记录 Server 被动缓存、RPC 拉取与事务入库：Server 缓存有 10000 条 / 8 MiB 上限；跨 Server session 重启的缺失量未知；客户端收取的 Server 日志保存在同库独立表。此资料可用于设计，但不能假定每台现有客户机已升级到相同版本。

首期数据源建议：

1. QSpin SQLite 日志：只读连接、固定字段和参数化筛选、短事务、按 ID 游标分页。限制查询时间、行数与输出字节；不得把打开活动 SQLite 主文件后直接复制视为一致备份。
2. 客户端已缓存的 Server 日志：在结果上写“客户端缓存”、采集覆盖时间、最后同步时间及已知缺口。GUI 关闭以后没有继续收取 Server 日志，不应显示实时。
3. 本机 Server 文件与专用通道：只有文件在这台机器、路径被明确登记且账户可读时启用；结果带文件身份、字节范围、轮转代次与截断标志。
4. Server 位于另一台电脑：需要在那台 Server 主机部署 Agent，或另行验收只读日志 RPC 适配。APP 注册一台前端机器并不自动赋予读取后端文件能力。

## 建议的 Windows 结构

建议产品名 `QSpin Remote Agent`，服务名 `QSpinRemoteAgent`，为待实施命名。用独立 Rust Host 直接依赖固定版本的 Lite Core，保留 Core 的产品无关性。

最小结构是一个签名 EXE，提供 `service` 与 `worker` 两种模式：SCM 服务通过受控本机 IPC 接收 Gateway 转来的已授权任务，承担安装身份、任务准入、持久化与升级停止；有诊断任务时启动受限 worker，worker 运行完整 Lite Core 循环与日志工具。这样闲置时不常驻模型上下文，worker 故障不会立即丢失任务账本。IPC 必须限制可调用身份并核对目标安装、操作者及授权范围。首期无需做通用插件系统或任意命令远控。

优先评估显式低权限 LocalService + 服务 SID，并只授予登记日志目录读取权、Agent 自有 ProgramData 目录写入权。是否能读取现场现有 SQLite/WAL，须实机 ACL 验证后定案。不要为少数拒绝读取而把整个网络 Agent 升为 LocalSystem；先补对应只读 ACL。允许工具清单与低权限账户分别约束业务范围和 OS 权限，互不替代。

建议目录：Program Files/CIQTEK/QSpin Remote Agent 保存二进制；ProgramData/CIQTEK/QSpin/RemoteAgent 保存注册状态、任务事件、checkpoint 和 Agent 自身日志。机器长期凭据仅该服务身份与管理员可读；模型配置、供应商密钥不进入客户安装包。

模型请求通过独立 HTTPS 云端统一代理，任务携带绑定 machine/task 的短期授权、模型路由和预算，不复用 Gateway 登录 token。SDK/Gateway 共线只传有界任务控制、摘要与分页事件；模型 token 流不占用该控制通道。客户机运行 Lite Core 决策循环，不要求客户配置模型；日志筛选后的片段仍会进入云端模型请求，不能声称日志数据完全不离开客户电脑。

新增的 start/cancel/resume 是有副作用命令，不能因日志工具只读而注册为可自动重试的 query。status/events 可按 task_id 与 event_seq 幂等补读。现有 direct 由浏览器持有；持久后台诊断控制路由是待扩展能力，不能把云端后台派发描述为已有 direct 能力，也不全局打开硬件 relay。

任务在服务持久接收后按有限租约执行。关闭页面只停止观察；控制路由中断时不接新任务，既有任务仅在原执行/模型租约内运行并保存事件，重连按收据核对。取消在远端确认前显示待确认；离线不能保证即时收到撤销，窗口上界为原短租约剩余时间，完全断网则不能调用云模型。

## 安装、更新和恢复

安装流程可沿用 NSIS 的组件载荷核对、SCM 注册、停止等待、启动检查结构，但远程 Agent 需要自己的服务与目录：

1. 安装包校验并部署固定版本 EXE，配置显式账户和最小 ACL，登记日志实例目录；安装数据与 APP 注册事实关联，不把可猜机器 ID 当认证凭据。
2. 注册自动启动服务，系统启动且尚未登录用户时也可启动。经既有 SDK/Gateway 与 IPC 报告状态，区分“服务运行”“控制连接建立”“身份已授权”“日志可读”四项；不得用 SCM 正常推断 GUI 退出后的控制连接也正常。
3. 云端不可达时保留“已安装，等待联网”，不阻止 QSpin 主功能使用，不虚报远程可用。
4. 更新先停止接收新任务，给当前任务有界排空时间，持久化 checkpoint 和收据；未停止则保留旧版本并报告。禁止直接覆盖仍运行的 EXE。
5. 切换版本后核对 EXE 哈希、服务版本、持久化格式与健康状态；失败回退二进制。持久化格式迁移必须向后兼容或保存可恢复副本，二进制回退不自动等于状态回退成功。
6. 重启恢复时按 task/session/run 与已确认事件序号续传；执行结果未知的工具先对账，不以重开会话伪装原 Run 恢复。只读日志重查也要声明采样窗口变了。
7. 卸载停止并删除服务与程序，云端标记离线/退役；身份撤销与状态保留分开处理。不要自动删除客户 QSpin 数据；Agent 记录的保留期限与清除选项在产品方案中明确。

资源初值建议仅用于内部试点测量，不是已证明可用指标：每机同时 1 个诊断任务；worker 有内存、CPU、读盘速率和总时长上限；日志检索分页返回；待回传事件缓冲按字节封顶。限额命中返回明确的部分结果和继续位置，不能把截断判成“没有错误”。具体数值由内部机器基线与诊断样例确定，避免直接沿用维护 helper 的 180 秒窗口。

## 交付验收

| 层次 | 可观察标准 |
| --- | --- |
| 安装 | 全新安装、覆盖升级、修复安装、卸载均有收据；服务实际账户、EXE 路径/哈希及启动类型与设计一致 |
| 独立生命周期 | 分别验证 SCM 服务存活与控制连接可达；未登录、QSpin 未启动/退出/卡死时能否接收已授权任务必须有运行证据，不满足则触发连接托管方案选择；QSpin/Gateway 原行为保持 |
| 日志读取 | 新旧 schema、中文路径、轮转、权限拒绝、锁忙、活动 WAL、断电后恢复、空匹配均返回准确状态和可追溯来源 |
| 资源 | 诊断期间测量 QSpin 操作延迟和采集业务受影响程度；CPU/内存/磁盘/输出限额命中能停止并保留部分结果 |
| 断线恢复 | 任务中断云连接、服务被 kill、worker 崩溃、Windows 重启分别演练；无重复提交、无假成功、无无界重试 |
| 升级回退 | 运行任务期间更新、旧服务拒绝退出、新版本启动失败、状态版本不兼容分别有明确结果与恢复路径 |

本调查只支持方案决策。未对安装包、现场服务账户、客户日志 ACL、Windows 冷启动、资源预算或上述故障路径做运行验收。

## 最后复审

2026-11-01，独立 Codex reviewer `/root/windows_agent_design` 从实现与维护成本角度复核主稿，并与 `/root/cloud_data_design`、`/root/core_protocol_design` 交换意见。主稿已把首期推荐收敛为 APP 直读、SDK/Gateway 诊断路由、受控 IPC 和独立 Windows 服务；图中的控制路线与独立 HTTPS 模型请求已分开，关闭页面、控制断线和原租约执行语义一致。本附录同步移除了“Agent 必须自行连接云端诊断”的旧建议。

审查快照：

- `build-report.mjs` SHA-256：`acc59c7f8cce62acec30e279de37bf9df6a89e3e3a0fad25d324a21b4ce3349b`。
- `design.html` SHA-256：`d15cfd2b79b0f734d6864ee471738fe1d081c148f8ee2aabe72f3a797db6c952`（128394 bytes）。

结论：本次复审范围内没有阻止方案交付的未解决问题。仍需在实施前确认诊断路由在既有协议中的落点，并实测 GUI 退出后的连接存活、IPC/日志 ACL、安装与升级恢复、共线流控和真实模型闭环。本结论是方案与源码一致性复核，不是产品实现、浏览器视觉或现场验收。
