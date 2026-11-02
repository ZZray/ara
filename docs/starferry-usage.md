# 星渡 Starferry：使用与技能安装

作者：Ray Zhang。版本：1.1.1。核验日期：2026-11-02。

本机安装包与线上独立 hub 均为 1.1.1；线上升级后真实配对和双目标 SSH 验证通过。

## 星渡和 Reach 怎么选

目前需要让本机 Codex 等 Agent 登录远端、执行命令和处理文件，优先用星渡。星渡已经有程序、安装包和实际连接验证；Reach 在当前仓库仍是设计文档和源码骨架，没有可运行安装包。

| 比较 | 星渡 Starferry | Reach 当前设计 |
|---|---|---|
| 主要职责 | 配对、直连或中转、提供 SSH 入口 | 远程系统、文件、进程、桌面、产品工具及远端 Agent 子任务 |
| 决策在哪执行 | 本机现有 Agent 决策，通过 SSH 操作远端 | 本机 Agent 可调用远端能力，也可派远端 Agent 持续工作 |
| 远端要求 | 被控程序和目标 SSH 服务 | 计划安装 Reach 远端服务，尚未实现 |
| 更适合 | 命令行运维、日志、构建、文件任务 | 需要桌面交互、现场工具或远端持续 Agent 的任务 |
| 当前可用性 | 已验证本机双端经真实 hub 配对及 SSH 夹具操作 | 文档设计阶段，不能当成已实现能力比较性能或可靠性 |

从协议和已有工具兼容性看，SSH 更通用；从计划覆盖的交互能力看，Reach 更广。可以先用星渡满足 SSH 工作，未来由 Reach 按需复用连接能力。仓库中两者目前没有这种集成。

## 为什么出现 192.168.3.199

这是现有 Windows 被控启动脚本针对 QSpin 跳板与后端的便捷流程，地址目前固定在脚本中，并不是星渡协议必须连接的地址，也不是已经实现了任意地址选择器。

- 只管理现场电脑：在后端用户名提示处输入 `S`，跳过后端，只发布 `jump`。不会向 `192.168.3.199` 安装 SSH。
- 要直接 SSH 登录后端：后端必须已有 SSH 服务，或经授权安装 SSH。已有服务会复用，星渡会话结束后 SSH 服务本身保留，仅撤销本次新增授权。
- 只需要从跳板访问后端现有 QSpin API、共享目录或其他管理接口：可通过这些既有接口完成任务，不必为了星渡再给后端安装 SSH。接口是否满足任务及其认证条件需按现场核实。

更通用的产品入口应把“只连接本机”和“可选后端”分开，地址由用户配置。当前版本仍保留 QSpin 快捷流程；本轮没有修改这部分代码。星渡被控程序运行在现场可联网电脑上，后端不要求安装另一个星渡 Agent。

## 安装和连接

1. 现场电脑运行 `RY-SSH-远程支持-安装.exe`，选择被控端。首次运行按提示准备 SSH；已有服务、主机密钥和配置保留。
2. 不需要 QSpin 后端时，在 `192.168.3.199` 用户名提示处输入 `S`。
3. 现场人员将新配对码交给本机 Agent。被控窗口保持运行。
4. 本机先安装星渡主控程序和技能，然后告诉 Agent：“用星渡连接，配对码是……，帮我查看……”。Agent 启动持续控制端，领取配对码，再用严格校验主机密钥的 SSH 执行任务。
5. 任务完成后 Agent 退出控制端。配对失效后要使用新码；现场强杀、断电后的授权撤销需检查回执，不能只看主控已断开。

安装器有角色选择界面，连接内核使用命令行；技能把本机操作交给 Agent，因此用户日常不必打开主控窗口。首次安装和现场 SSH 准备仍需完成。

## 主控安装会自动安装技能吗

**1.1.1 安装器不会安装技能。** 当前安装器只部署程序、脚本、离线 SSH 载荷、说明和快捷方式。本机 `starferry` 技能是此次单独创建并安装的，不是安装器自动完成的。

技能应是一整个 `starferry` 文件夹，里面至少有 `SKILL.md`。将它放到所用 Agent 的用户级目录；不要只把 `SKILL.md` 直接丢在 `skills` 根目录。

| 本地 Agent | Windows 用户级目录 | 项目级目录 |
|---|---|---|
| Codex，按当前官方文档 | `%USERPROFILE%\.agents\skills\starferry\SKILL.md` | `<项目>\.agents\skills\starferry\SKILL.md` |
| Cursor | `%USERPROFILE%\.cursor\skills\starferry\SKILL.md`；也支持用户级 `.agents\skills` | `<项目>\.cursor\skills\starferry\SKILL.md` |
| Claude Code | `%USERPROFILE%\.claude\skills\starferry\SKILL.md` | `<项目>\.claude\skills\starferry\SKILL.md` |
| 使用 DeepSeek 模型的本地 Agent | 由宿主工具决定；例如 Cursor 使用 Cursor 目录 | 由宿主工具决定 |

DeepSeek 模型/API 本身不规定一个通用本地技能目录。不能仅凭模型名称创建 `.deepseek/skills` 并声称它会加载。Claude Code 的本地目录也不能等同于 Claude 网页版/Cowork 的技能导入方式。

本机已有兼容入口为 `C:/Users/loveu/.codex/skills/starferry`，junction 指向 `E:/doc/knowledge/skills/starferry` 正本；控制端为 `D:/Tools/Starferry/starferry.exe`。新机器建议按上表采用对应版本的官方目录，避免在同一 Agent 扫描的多个目录重复安装同名技能。

本次提供的技能包是 Windows/Codex 已试用版本。复制到另一台电脑后，应核对技能中的程序和参考文件路径：安装器默认程序为 `%LOCALAPPDATA%\Programs\RY SSH Remote Support\ry-ssh-tunnel.exe`，也可保留本机使用的便携目录。技能中 `exec_command`/`write_stdin` 是本次 Codex 工具名称；其他 Agent 需要具备等价的持续标准输入/终端会话能力，并使用自己的工具接口。只有后台启动能力而不能持续写入控制端 stdin 时，不能承诺本技能可直接保持会话。

装好后在新会话确认能找到 `starferry`，再用真实配对码测试。当前只完成 Codex 显式读取技能后的实际试用；Cursor、Claude Code、DeepSeek 宿主及新聊天自动发现均未实测。未来安装器可提供 Agent 选择和自定义目录选项，本轮只给出安装说明，未实施自动安装功能。

目录依据：[Codex 官方说明](https://learn.chatgpt.com/docs/build-skills)、[Cursor 官方说明](https://cursor.com/docs/skills)、[Claude Code 官方说明](https://code.claude.com/docs/en/skills)。

## 已验证与未验证

1.1.1 普通主控与被控程序经真实云端配对，本机 SSH 夹具的 jump/backend 各执行一次 probe，返回正确内容和退出码 0；主控 quit、被控退出、临时密钥和会话目录、监听端口清理通过。Go race/vet、独立代码审查、Windows/Linux/NSIS 构建及载荷哈希核验通过。

本机 host/host UDP 成功不代表不同公网 NAT 已验收。公网 NAT、异机 LAN、真实现场安装和授权撤销、SCP/SFTP 工具、新版安装/卸载 GUI 均仍需实际环境验证。安装器尚未签名。

完整命令和安装说明见 [工具 README](../tools/ssh-tunnel/README.md)；实现与验证历史见 [交付记录](evidence/ssh-tunnel-20261102.md)。
