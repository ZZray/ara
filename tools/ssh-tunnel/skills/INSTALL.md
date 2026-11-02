# 将星渡技能安装到当前 Agent

作者：Ray Zhang。

星渡安装包已携带本目录。主控、被控两种角色都会安装 `skills/starferry/SKILL.md`；安装器不修改各 Agent 的用户配置。用户可以把本文件交给正在使用的 Agent，让它完成以下本机安装。

## 给 Agent 的安装步骤

1. 从本文件所在的 `skills` 目录确定星渡安装根目录，核实 `../ry-ssh-tunnel.exe` 和 `../使用说明.md` 存在，运行程序的 `version`。记录解析后的绝对路径，不依赖当前工作目录。
2. 根据当前宿主选择下表中的一个目标目录。复制前比较源、目标的实际路径：若相同，跳过复制，只补步骤 4 的本机记录。如果目标已有同名技能，先读内容和路径记录，确定需要保留的用户修改，再按此次更新范围合并，不盲目覆盖。
3. 对尚未安装的目标，创建 `starferry` 文件夹，复制本目录内 `starferry` 的全部内容；已有目标按上一步核对结果合并。不要只复制 SKILL.md，也不要把同一技能重复放进同一宿主扫描的多个目录。
4. 在目标技能目录创建或更新 `references/local-installation.md`，写入核实的本机程序、安装根目录、SSH 客户端和使用说明的绝对路径，保留其他用户记录。只记录路径，不记录配对码或密钥。
5. 验证复制的文件，向用户报告技能路径及检测到的程序版本。按宿主能力刷新技能或在新会话确认 `starferry` 可见。安装技能不需要启动隧道或领取配对码。

## 默认目录

| 宿主 | Windows 用户级技能文件位置 |
|---|---|
| Codex | `%USERPROFILE%\.agents\skills\starferry\SKILL.md` |
| Cursor | `%USERPROFILE%\.cursor\skills\starferry\SKILL.md` |
| Claude Code | `%USERPROFILE%\.claude\skills\starferry\SKILL.md` |
| 使用 DeepSeek 的 Agent | 使用宿主的技能目录，模型本身不定义本地 skills 路径 |

部分既有 Codex 环境使用 `%USERPROFILE%\.codex\skills`；以当前宿主实际扫描目录为准，避免重复安装。Cursor 也支持 `.agents/skills`。自定义目录或项目级技能按宿主配置选择。

目录依据：[Codex](https://learn.chatgpt.com/docs/build-skills)、[Cursor](https://cursor.com/docs/skills)、[Claude Code](https://code.claude.com/docs/en/skills)。网页聊天、云端 Agent 和本地 CLI 的加载位置可能不同。

## 安装后怎么用

告诉 Agent：“使用星渡，配对码是……，帮我……”。Agent 必须有可持续写入 stdin 的终端能力，才能持有交互控制端；只会启动后台进程还不够。技能正文描述的是终端能力，Agent 应映射到自身工具，不能假定所有宿主都有 Codex 的工具名称。

本包只提供技能和安装步骤，不保证每种 Agent 已实测。其他宿主安装后先验证技能发现、程序定位和 `controller` 的 list/quit，再按用户任务使用新配对码。需要 QSpin 后端以外的使用说明时，读取安装根目录的 `使用说明.md`。
