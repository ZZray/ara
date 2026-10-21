# HandWave Node 0.2.9 与 ara-agent-lite-core 发布

2026-10-21。用户授权提交、推送、部署，并要求给其他 AI 的 Core 接入说明。
运行库保持 `0.1.0`，被控端发布为 `0.2.9`，没有增加本轮提取之外的 Agent 功能。

## 代码与接入

- Rust 运行库与被控端：`0e4d81a91682196e9ef8be4a5ffae469ddd99bcc`，已推送 GitHub `ZZray/ara` 的 `dev`。
- 下载与源码安装入口：`0b9cb7c3eabf1e4d82bfc1d6147786c2d883721c`，已推送原 Go 宿主仓库的 `dev/rust`。
- [给其他 AI 的接入说明](../../../../crates/ara-agent-lite-core/docs/ai-integration.md)包括固定版本、Cargo 依赖、宿主授权与路径、业务工具、并行事件消费、知识／记忆、Worker 和验收模板。
- [发布收据](receipts/stage-publication-20261021/publication.json)记录产物、执行范围和证据哈希。提取阶段的原报告与失败记录保留其时点含义，不改写为本次新验收。

Rust 提交仅含 625 个指定路径，完整版 ARA／OMP 和原有七条应用 WIP 未纳入。
两份共用知识文档只提交 Core 链接和 HWN-037；工作区原有观察记录与 HWN-036 保留。
Go 提交仅含九个指定路径，37 份既有 WIP 的字节保持。

线上 Ray 还包含此前独立上线的 Shift 入口，原 Go HEAD 缺少其四份源文件。
先以固定 HEAD、已提交的稳定 Web 资源及这四份现有源文件重建，所得二进制与线上
`22adf71e…` 精确相同，再更新下载包。该源归档用于保留现有入口，没有重新设计 Shift。
最终两个提交的导出分别与 1237 份包内源文件、1429 份 Go 构建输入逐字节匹配。

## 产物

| 产物 | 字节 | SHA256 |
| --- | ---: | --- |
| Windows 独立 EXE | 5,408,256 | `8641836c793180f2686b96877f71a12700d3729178cb771c44ff09b9bcf37fea` |
| [Windows ZIP](https://118.25.103.226/ray/releases/handwave-node-windows-8641836c.zip) | 3,878,855 | `fb92da81b807bd8af72db7fcf6c10dc93802566dd402446e3e58c4b3c85fa7df` |
| [源码包](https://118.25.103.226/ray/releases/handwave-node-source-c66c50a8d91d0a3c.tar.gz) | 810,614 | `c66c50a8d91d0a3c007ab3e243ded96079a759b7e2fea3c00bf855420db2b531` |
| Linux 源码安装脚本 | 7,137 | `4ecd9f615c28aab4c5867be278b4c1ed1bd40c5ed42c167763fddfc1cae4e303` |
| 线上 Ray 镜像 | 69,101,381 | `72c4eff9bd3f747ff9c111dbdbb53803816683634e4818ab4df453c4ef61a188` |

源码包采用 `handwave-node/apps/handwave-node` 加 `handwave-node/crates/ara-agent-lite-core`
布局，保留真实相对依赖、编译期资源、接入说明、格式配置和许可证。
`/ray/install.sh` 已固定本次源码 URL 与校验值。原 26 个公开下载保留，本次新增三个，
29 个地址均经线上校验；新包响应的附件文件名也与各自 URL 一致。

## 本次执行与已有证据

- Windows：两个独立 workspace 的 fmt、clippy、208 项 unit／public API 测试通过，公共联网测试一项 ignored；最终 EXE 的 27 组实际 Host 回归和独立宿主的 12 组检查通过。
- GNU Linux x86_64：实际新源码包以非 root 账户构建 debug Node；14 项知识、12 项技能、两项 Run 预算测试及五组真实 Node／Hub Host 用例通过。它不是 Linux release/LTO、全 Linux suite、其他架构或系统服务安装验收。
- 两个交付清单的离线 `cargo deny` 检查通过。审计库是否最新未确认；固定上游提交、两项 TLS 补丁及消费者版本下界保留。
- 提取阶段的 47 份运行时 Rust／提示词文件与本次交付字节相同。原六套 59 组回归、四组旧版本升级、真实模型四次调用／17.166 秒的读写任务可作为该范围的来源证据；本次没有把它们重复计为新运行或新模型试验。

独立 Codex reviewer `/root/lite_core_architecture` 审查了接入说明、实际暂存范围、
已部署源基线、下载逻辑及部署控制。发现两个新下载分支仍带旧版附件文件名，
修复后新增所有新旧 GET／HEAD 的响应头断言，再运行 Go 包测试、vet 和构建。

首次独立导出缺少仓库根的格式配置，fmt 未通过；两个独立项目随后携带与原配置相同的
`rustfmt.toml`，新导出检查通过。Linux 首次离线构建因缺少两项 TLS 补丁缓存而失败，
按锁文件取得依赖后在新目录重验。最初预算筛选匹配零项，不计作测试通过；另用
`run_budget` 实际执行两项。原日志均保留，没有为通过而降低验收条件。

## 部署与保留状态

云端只替换并重启 Ray。实际运行镜像与上述新 SHA 一致，63 份配置、认证、历史和 lease
文件保持；Hub、OMP 入口／Gateway、RainFlow、Shift 与 Arena 不重启、不换镜像，
Shift 当前静态版本保持。正常 HTTPS 校验通过；未登录应用、Shift 和头像仍进入登录门。
独立 Windows 客户端还下载并逐项校验新 ZIP、源码包、脚本、附件名称与默认安装 pin。

本机安装的 EXE 已更新为 0.2.9。原 126 份 state／启动文件的完整清单与哈希、
设备身份均保持；本机原本已停止，因此保留停止状态。没有续设备授权、启动保存的任务，
或恢复整份用户状态。二进制专用备份与本机原始收据位置留在私有本机知识库。

本轮没有生产模型任务或新的浏览器视觉验收；“新会话自主读取沉淀知识正文”仍为
FAIL／OPEN，也没有通用自我进化或 token／费用节省的可比证明。根仓库文档 bootstrap
此前超时仍为 NOT_COMPLETED，本轮文档做限定范围链接与 Git 空白检查。
