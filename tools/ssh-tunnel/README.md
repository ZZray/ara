# RY SSH 远程支持

作者：Ray Zhang

主控端通过一次性配对码连接被控端，可同时管理多台被控电脑。被控端提供本机和指定后端的 SSH 入口。云端只转发连接，以独立进程运行，不使用 LabSlot 的账号、数据库或应用接口。

## 安装和选择角色

运行 `RY-SSH-远程支持-安装.exe`，选择角色并安装。程序放在当前用户的应用目录，桌面快捷方式启动所选角色；开始菜单也保留两种角色的入口。

| 角色 | 在哪运行 | 操作 |
|---|---|---|
| 主控端 | 支持人员的 Windows 电脑 | 启动主控端，输入现场给出的配对码 |
| 被控端 | 能联网、也能访问后端的现场 Windows 跳板机 | 启动被控端，允许管理员提权，按提示填写 |

同一个 EXE 可复制到两端使用。也支持命令行静默安装；`/S` 区分大小写，静默模式必须指定角色。

```powershell
.\RY-SSH-远程支持-安装.exe /S /ROLE=controller
.\RY-SSH-远程支持-安装.exe /S /ROLE=controlled
```

静默安装不启动隧道、不安装 SSH 服务，也不索取密码。安装完成后从桌面或开始菜单启动，按现场提示配置和配对。需要等待安装结束并读取退出码时，可用 `Start-Process -Wait -PassThru`。卸载程序只移除本工具的安装文件和快捷方式，保留 SSH 服务、会话回执及用户另外放入的文件。

启动时可以输入云端地址，直接回车使用 `https://nmrkit.ciqtek.com/ssh-tunnel`。自建服务需要有效的 HTTPS 证书；不提供跳过证书验证选项。

被控端先检查本机 SSH。已有服务会保留配置和主机密钥；缺少服务时使用包内离线 OpenSSH 安装。后端默认是 `192.168.3.199`，在现场窗口输入它的管理员账号和密码。密码只用于这次远程管理，不写进包或配置，输入失败可以重试。

被控端给出配对码后，将配对码发给支持人员。主控窗口输入 `add 配对码` 添加设备，`list` 查看连接信息，`ssh 序号 jump` 登录跳板，`ssh 序号 backend` 登录后端。每台设备的本地端口和 SSH/SCP 参数由主控窗口列出，端口只监听 `127.0.0.1`。

输入 `stop 序号` 结束指定被控会话。输入 `quit` 结束本次主控管理的所有会话；被控端也可以在现场结束程序。再次连接时重新启动并配对。

## 后端还没有 SSH 时

从跳板安装 SSH 需要后端已有 Windows 远程管理能力：管理员共享和 WMI/DCOM 可访问，且账号具有相应权限。账号密码正确也不代表这些通道可用。

脚本不会关闭 UAC、改动全局 TrustedHosts 或扩大现有防火墙规则来绕过限制。如果远程管理不可达，窗口会说明原因，可先选择只连接跳板。现场也可在后端安装同一个 EXE，从开始菜单运行“仅安装后端 SSH”，提前准备本机服务；这一步不修复 SMB/WMI、不授予主控公钥，也不单独开放来自跳板的网络访问。后续接入仍需现场管理员配置可用的管理通道，或者在可联网的后端直接启动被控端。

远程安装已开始但没有拿到结果时，先查询安装回执和服务状态；结果未知时不重复覆盖安装。

## 密钥和结束后的状态

包中没有通用私钥或旧现场授权公钥。每次会话生成独立密钥，配对码短期有效且只能领取一次。云端重启或连接断开会使本次配对失效。

目标 SSH 主机公钥由现场脚本读取并交给主控，主控按该公钥验证目标；公钥不符时拒绝连接。外层中转使用系统信任的 HTTPS 证书。

脚本只追加本次主控的公钥。Windows 管理员通常共用 `administrators_authorized_keys`，该文件中的授权适用于读取它的管理员账号；自定义 SSH 配置以现场有效设置为准。正常退出时移除本次新增的授权。隧道关闭与授权文件清理是两件事：现场进程被强制结束或机器断电时，清理可能未完成，需要检查现场回执。

SSH 安装会保留在现场电脑上，已有 SSH 服务、主机密钥、其他授权和业务程序不会随会话结束而卸载。隧道程序只在本次窗口运行，不注册开机任务或后台常驻服务。

## 构建和验证

需要 Go 1.26.5、Windows PowerShell 5.1、Unicode NSIS，以及经核对的离线 OpenSSH 原包。先构建双端内部载荷和 Linux 服务二进制，再将完整被控端载荷封装为统一 EXE。构建脚本校验 OpenSSH 原包 SHA-256，并生成文件清单和校验文件。

```powershell
.\scripts\build-packages.ps1 -Go 'E:\tools\go1.26.5\go\bin\go.exe' `
  -OpenSshArchive 'C:\Users\loveu\tools\qspin-ssh-relay\OpenSSH-offline-for-C.zip' `
  -OutputDirectory 'D:\Tools\远程ssh'

.\scripts\build-installer.ps1 `
  -PackageDirectory '<上一步输出的 build 目录>\被控端' `
  -MakeNsis '<NSIS目录>\Bin\makensis.exe' `
  -OutputDirectory 'D:\Tools\远程ssh'
```

验证记录保存在仓库 `docs/evidence/ssh-tunnel-20261102.md`。网络夹具测试、真实云端连接和真实 Windows 安装分别记录，不互相代替。

## 云端部署

`deploy/ry-ssh-tunnel.service` 以独立低权限账号启动服务，监听 `127.0.0.1:50122`。`deploy/ry-ssh-tunnel.nginx.inc` 将 HTTPS 的 `/ssh-tunnel/` 交给该服务。部署脚本只面向已确认的现有 TLS 虚拟主机，检查、备份配置后增加一条 include，验证 Nginx 配置再 reload。

将 Linux 二进制命名为 `ry-ssh-tunnel`，与 `deploy/` 中三个部署文件放在同一临时目录。以 root 执行：

```bash
bash install.sh <binary-sha256> <release-id>
```

脚本打印备份路径和版本。部署失败会尝试恢复本次改动；应检查输出和实际服务状态后决定下一步。停止中转用 `systemctl stop ry-ssh-tunnel`，这会结束经过该服务的所有会话。服务不持久化配对关系，重启后需重新配对。
