# SSH 工具交付验证

作者：Ray Zhang。日期：2026-11-02。基准：ARA `90f2b1cdd138129e2d331d89a37d70852e849739`；分支：`codex/ssh-tunnel-labslot-20261102`。

## 实现范围

独立 Go 中转服务、一次性配对、多个被控端、主控结束指定会话、现场 SSH 准备与临时公钥授权、后端管理员凭据重试、统一 Windows EXE。原有 LabSlot 应用和 ARA Rust Core 不变。

## 已执行

| 验证 | 结果与边界 |
|---|---|
| Go 本地夹具 | 12 个顶层测试及4个子例通过；TLS/WebSocket/SSH 实际网络连接，配对竞争、跨主控隔离、码失效、半关闭、目标故障、断线取消、连接上限 |
| Go race / vet | 最终源码 root 再跑 `go test -race -count=1 ./...`，2.100s，通过；`go vet ./...` 通过 |
| 真实 SSH 客户端 | Windows `ssh.exe` 在含空格和 `%` 的目录中登录夹具成功；改变目标主机公钥后拒绝连接 |
| Windows PowerShell 5.1 | 42 项通过；包含真实子进程读中文 JSON、超时操作禁止并行清理、实际离线 OpenSSH 有效配置解析；服务安装分支使用受控替代，不是实际异机安装 |
| Windows/Linux 构建 | amd64 构建通过，交付程序版本 1.0.0 |
| NSIS 编译 | Unicode NSIS 3.04 编译通过，无警告；统一 EXE 约 4.97 MB |
| 静默安装 | 主控、被控 `/S /ROLE=...` 均退出0；安装后逐文件核对 manifest SHA-256，角色及桌面快捷方式一致，程序版本为1.0.0 |
| 静默卸载 | 两种角色均通过；工具程序移除，另放入的测试文件保留；原本机 SSH 服务状态不变 |
| 静默缺少角色 | `/S` 未指定角色时退出2，未安装 |
| 独立云端 | 新 systemd 服务绑定 loopback 50122，TLS 路由 `/ssh-tunnel/`；HTTPS健康200且正文一致 |
| 真实云链路 | opt-in `TestLiveRelay` 1.18s通过：本机受控 SSH 夹具经过实际云端完成配对、两个目标登录、错误主机公钥拒绝、主动关闭；无学校硬件操作 |
| 原服务共存 | 原 API/Web/runtime/SDK 监听进程 PID 未变；原 upstream 和 runtime-routes 配置 SHA未变；API健康200，Web保持原307行为 |
| 独立复审 | Codex `tunnel_review` 完整检查工具目录；OCR preview 34文件，默认排除项人工补读；发现的信号回滚、UNKNOWN清理竞态、PS5编码、SSH路径引用均已修复并回归 |

## 未覆盖及保留事项

- 未在一台全新的现场 Windows 机器上执行真实 SSH 安装，未执行真实异机 SMB/WMI 凭据登录；需要现场条件验收。
- 安装页面可视检查被用户 Esc 停止，未完成截图或高 DPI 验收；静默安装及角色快捷方式已实际验证。
- 未测试系统重启或强杀后的自动清理；工具明确保留回执并提供人工撤销入口，不承诺这类场景必然清理成功。
- 一个本机临时 OpenSSH 配置探针目录的删除被自动审批拒绝，已保留；不在 Git 或交付包中。
- 安装程序未作代码签名。

原始构建、测试、云端前后对照和安装回执保存在交付目录的 `evidence` 子目录。现场身份和管理入口保存在本机知识库，不包含在团队仓库。

文档全仓检查 verify_bootstrap.py 未通过：当前基准已有 38 项不相关的历史断链或外部临时路径缺失，失败路径均未被本次修改。新增 SSH 文档的本地链接已核对；未扩展修复旧文档。

## 1.1.0 直连与回退（2026-11-02）

任务：`01a19c8e-c512-7240-82c0-7bf564b05b84`；实现基准 `6620ad0a9b94d26999b6a74a0ff0847371dac1d1`。本节独立记录本次增量，前文 1.0.0 安装及线上验收不能作为 1.1.0 已上线的证据。

目标是在一次性配对后自动尝试 LAN 和公网 NAT 直连，失败保留中转，无需现场手动获取 IP。实现采用 Pion WebRTC v4.2.26：ICE host/srflx/prflx 候选、STUN、可靠有序数据通道和双方认证的 SSH；不配置 TURN。旧配对 JSON 契约、目标主机公钥校验、共享 16 流限制与会话撤销保留。Reach、ARA Core、LabSlot 业务和现场设备不变。

| 文件范围 | 本次改动 |
|---|---|
| `direct.go`、`hub.go` | 独立有界信令、候选协商、消息到字节流适配、设备身份传递 |
| `controller.go`、`device.go` | 直连路径选择、原授权复用、关闭及后续新流回退；已开始的流不重放 |
| `main.go`、两个启动脚本 | 默认自动尝试、`-direct=false`/`-RelayOnly`、可选 STUN 地址与路径显示 |
| `go.mod`、`go.sum`、许可证及第三方声明 | 固定 Pion 及相关依赖，保留 20 份新增原始许可证 |
| `direct_test.go`、`process_test.go` | 真实 Pion/SSH、受控故障、当前及历史普通 hub 进程验证 |
| 构建脚本、安装器、README 与知识页 | 1.1.0 版本、使用条件、兼容性、生命周期约束及验证边界 |

### 最终源码验证

执行环境：Windows，Go 1.26.5；Go 构建并行上限 8。独立审核修复后执行：

```text
go vet ./...
go test -race -p 8 -v -count=1 -timeout 180s ./...
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tests/windows/Test-WindowsScripts.ps1 -OpenSshPayload <已核验离线OpenSSH目录>
```

| 验证 | 实际结果 |
|---|---|
| Go 模块完整测试 | 23 个顶层测试、12 个子测试通过，17.621s，无 race 报告；`TestLiveRelay` 为 opt-in，本次跳过 |
| 数据不经 hub | 主动占满 hub 的 relay 槽并确认 `ry-forward` 被拒绝，4 路并发各 1 MiB 请求/回复仍完整返回；选择 host/host UDP |
| 传输与权限 | 小至 7 字节读取、16 KiB 消息分片、半关闭、共享 16 流限制、非 owner、双方错误公钥拒绝均通过 |
| 回退 | 禁用直连、真实无响应 STUN、500ms 信令超时覆盖；中转及 stop 仍可用；直接流故障后只为新流回退 |
| 生命周期 | stop 多设备隔离、主控控制连接关闭、hub 关闭均关闭已开始的直连流 |
| 普通进程 | 当前普通 hub PID 15356，经受控 TLS 代理和真实 `ssh.exe`，relay/direct 两条路径的 jump/backend 均登录成功；杀 hub 后直连被撤销 |
| 实际旧 hub | 从固定 `6620ad0` Git archive 构建旧普通 hub，PID 60880；拒绝未知直连 channel 后，真实 SSH 双目标登录与 stop 通过 |
| Windows PowerShell 5.1 | 42 项通过；包含实际离线 sshd 配置解析、中文回执与未知结果不重放，未安装真实服务 |
| 构建 | Windows/Linux amd64 与 NSIS 统一安装器 1.1.0 构建通过；输出到独立版本目录，旧安装包保留 |
| 文档 | 自有文件 staged diff 空白检查通过；上游 mdns 许可证第 3 行原有尾空格按原件保留；全仓 bootstrap 文档检查仍为 38 项历史断链，本次 SSH 文件无失败 |

普通进程测试中，端点函数和 TLS 代理仍在测试进程内。Windows 系统 TLS 验证没有 `SSL_CERT_FILE` 覆盖入口，本次没有为夹具修改系统证书信任或新增生产测试开关。

### 独立审查及修正

Codex `design_review` 完成实施前方案审查和实施后 `6620ad0` 到交付工作区的 diff 审查；应用 `ara-git-review`，OCR preview/rule 成功，默认排除的测试、依赖和安装器人工补审。两项发现均回读确认修复：构建脚本恢复 UTF-8 BOM；hub 协商等待改为可取消并释放转发槽，迟到 channel 关闭。Pion 分片、短读、背压和关闭语义与依赖源码对照；20 份许可证与模块缓存原件哈希一致。最终无未解决阻断代码缺陷。

独立测试执行者为 `direct_tests` 和 `process_validation`，主代理回读关键代码、测试和最终运行日志。Rust Core、模型试验与 OMP 点位验收不适用于本独立 Go 工具；本节不增加任何 Rust/Reach 接受计数。

### 未覆盖与交付位置

- 未执行不同真实公网 NAT、异机 LAN、运营商 CGNAT/UDP 封锁现场验收；当前 host/host 成功不代表公网打洞通过。
- 未执行新版普通 controller/device CLI 在两台机器上的安装后配对；未执行真实 SCP/SFTP 文件工具。大载荷测试证明字节传输，不能替代这些使用流程。
- 旧 hub + 新端已执行；新 hub + 旧 device 仅有协议源码和未知 channel 拒绝语义支持，未运行旧 device 二进制。
- 未更新线上 hub、重启支持会话或接触学校/仪器。新版客户端连接旧线上 hub 时继续中转；需要后续升级 hub 才能使用新增信令。
- 1.1.0 安装器完成构建和载荷清单核对，未重做安装/卸载、GUI、高 DPI 或代码签名验收。
- PowerShell 首次验证误指向不存在的 OpenSSH 目录，退出 1；核对真实目录后 42 项通过。未因该环境错误修改测试或产品。

版本产物和最终提交收据：`D:/Tools/远程ssh/1.1.0-20261102/`，原始日志在其 `evidence/` 下。提交后从干净源码重建，`release-receipt.json` 绑定提交、二进制/安装器哈希及实际覆盖。原 `D:/Tools/远程ssh/RY-SSH-远程支持-安装.exe` 保留。

## 后续线上升级与星渡技能

用户随后授权升级 hub 并创建本机 Agent 技能。线上独立 hub 已从 1.0.0 升级至提交 `121bbce` 的 1.1.0：先确认无活动连接，再替换二进制、原子切换 current、重启该服务并核验运行文件哈希。既有配置哈希及 LabSlot 服务 PID 前后不变。真实云端 `TestLiveRelay` 2.209s 通过，覆盖配对、双目标 SSH、错误主机密钥拒绝和 stop；前节“未升级”是当时状态，不代表本节完成后的状态。

技能名称为星渡 Starferry / `starferry`。本机正本、发现入口和便携控制端已安装；格式验证通过。独立 `starferry_trial` Agent 按技能启动普通 controller，接收新配对码，经真实 hub 连接普通 device 和受控 SSH 夹具，选择 host/host UDP，jump/backend 的 probe 各返回 `RY fixture probe OK`、退出 0。list、quit 及主控临时密钥目录/本地监听清理通过。该结果不代表跨公网 NAT 或现场 SSH 安装验收。

首次普通 device 在主控退出后返回 1，失败记录保留。1.1.1 仅修复 device 结束时 Requests 与 Conn.Wait 的调度竞态，更新客户端包版本；hub 协议不变，线上保留已验证的 1.1.0。独立 `design_review` 回读依赖、产品及测试确认修复范围；`direct_tests` 的六种结束组合修复前均失败，修复后均返回 nil 且活动流关闭，授权失败仍报错。针对性 race 2.908s；主代理 `go vet -p 8 ./...` 和模块 `go test -race -p 8 -count=1 -timeout 90s ./...` 通过，19.153s。线上测试为单独运行；本次模块默认测试仍跳过 opt-in live 项。

部署、首次试用及生命周期日志：`D:/Tools/远程ssh/1.1.0-20261102/evidence/starferry-upgrade/`。1.1.1 的普通进程复验、构建及最终收据保存在 `D:/Tools/远程ssh/1.1.1-20261102/`。新补丁安装/卸载、GUI、异机 LAN、公网 NAT、真实 SCP/SFTP 与现场授权撤销仍未验收。

### 1.1.1 交付复验

从提交 `95e1b2959713ea8bdae7703d0d6bc58dc52d643b` 的干净工具目录生成 Windows/Linux amd64 和 NSIS 1.1.1；主控 42 个、被控 68 个载荷文件 SHA-256 全部匹配。普通 Windows 二进制 SHA-256 为 `1947722457702a7b913391580045d96046b2aa3ce3dd2bda4fcea40dd5bce300`，统一安装器为 `4e707856472c0aec6ffad7c0d57386bbfbbb3ec7a5c3778bc6fee11d7e04f0a1`。

独立 `starferry_trial` 与 `process_validation` 复用测试夹具并领取新码，完成真实 hub 配对、host/host UDP 和 jump/backend 两个 probe。主控 quit 退出 0，被控随后自然退出 0；临时密钥目录、被控 session 目录和两个本地监听均已清理，夹具 58.95s 通过。夹具源码仍为固定 `121bbce`，被执行的普通客户端是上述 `95e1b29` 产物；二者分别记录。脱敏结果见 `evidence/agent-trial.json`、`evidence/starferry-trial-device/`。

最终交付仍只有主开发工作树、`dev` 开发分支和 `main` 主线。另有 25 个原 WIP 文件哈希与合并前一致，知识 INDEX 与合并后保存件一致，均未混入补丁提交。技能在本机全局知识库维护和安装；新聊天自动发现尚未实际验证。

用户随后再次明确要求部署，线上 hub 于 23:17 升至同一 1.1.1 产物。升级前核实无活动连接；只替换二进制和 current，三份配置及原 LabSlot 服务 PID 保持一致。运行进程对应 Linux 二进制 SHA-256 `f0842fdaf5c6336722dac56a3bb639c7230e4288f14461059ee07c5b5dc5ba8e`，健康响应正确。部署后真实云端 `TestLiveRelay` 再次通过，测试主体 1.05s、Go 包总计 1.679s，覆盖配对、双目标登录、错误主机密钥拒绝和 stop。证据为 1.1.1 版本目录的 `evidence/deploy.log` 和 `evidence/live-relay.log`。

新增 [使用与技能安装说明](../starferry-usage.md)，核对 Reach 设计状态、QSpin 固定后端的可跳过流程、Codex/Cursor/Claude Code 官方技能目录及 DeepSeek 由宿主决定的边界。该次补充未修改产品代码或增加自动安装技能功能。

## 1.1.2 安装包内置技能

用户追加要求“至少安装包提供 skills，其他 agent 读取自己装”。提交 `7a5ce024f7f349f47e60c9be84c31784ec7c51f9` 在两个角色的载荷中加入 `skills/INSTALL.md`、`skills/starferry/SKILL.md` 和 `skills/starferry/agents/openai.yaml`。安装器沿用递归安装和逐文件卸载清单；版本升为 1.1.2，Go 隧道逻辑未改变，线上 hub 保持已验证的 1.1.1。

分发技能采用安装包相对路径、默认安装目录及复制时写入的本机路径记录，不携带开发机 D/E 盘路径。说明要求 Agent 在复制前检查同路径和已有同名技能，保留用户修改，再安装到所用宿主的技能目录。安装器本身不修改各 Agent 配置。独立 `design_review` 指出的“先复制、后保护已有内容”顺序问题已修正并回读确认。

技能格式、PowerShell 解析、diff 空白检查及 Windows/Linux/NSIS 构建通过。实际运行统一安装器，分别以 controller/controlled 静默安装到独立自定义目录：两次均退出 0，71 个载荷文件哈希匹配，三份技能文件存在，程序报告 1.1.2。复制技能及本机路径记录检查通过；两次卸载均清除包内 skills，保留另放入的用户文件，产品注册和快捷方式清理，原 SSH 服务状态保持。

安装器 SHA-256：`962d03447dc9e28ff1e18a0a0dfa5b53dfb1734db6ae9b835718bb5ab01d135f`。产物及原始收据位于 `D:/Tools/远程ssh/1.1.2-20261102/`，实际安装验证为 `evidence/installed-skills.json`。本次未运行其他 Agent 宿主、技能自动发现或新的网络配对，也未重复 GUI 和现场 SSH 安装验收；既有网络验证仍对应前文版本和范围。
