# Windows Computer Use 选型复核

日期：2026-11-01。用户约束：Windows 优先，复用开源通用 Computer Use，Mac/Linux 后续适配。本轮只更新设计，没有安装、产品修改或现场操作。

推荐 CursorTouch/Windows-MCP 作为优先技术试点，保留 Lite 循环与 QSpin 专用 UI 路径；不把技术候选认定为已批准分发依赖。官方 README、pyproject.toml 和 LICENSE.md 均回查提交 `7fd39b4c1551d4d9da33d55c486ac5c4c6e2cf41`。版本为0.8.8，要求Python≥3.14及Windows10/11。pywinauto、FlaUI为库级备选；Microsoft UFO整套编排与Lite职责重叠，暂不引入。

主代理和独立 Codex `/root/windows_cu_review` 分别核对官方资料，确认以下约束已进入方案：

- Windows-MCP顶层MIT不代表依赖全部MIT：fuzzywuzzy为GPLv2，python-Levenshtein为GPLv2或更新版本。发布前核对实际锁定依赖及分发义务，不能用进程隔离代替许可核查。
- 通用后端运行于授权交互用户会话，SCM服务负责调度；用户会话broker为待实施能力。
- 默认全工具和遥测须收窄；区域截图不能代替窗口身份及整个结果的隐私过滤。
- 用户接管是best-effort，不能视为权限隔离或撤销已发生效果；UNKNOWN不得换API/UI重放。
- Core只补通用视觉/工具能力；Host负责授权与收据；QSpin专用profile/recipe保留。Mac/Linux不计本期验收。

最终独立复审结论：无阻断设计发现。此结论不代表依赖许可审计、运行可靠性、客户环境或产品验收通过。

绑定文件SHA256：

- build-report.mjs：`692dde0bbd3770a89629fad845a7c6f18be123fa52b1c366b45eb036bad916c9`
- design.html：`b7033197c20eba6b321575be393f0478d423d8d01dcd39937c376937fe090a6b`

静态检查见 `windows-open-source-validation.json`：15节、33个唯一ID、0坏本地链接、3段内联脚本语法通过。文案已按Humanizer审读。既有file预览策略拒绝保留，未绕过；未做视觉验收。历史14节验证与复审记录保留原SHA绑定。
