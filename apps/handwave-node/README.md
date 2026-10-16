# HandWave Rust 被控端

独立运行的电脑 Agent。云端 HandWave 工作台的「本机」聊天窗口直接调用这台电脑：读日志和文件、修改代码、执行命令与编译、下载工具。Windows 运行只需要 `handwave-node.exe`；使用本机已有的编译器和命令行工具。程序不会附带 Rust、Node、Go 或编译器运行时。

## 使用

**0.2.5** 是需要同步升级 Hub 与被控端的安全版本。平台当前下载以安装页为准，
下方 0.2.4 固定链接保留作历史版本与验收记录。公网连接必须
使用有效证书的 HTTPS/WSS；设备通过原有密钥签名证明身份，旧设备授权需要明确
重新批准。Hub 与被控端须同步更新；旧目标和会话不自动迁移所有者。详见
[修补与升级边界](docs/evidence/security-hardening-0.2.5-20261016.md)。

本版 **0.2.4** 的 Windows 独立 EXE 为5,151,744字节（约5.15MB）；
[Windows 包](https://118.25.103.226/ray/releases/handwave-node-windows-1396bffa.zip)、
[Linux 源码安装](https://118.25.103.226/ray/install)。原生终端、产物校验及实际发布
状态见[终端版本验收](docs/evidence/terminal-0.2.4-20261016.md)。Windows需要
Windows10 1809及以上或Windows11。重连和队列的历史验证分别见
[恢复记录](docs/evidence/recovery-0.2.3-20261016.md)、[队列记录](docs/evidence/message-queue-0.2.2-20261016.md)。
安装、工具、压缩、参考记忆、任务清单和目标见
[基础版本验收](docs/evidence/version-0.2.0-20261016.md)；研究依据见
[上下文与缓存效率](docs/research-efficiency-20261016.md)。

执行模式可调用 `task_update` 维护本次 Run 的清单，完成项必须带证据；它不等于
跨会话项目任务数据库。目标模式使用原生持久 Goal 步骤、预算、续跑和待验收状态。
Goal/Job 的动态观察不再拼入固定 system 提示；模型回执记录有来源的正数用量，
缺失或无法区分的零保持未知，不据此承诺缓存命中或费用节省。Auto 仍为随机。

1. 将 `node.conf.example` 复制为 EXE 同目录的 `node.conf`，填入云端地址和 **node 入网令牌**。模型供应商密钥留在云端。
2. 运行 `start-node.cmd` 或 `handwave-node.exe --cwd E:\your-project`。
3. 在 HandWave 设备列表允许屏幕显示的指纹，接入设备。点击聊天框旁的「本机」，使按钮高亮，任务才直接在 Rust 被控端的原生会话执行。
4. 直接提出任务，例如「读取这个日志，定位错误」「修改程序并编译」「下载这个工具到指定目录」。聊天窗口显示模型输出、工具执行与最终结果。

当前云端工作台入口：[HandWave](https://118.25.103.226/handwave/)，统一登录页为 `/ray/login`。网页登录只输入 Ray 平台验证器的 6 位验证码，无需密钥；同一次登录可进入各平台服务。`node.conf` 的 `hub` 填 `https://118.25.103.226/handwave/`，保留这个路径前缀，使设备连接和模型请求进入同一个服务。设备入网令牌用于程序连接，与网页登录验证码分别管理。

云端默认使用 **Auto 随机选择当前可用模型**，不再调用额外模型做智能路由。可手动选择目录内的具体模型。`deepseek-v4.1-flash` / `openai-completions` 有此前的独立实测；本轮 Auto 的真实召回与撤销结果见[云端记忆交付记录](docs/evidence/cloud-memory-20261015.md)，不从 Auto 名称推断具体下游模型。

```powershell
$env:HW_HUB = 'https://118.25.103.226/handwave/'
$env:HW_TOKEN = '<node enrollment token>'
.\handwave-node.exe --cwd E:\your-project
```

文件工具支持绝对路径；相对路径以 `--cwd` 为基准。权限与启动程序的 Windows/Linux/macOS 用户相同。命令不是 OS 沙箱；需要管理员权限的操作仍需使用相应本机账户。程序不会自动安装服务或提升权限；只有显式目标授权才提供受管后台 Job。

## 代理

```powershell
.\handwave-node.exe --cwd E:\your-project --proxy http://127.0.0.1:7890
# Or SOCKS5:
.\handwave-node.exe --proxy socks5://127.0.0.1:1080
```

也可在 `node.conf` 中设置 `proxy=`，或使用 `HW_PROXY`。该配置同时作用于 WebSocket、模型 HTTP、`download_file` 下载与工具进程的代理环境。未显式配置时读取 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`；`NO_PROXY` 可排除本地地址或指定域名。代理需要运行在被控电脑上。命令行下载工具自身仍需支持这些环境变量，优先使用内置 `download_file`。

## 会话与取消

默认独立数据目录为 Windows `%LOCALAPPDATA%\handwave-rust-node`，Linux/macOS `$HOME/handwave-rust-node`。用 `--state-dir` 指定其他目录。每个控制器、工作目录与明确的网页会话分别保存历史；旧客户端未发送会话标识时保留原路径。不读取或替换原 Go 被控端身份。

`--identity` 显示指纹且不连接网络。同目录只能运行一个实例。模型与工具历史持久化，重启后继续；重复任务 ID 不自动重放。若进程意外终止且副作用未知，下一次任务会明确报告未完成状态。先检查原会话和文件结果；网页明确新建会话会使用独立原生记录。新会话不能证明旧副作用已撤回，不要删除历史来假装恢复。

云端停止会取消当前任务并清理其命令进程树；控制器离线或网络断开时，普通任务同样清理。只有明确授权且未到期的后台目标可在关页后继续；Node 断线后仍需重新核验授权，未知操作不自动重放。取消前已经完成的文件修改会保留。云端「转向」在工具间隙注入，后续问题排队；不会同时运行两个模型任务。

任务执行时可继续发送消息，在输入框上方排队；支持删除、编辑、移到最前，或「引导」当前任务。操作以电脑回执为准，已开始的消息不能再按待发消息删除。引导分别显示已提交、已加入上下文或未应用，不表示模型已经照做。队列最多 16 条，仅存在于当前连接；断线会清空，不会自动重放。详见[消息队列边界](docs/knowledge/message-queue.md)。

单任务默认最多 24 次模型请求、600 秒，并按收到的短期访问码 TTL 缩短；使用 `--max-calls` / `--max-seconds` 调整。任务超过授权时限会明确停止，不盲目续码重放。上下文摘要也计入同一任务预算。文本文件完整读取限 1 MiB，大日志可用 `read_file` 的 `offset` / `limit` 分段读取或 `tail` 读取末尾；命令输出限 128 KiB。下载限 128 MiB；Web 文件上传限 16 MiB，下载按确认帧流式传输。

## 本版新增能力

**0.2.4** 增加同一进程的管道输入与原生终端：`job_start`
使用 `terminal=true` 创建 Windows ConPTY / Unix PTY；输出合并到 stdout，支持
连续键盘输入、Ctrl+C、终端 EOF、尺寸变化与进程树停止。`interactive=true`
保留管道方式，适合普通行输入。解释器仍来自被控电脑。

工作台「进程控制台」默认折叠，先只观察；回放完成后点击「接入终端」才接收
键盘和粘贴。当前终端接入时每150ms有界刷新，输入ACK后立即读取，不调用模型。
控制台直接输入不进入聊天；终端或程序自身仍可能回显到输出日志。
断线、切换任务或未知ACK立即脱离输入，只核对原操作编号，不自动重发。

终端常用 DSR/DA 查询由活着的 worker 实时处理，浏览器回放不会向进程补发
历史查询响应。具体支持集、跨平台执行证据与发布状态见
[0.2.4 原生终端验收](docs/evidence/terminal-0.2.4-20261016.md)。
此前[管道阶段记录](docs/evidence/interactive-0.2.4-20261016.md)保留作历史证据。

长任务版本的 Windows 独立 EXE 为 **4,899,328 字节（约 4.90 MB）**，SHA-256
`a38da90708ca929780438d9e0c00c935a2018e71aa0ea1a99d254200968c7db1`。
原始历史按分段日志保存，模型可按需检索旧收据；目标在明确选择后台授权后可
分片继续，托管 Job 等待期间不消耗模型调用。网页提供暂停、继续、停止和
待验收状态；后台授权默认关闭，最长时间、调用次数和分片数固定受限。
生产和公开下载状态以[本轮交付记录](docs/evidence/long-task-runtime-20261016.md)
及 [Ray 安装入口](https://118.25.103.226/ray/install)为准。以下版本说明保留为历史。

最新可审经验增强：Windows EXE **4,602,880 字节（约 4.60 MB）**，比上一轮
增加 35,840 字节，SHA-256 `c3ff87a704d89b81bf4584057757efdd4be612d58583df185f4cf308326f1059`。
实际工具失败、恢复后，Agent 可通过 `experience_propose` 提出带适用条件和
真实工具收据的经验候选；`/memory list` 查看、`/memory accept ID` 采纳。
后续 `experience_search` 只返回已采纳且来源当前有效的经验，最多 3 条、
完整 JSON 最多 8 KiB。撤销或更正来源后失效。候选不会自动采纳；不自动
修改程序、权限或 Skills。经验当前沿用本原生会话范围，不自动跨设备或
网页会话共享。见[完整验收](docs/evidence/evolution-20261015.md)。

上一轮编码／排障增强包：Windows EXE **4,567,040 字节（约 4.57 MB）**，
比上一版增加 48,128 字节；SHA-256 `080bdb85771c68d297752b1a8887c2f78bb984519c82881e1b5d50f3383f05d3`。
新增 `find_files`（文件名／路径定位）、`system_info`（OS 与解释器候选）、
`run_script`（Python／PowerShell／cmd／sh）。脚本使用本机解释器，默认输出
8 KiB 并保留 stdout/stderr 首尾；错误、取消、超时均有明确收据。内容搜索
现在返回长行命中附近的摘录。完整边界和实际模型修复任务见
[本轮交付记录](docs/evidence/problem-tools-20261015.md)。桌面 UIA、截图与点击
闭环尚未实现；本轮包不会自动替换已运行的节点。

以下云端下载链接仍对应此前已发布的云端记忆版本，不能据此取得本轮新包。

2026-10-15 版增加云端显式记忆和独立网页会话。Windows 独立 EXE 为 **4,518,912 字节（约 4.52 MB）**，SHA-256 为 `40669969a4b84074674f767b38180901293d9adbeebb8263347ed3c3d4729f73`。最终 Windows 进程、Linux x86_64 GNU 源码构建、线上真实模型召回/撤销均有分层证据；[最终交付记录](docs/evidence/cloud-memory-20261015.md)同时保留失败和未覆盖项。此前的[上下文切片](docs/evidence/context-memory-20261014.md)属于对应历史版本。

下载入口：[Windows 独立包](https://118.25.103.226/ray/releases/handwave-node-windows-40669969.zip)、[Linux 源码安装](https://118.25.103.226/ray/install)。源码脚本保留已有配置与设备记录；首次设备仍需入网配置和审批。ARM、musl 和 macOS 不在本轮运行验收范围内。

当前公开 tar 包面向 Linux 运行时源码构建，未包含全部交付 evidence、Windows 打包和许可证收集脚本。需要完整构建／验证入口时使用本 Git 目录；固定下载包不会因文档更新而覆盖原有校验值。

- **代码和日志**：`search_files` 进行字面文本搜索，可选择大小写；跳过生成目录、链接和非文本文件。搜索与分段读取会报告扫描或输出不完整，不能把零命中视为全项目不存在。`tail` 的行号表示距文件末尾的行数。
- **项目规则**：每次任务读取 `--cwd` 下的 `AGENTS.md`，最大 16 KiB，保留来源路径和内容哈希。当前用户要求优先；子目录的规则由 Agent 按需读取，父目录不会自动合并。非法 UTF-8、超限或读取失败会明确报错。
- **工作上下文**：达到字节调度阈值后，对旧的完整轮次生成摘要，保留最近上下文与当前请求。摘要带来源，原始会话和工具收据保留。此阈值不是精确 token 计数，也不保证适配所有模型窗口。
- **独立用户参考**：用户请求、上下文 brief、转向文本分别保存；模型可按来源 ID 读取原文或检索。更正产生新版本。当前支持文本，附件二进制的独立归档尚未实现。
- **记忆提取**：模型可以提出事实、偏好、决策和经验候选，并附来源。候选不会自动成为已采用记忆。共享平台目前按控制器和工作目录隔离，尚不是按个人用户隔离。
- **云端显式记忆 M1**：右侧「云端记忆」支持添加、编辑和撤销，保留版本与原件。默认本机隔离，可显式加入共享项目或使用 Ray 工作区共享；共享验证码不是个人身份。每次新请求获取有界当前快照，正文不写入本地原始用户消息。撤销对后续新请求生效，不即时中断运行或排队任务。离线双向同步、自动整理和向量检索尚未实现。
- **输入建议与会话归属**：网页输入框按上下文显示简短 placeholder，Tab 只填入、Escape 忽略，不增加模型调用。新网页会话使用独立原生历史；切换会话后的迟到输出不会写入另一会话，脱离页面的记录保留在被控端审计中。

在本机聊天中单独发送精确命令即可管理，无需模型执行：

```text
/reference list
/reference correct ref-1 更正后的原文
/reference revoke ref-1
/memory list
/memory accept mem-1
/memory revoke mem-1
```

ID 以实际列表为准。管理命令应作为新请求发送；放在引用文件、brief、工具输出或执行中的转向消息里不会获得管理权限。撤销或更正会清空该边界之前的工作视图，避免旧回复复述已撤销内容；这会同时移除那段视图中的其他上下文。原始会话仍作为审计记录保留，当前有效参考可重新按需读取。

原始历史现按分段日志保留，近期检查点与模型上下文分开；模型可用 `history_search` / `history_read` 按哈希和范围查询旧收据。近期维护阈值为128条／2MiB，硬边界为512条／8MiB；日志、参考和记忆仍有明确容量上限，不自动删除原文。旧版超过8MiB且没有足够有效摘要覆盖的检查点需要先核验迁移。实现合同与恢复边界见[长任务运行机制](docs/knowledge/long-task-runtime.md)。

模型由当前 HandWave Hub 的共享 Gateway 提供。空/default 使用 Hub 下发的真实默认模型，显式模型与协议要在目录中存在；多个协议需明确选择。当前精简端支持 `openai-completions` 和 `openrouter-chat`，其他协议明确报错。智能模式按 Hub 下发的完整绑定执行并保存会话身份；不允许静默换模型、协议或绑定。若 Hub 未配置 Gateway，设备仍可上线，但模型任务会报告配置错误；只保留旧 `upstreams.conf` 不会让现代 Hub 的模型路由生效。

旧 Hub 的 smart 模式兼容仍保留：断连会撤销其服务器绑定，恢复后失效授权必须明确拒绝。当前部署使用随机 Auto。无论哪种模式，同一原生会话都不会在模型、协议或绑定不一致后静默重置历史；需要新任务上下文时在网页明确新建会话。

## 构建与验证

Host 强制区分计划／目标模式：计划只开放显式只读工具，包括历史检索；目标
保存步骤和完成标准，正常结束停在待验收，由用户确认完成。后台目标的验收还
校验目标版本与被审阅的 Run，旧卡片不能确认后来的结果。
忙碌跟进继承当前权限和目标，不会改成执行模式。前轮 Windows 独立 EXE
4,650,496 字节（`f893522a…`）已与全局／项目记忆联想、@会话、@机器和
经验云副本启用，见[生产验收](docs/evidence/production-activation-20261015.md)。
本轮增加分段原始历史、有明确授权的目标分片续跑和托管长进程。故障、真实
模型试验和发布状态以
[本轮证据](docs/evidence/long-task-runtime-20261016.md)为准。

模型能力按添加／编辑时的云端配置读取，包含上下文、最大输出和输入类型。
已声明1M的模型采用更大的文本工作预算，完整请求计入系统提示和工具声明；
容量缩小不能装下时明确报错并保留会话。预算以保守文本字节估计，不等于实际
token消耗。模型支持图像与被控端实际传入图像是两层能力，目前被控端仍为文本
输入。参见[模型能力验收](docs/evidence/model-capabilities-20261015.md)。

此目录是独立 Cargo workspace，避免构建完整 ARA/OMP 对齐工程。只复用固定版本的 rpi Agent/AI 库，不引入 rpi CLI、TUI、插件或图像工具层。

```powershell
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
./scripts/build.ps1
```

在 Linux/macOS 上可以直接 `cargo build --release --locked`；跨平台实测结果以交付证据为准。Windows 包使用静态 CRT，构建脚本只打包 EXE、说明、许可证和配置模板，排除真实令牌、设备身份与历史。

实际本地 Go Hub + 受控模型 + Rust 被控端验收：

```powershell
python -B tests/verify_host.py --binary <handwave-node.exe> --hub-binary <isolated-handwave-hub.exe> --proxy
```

受控模型验证协议及真实本机效果；真实云端/模型结果另记录，不能用此测试替代。

云端快照与明确网页会话的进程回归入口为 `tests/verify_cloud_snapshot.py` 和 `tests/verify_native_sessions.py`，均使用相同的 `--binary`、`--hub-binary` 与独立 `--output` 参数。`tests/verify_cloud*.py` 中真实云端入口只在已有明确授权时执行，不因跑本地测试而自动连接生产。

## 范围

兼容现有 HandWave WebSocket relay、设备审批、短期 model-grant、agent-loop/steer/follow/abort、exec、agent-task、put/get/ack。当前不含 WebRTC、MCP、插件、托盘、服务安装和完整 OMP 对齐。原 Go 节点和完整 ARA Rust Core 工程保持各自交付边界。

