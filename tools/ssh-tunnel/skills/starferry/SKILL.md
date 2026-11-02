---
name: starferry
description: 使用星渡配对码连接远端电脑，通过 SSH 执行用户指定的命令和文件任务，并管理连接及清理。用于现场远程支持；不用于部署 Reach Agent。
---

# 星渡 Starferry

作者：Ray Zhang。

星渡提供一次性配对和 SSH 连接。现场人员先启动被控端、完成 SSH 准备并提供配对码；本机 Agent 负责启动控制端和执行用户任务。

## 找到本机程序

按下面顺序定位，确认文件存在后运行 `version`：

1. 若本技能目录有 `references/local-installation.md`，读取其中记录的程序和说明路径。这是安装本技能的 Agent 根据实际机器写入的本机记录。
2. 若正在安装包内读取本技能，程序在本技能目录上两级的 `ry-ssh-tunnel.exe`，使用说明在同一安装根目录的 `使用说明.md`。
3. Windows 默认安装位置为 `%LOCALAPPDATA%\Programs\RY SSH Remote Support\ry-ssh-tunnel.exe`。PowerShell 用 `$env:LOCALAPPDATA` 展开，不把 `%LOCALAPPDATA%` 当成已展开的字符串。

自定义安装位置优先采用用户提供的目录或本产品卸载注册项 `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel` 的 `InstallLocation`，核对实际文件后使用。找不到时报告缺失，不下载来源不明的同名程序。

SSH 客户端优先使用安装根目录的 `payload\OpenSSH-Win64\ssh.exe`，也可使用本机已安装的 OpenSSH。SCP/SFTP 同理。默认云端为 `https://nmrkit.ciqtek.com/ssh-tunnel`；用户指定其他服务时传 `-server`，保留 TLS 校验。

## 保持控制端会话

控制端必须由可持续写入标准输入的终端工具持有。使用宿主 Agent 实际提供的工具，保存终端句柄；工具名不属于星渡协议。Codex 的示例是 `exec_command(tty=true)` 启动，再用 `write_stdin` 写入同一 `session_id`。

```powershell
# $starferryExe 为上一步核实的绝对路径。
& $starferryExe controller
```

不要用 `echo ... | controller`，管道 EOF 会结束支持会话。只有后台启动能力、不能持续写入 stdin 的宿主无法直接使用这个交互流程；明确报告工具缺口，不声称连接已保持。

1. 等待 `ry>`，向同一终端写入 `add 配对码` 加实际回车换行。码作为控制端输入，不作为 shell 命令；不保存到技能或长期日志。
2. 被控授权最多约三分钟，之后直连协商最多约十五秒。等待期间读取原会话输出，不重复发送 add。失败或结果未知时先看错误和 `list`，不自动重放一次性码。
3. 成功后记录设备 ID、序号、名称、Path，以及每个目标的用户、本地端口和完整 SSH 参数。`jump` 是现场被控电脑，`backend` 仅在现场配置后出现。按设备 ID 对应，不假定序号总为 1。

`direct UDP` 表示当前数据直连，`relay` 表示云端中转。host/host 或 STUN 候选本身不能证明跨公网 NAT 打洞已验收。控制连接断开也会结束直连。

## 操作远端

保持控制端运行，在另一个终端调用 SSH。把生成参数解析成参数数组，保留身份文件、`IdentitiesOnly=yes`、`HostKeyAlias`、`UserKnownHostsFile`、`StrictHostKeyChecking=yes`、端口和用户。不要 eval 控制端或远端输出。

额外使用 `-o BatchMode=yes -o ConnectTimeout=10`，避免等待密码或主机确认。例如 `$sshArgs` 已包含目标地址时：

```powershell
& $sshExe -o BatchMode=yes -o ConnectTimeout=10 @sshArgs 'hostname'
```

终端长行可能带 ANSI 光标码和折行。根据同一设备元数据、密钥路径和公共选项核对参数，不复制显示控制码。无法确定参数时重新读取 `list`。

配对授权建立连接，远端操作范围由用户任务决定。只有配对请求时，可读取 hostname/whoami 确认身份；不要自动部署、重启、删除或操作仪器。按远端实际 shell 引用命令，超时或断线后先核实副作用，不自动重放写操作。

不要用控制端的交互 `ssh 序号 目标` 子菜单自动化，它会接管同一终端输入。文件传输复用同一身份和主机密钥约束；SSH 端口选项是 `-p`，SCP 是 `-P`，不能原样复用整条 SSH 参数数组。核验源、目的和传输结果。

## 接续和清理

- 同一会话继续操作时，复用终端句柄并先发 `list` 核对设备。可以记录句柄、设备 ID 和临时密钥路径，不读取或展示私钥内容。
- 用户明确结束任务时发送 `quit`；结束单台设备用 `stop 序号`。用户要求保留连接时说明最近一次配对成功后最多支持十二小时，控制连接提前关闭则结束。
- 等待控制端退出，核对其临时密钥目录和本地监听已清理。主控清理成功不代表现场授权文件撤销已确认，现场脚本异常时按回执处理。
- 隧道进程退出 0 只表示生命周期收尾，不能证明远端命令成功。分别检查命令退出码及输出。
- 新聊天没有原终端句柄、控制端已退出或配对失效时，不能用旧码静默重建。先确认现存状态，再按用户任务使用新配对码。

## 已验证的范围

普通 Windows controller/device 经真实 hub 配对、本机 SSH 夹具双目标 probe、host/host UDP、退出及临时目录/监听清理已通过。Codex 显式读取技能的流程已试用；其他 Agent 的工具适配、新会话自动发现、跨公网 NAT、现场 SSH 安装与授权撤销、SCP/SFTP 仍需实际验证。
