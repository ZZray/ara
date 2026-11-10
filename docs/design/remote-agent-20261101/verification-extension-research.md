# Computer Use 与 QSpin UI 验证接入调查

1. **范围与版本。** 2026-11-01，只读核对 `E:/repos/qspin` dev1，HEAD `367090d213e3deb1a7d6cdf4b93834a6e02eae31`。`QuantumSpin/tools/qspin_ui_sdk/pyproject.toml` 已修改，`qspin_ui_sdk/v3_agent/`、两个 V3 入口和 `tests/unit/test_v3_agent.py` 为既有 untracked；Gateway `internal/uiv3/`、`internal/hostv3/ui*.go` 等也属于既有 WIP。以下是当前工作区源码支持，不能归为 HEAD 已发布能力。本轮没有执行桌面动作、截图客户画面、启动产品或设备。图索引为 2026-10-31 fast；关键文件 metadata_changed，结论均回读实际源码。

2. **现有 UI 自动化可复用。** `qspin_ui_sdk/README.md:3` 声明 pywinauto 纯 GUI 路径；包中已有 lifecycle、driver、pageobjects、actions、capture 和 MCP。`qspin_ui_sdk/mcp/server.py:132` 创建 FastMCP，`:144` 起将 action catalog 反射成 `action_*` 工具；`mcp/tool_adapter.py:57` 将 ActionResult 转为 JSON 文本。因此不必为“通过 AI 看 QSpin 并验证”从零再造通用 UI helper。业务 API MCP、旧 UI Action MCP、V3 私有 UI Agent 是不同入口，不能因都叫 MCP/Agent 就混成一个权限面。

3. **旧 UI MCP 不能直接整体远程开放。** 旧 `driver/widgets.py:55`、`:142` 的点击先用 UIA Invoke，异常后会回落 `click_input`；`:201` 的下拉选择存在键盘输入路径。`mcp/_attach.py` 使用主窗口 class，失败时遍历可见窗口再附着；不是 V3 精确 PID/启动代次绑定。`capture/screenshot.py:13` 使用 `ImageGrab.grab(bbox=窗口矩形, all_screens=True)`，屏幕遮挡可能进入所取像素。若为旧版本兼容而使用，需单独的选定工具清单、准确目标绑定、人工操作授权与截图处理，不能原样复用整个 catalog 的权限假设。旧 action manifest 还明确未保存 result 正文（`actions/_manifest.py:68` 附近），不能当完整结果证据账本。

4. **优先复用现有 V3 桌面 Agent。** `v3_agent/protocol.py:8–9` 定义 16 个 `ui.*` 方法及两个私有 agent 方法，已有 profile/session/window/observe/elements/recipe/action/screenshot。`v3_agent/README.md:11` 与 Gateway `internal/uiv3/client.go:123–168` 表明该 Agent 由 Gateway 以固定 Python、`-I -u`、匿名 stdin/stdout pipe、无 shell、隐藏窗口方式创建并持有，不监听 TCP，也不拥有第二套持久 operation 账本。Remote Agent 应调用既有 Gateway UI 能力，让 Gateway 保持认证、准入、dispatch marker 和最终 operation 归属，避免另开一个可绕过其准入的 UI 端口。

5. **Windows 服务不能直接承担桌面动作。** `v3_agent/windows_identity.py:118–122` 拒绝 Session 0；`:164–179` 逐次检查目标、父进程和自身具有同 SID、同非零 Windows session，并要求当前窗口站/桌面及 input desktop 是 `WinSta0\\Default`。Gateway `internal/uiv3/process_windows.go:87` 也要求父 Gateway 与目标 QSpin 同 SID/非零 session。故 Session 0 的 QSpinRemoteAgent 服务负责模型、日志和任务；交互用户会话内已有 Gateway/桌面 Agent 执行 UI，二者通过已授权请求衔接。把 Gateway 简单移入 LocalService 服务可能破坏当前 UI Agent 的同用户同会话契约；若采用此部署模式，需先设计用户会话端的托管接线，复用现有 UI driver，不是删除这些检查。

6. **锁屏与 RDP 的真实边界。** `windows_identity.py:172–177` 在 input desktop 不可访问或变为非 Default 时拒绝；`:193` 对要求前台的动作返回 `UI_FOCUS_CHANGED`。这支持“不可交互时停止 UI 路径”的设计，不能证明所有 RDP 断开状态都被单独识别：本轮未发现显式 WTS 会话连接状态检查，也未做锁屏、RDP 断开、切换用户、UAC 安全桌面实测。管理页应将 GUI 会话不可用与后台日志 Agent 在线分开；禁止自动解锁、登录、切桌面或改变 RDP 状态来绕过前置条件。

7. **V3 输入是批准 recipe，不是任意 Computer Use。** `v3_agent/profile.py:55–70` 只接受空参数输入 schema，recipe 为 1–8 步，动作仅 `invoke`/`expand`，需要批准 locator 和后置 locator；`:87` 起的摘要要求 profile 与 recipe 校准引用。`state.py:187–213` 仅允许 `ui_only`，`combined` 返回 `combined_requires_unimplemented_binding_bridge`；输入要求 `v3_coordinated`、有校准证据且没有活动/未知输入 session。因此当前能复用结构化观察与批准单步动作，不能声称已支持任意坐标点击、键盘输入或 API/UI 自动联合绑定。实际可用 recipe 还要检查现场批准 profile 及其 hash，源码存在不等于现场可执行。

8. **一次输入与证据。** `uia_driver.py:122–149` 派发前复核前台、模态、唯一元素、布局、可见/启用与 UIA pattern，动作开始后不选第二驱动；`state.py:384–416` 在已派发但缺完整收据时返回 RESULT_UNKNOWN 并冻结输入。Remote Agent 在这种情况下必须停止动作与自动重试，沿原 operation 对账；不能改用旧 MCP 的鼠标回落来“继续验证”。UI 成功仅证明动作/后置条件成立，不能直接代表硬件任务或客户问题已解决。

9. **截图复用与模型能力。** V3 `uia_driver.py:169–196` 使用 PrintWindow 只渲染已绑定目标窗口，并要求该窗口在前台；`state.py:418–452` 限定批准 main_window 区域，图像发布带 hash、尺寸、观察/窗口代次，且文件发布后收据不完整会标未知。建议把该 Artifact 通过现有 Gateway 资源路径返回 Remote Agent/LabSlot，提供用户查看和模型视觉验证；云端为需要读图的任务选择支持图像的模型，客户仍免模型配置。敏感界面和截图传输范围需属于任务授权；不要默认连续录屏或截取整个桌面。16 MiB 单图等源码上限是保护阈值，不是每次都应发送的预算。

10. **与 API MCP V2/V3 联合验证的最小方案。** Lite Host 加三类受控工具适配：日志证据、当前已授权 API MCP、UI 观察/批准动作。按目标现场版本选择 V2 或 V3，不把 V2 缺少的收据/恢复语义冒充 V3；API 能直接读取的状态先用 API，GUI 展示、对话框与按钮路径再用 UI 证据。一次任务先保存验收条件与操作许可，再记录 API 响应、UI 观察/图像、操作前后对照与日志引用。当前 V3 `combined` 不可用，应采用任务层的顺序验证记录，标注两次观察时间与关联依据，不宣称原子一致快照或已实现协议级 combined。

11. **分期验收。** 第一批只做内部机器的日志＋API 只读＋UI 观察/单窗口截图，证明目标绑定、证据可回查和免客户模型配置。第二批选已校准、不会触发设备动作的 recipe，验证输入租约、用户抢焦点、模态阻挡、取消与未知冻结。第三批才按明确场景验证会改变配置、实验或硬件状态的操作；用户本轮要求的是设计增补，未授权任何现场操作。锁屏、未登录、RDP 断开、多用户、多 QSpin、DPI/语言/版本变化与服务重启各自记录结果；UI 不可用时允许已授权日志/API 诊断继续，结果明确标“GUI 验证未执行”。

12. **本轮关键快照。** `v3_agent/state.py` SHA-256 `1b7529ed927af1119566cb98ab073f38d081a6a1f5b9db6ebad07dd028aea668`；`windows_identity.py` `e2048278e7e35907512eaa6871857761bc825d1f6917862bc469ec36bb0d20d4`；`uia_driver.py` `c292368bfef031818198a6ddcbb37f58734e3165a45e054f3141ada0310a0ec2`。调查只证明当前源码形状与设计接点；未跑真实 UI、MCP 调用、模型读图、Windows 服务或硬件验收。

13. **增补最终复审。** 2026-11-01，独立 Codex reviewer `/root/windows_agent_design` 复核主稿 `verification-capabilities`、`verification-loop` 与关联服务/交付文字。UI 路径为 Session 0 服务调度，经授权 Gateway/UI 接口进入原用户会话内桌面 Agent；稿件准确保留 ui_only、invoke/expand、校准 profile、untracked WIP、combined 拒绝及通用 Computer Use 尚未具备的边界。API/UI 证据仅在 Task 层按时间聚合，不声称原子验证；现场 API、截图、输入和硬件操作均明确未执行。此次 UI/Windows 范围复审为 0 个未解决发现；Core 视觉与 API MCP 细节由对应专项调查覆盖，不纳入本条独立核验范围。绑定 `build-report.mjs` SHA-256 `0ac9b29d6488a185b93e3fc1dad440068bf4126481154d662e18c16f0edb52aa`、`design.html` SHA-256 `6f2a45e36e5af0d16ee287602e9aa31ecf708c8d6940572f79dacabc1e168ea1`（140904 bytes）。本条不替代真实 UI、模型或产品验收。
