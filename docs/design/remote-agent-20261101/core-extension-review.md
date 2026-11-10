# Lite 接入 Computer Use 与 QSpin MCP 的边界

2026-11-01，只读源码调查。ARA HEAD `e229fb9ab800b46313047b89b73ac41e60c9efb4`；固定 rpi rev `8ec462672d90e62712d96074638efef89436cebe`。未修改业务、运行模型或操作客户桌面。本次结论来自当前源文件，图索引仅作线索。

## 结论

Lite 已有通用 AgentTool 与 MCP 接入基础，可供 QSpin API 工具和桌面工具使用。README 的“Computer use 不在需求42内”表示该功能未在该次交付中实现和验收，并不禁止加载返回图片的 MCP 工具。但当前完整视觉链路存在一个明确的 Core 接线缺口，不能只配置 MCP 就宣称模型能看到截图。

建议保持一个 Lite 循环，Host 接入两种工具来源：QSpin MCP 用于结构化状态和已授权动作；Computer Use 用于实际桌面观察与交互。验证时优先选择已有结构化 API；只有界面路径本身需要验收或 API 没有对应能力时才操作 UI。两者共用 Task、目标安装、操作授权和证据收据。

## 当前源码事实

| 事项 | 证据 | 结论 |
|---|---|---|
| MCP 工具作为普通 AgentTool | `crates/ara-agent-lite-mcp/src/lib.rs:219–232,258–260`；Core `src/agent.rs:27–30,48–57` | Host 注册工具即可，不需要在 Core 加入 QSpin API 或桌面平台依赖 |
| MCP 图片转换 | `crates/ara-agent-lite-mcp/src/tool.rs:48–67,136–141` | 合法 base64 image 转为 ImageContent；这里只验证编码，不证明图片格式、尺寸和视觉效果有效 |
| Core 模型构造缺口 | `crates/ara-agent-lite-core/src/agent.rs:1419–1423` | Model::new 后设置了输出/context/headers，未把已解析的能力 input 传给实际模型 |
| rpi 默认文本模态 | `C:/Users/loveu/.cargo/git/checkouts/pi-rust-ba8012a5f17fe953/8ec4626/crates/rpi-ai/src/model.rs:199–212` | Model::new 默认 `input=[Text]` |
| rpi 实际图片请求门禁 | 同一 rpi checkout 的 `crates/rpi-ai/src/providers/openai_completions.rs:334–367` | 只有 `model.input` 包含 Image 才把工具图片附为 `image_url`；当前 Core 默认路径不会发送该图片 |
| 模型协议范围 | Core `src/agent.rs:1277–1279` | 当前只准入 openai-completions/openrouter-chat；不能宣称已支持供应商原生 Responses/Anthropic Computer Use 协议 |
| JSONL 两类工具路径不同 | `crates/ara-agent-lite-jsonl/src/runtime.rs:315–323,366–374` | 内置 mcp_servers 直接注册 MCP 工具；普通外部 tool_reply 把 value 转纯文本。后者不能用 JSON 图片字段冒充 ImageContent |
| MCP annotations 影响结算 | MCP `src/lib.rs:219`、`src/tool.rs:157–177` | readOnlyHint 被直接用于中断后的“无外部效果”分类，不能当作动作授权或可靠的副作用证明 |
| MCP 错误结果丢结构化结算 | MCP `src/tool.rs:84–100` | isError 时 structuredContent 拼入文本并返回 Err；不会保留 AgentToolResult.details.effects_unknown |
| 图像预算仍按文本 JSON | Core `src/model_caps.rs:63–81,94–103`、`context_memory.rs:613–614` | base64 计入字节预算；当前 Run 过大直接拒绝，不能靠旧轮次压缩保证持续截图可用 |
| 当前受控测试边界 | MCP `tests/result_props.rs:63,117–121`；`tests/harness_mcp.rs:91` | 图片测试覆盖转换；Harness fixture 声明 text，仅验证成功与超时机制，不能作为视觉端到端证据 |

## Core 与通用 MCP 层的最小增量

1. Core 把受信模型能力中的输入模态传给实际 rpi Model；Host 请求视觉任务时必须检查模型具备 image 能力。能力缺失或不兼容应拒绝截图推理，不能让模型在没收到图时继续给出图像判断。同步修改 `host_input` 事件（当前 `agent.rs:1304` 固定 text）。
2. 给图像数量、编码字节、像素尺寸与连续观察历史设明确预算。截图工具先生成合适尺寸；Core 保留一般化的模态和请求准入。基于实际 provider 请求验证，不把 context_window token 值直接当图片容量。
3. 普通 JSONL 工具若必须承载截图，另扩展版本化的 typed image result；如果先采用 Rust Host 或 JSONL 内置 mcp_servers，可暂不改普通工具帧。
4. 通用 MCP 若要支持错误保真与可信副作用策略，可增加类型化错误结果/Host 策略覆盖接口。这是 MCP 适配层职责，Core 继续只识别通用 effects_unknown，不认识 QSpin 的错误码。

## Host 必须完成的工作

- MCP 配置、工具名单、模型视觉权限由 Host 选择。Computer Use 的截图、点击、输入、按键分别分类；点击和输入不能因服务器 readOnlyHint=true 变成“只读”。权限审批在实际执行前核验，绑定目标安装、桌面会话、窗口、动作与原 Task；允许名单不是 OS 沙箱。
- QSpin V2/V3 的版本、capabilities、操作 ID 与恢复语义由产品适配器解释。V3 的 UNKNOWN、may_have_been_sent、original_intent/handle 需保存原始收据，并转为结构化 effects_unknown/待结算。不能只把错误 JSON 放进模型文本后期待它自行不重做。
- 取消等待模型或 MCP 调用不等于撤销已执行点击/采集命令。超时后先观察状态、查原 intent，结果不能证明时停止并保留现场；不得重新点击或重发相同业务命令来“验证”。
- Windows 服务在 Session 0；桌面访问应通过指定交互会话中的受控 helper/MCP server，绑定实际用户/会话/窗口。锁屏、未登录、UAC 安全桌面或会话切换应明确报告不可操作，不能声称安装服务后即可操作所有桌面。
- 截图可能含客户文件、其他窗口和账号。裁剪目标窗口、遮蔽指定区域必须在图片进入 Core/MCP 结果前完成。图片进入 Session 和模型上下文后应视作敏感证据；定义留存、访问、上传与删除规则。
- 每次 UI 观察记录安装/用户会话、窗口身份、时间、分辨率/DPI、截图 hash 和裁剪坐标；动作记录原观察 ID、目标坐标/控件与执行前后证据。旧截图不能作为窗口切换后的有效操作依据。

## 必须补的验证

| 验证层 | 通过标准 |
|---|---|
| MCP 到 provider 受控链路 | 返回真实小图片，核对第二次出站请求实际含该图片 data URL；非视觉模型明确拒绝；不只断言 ImageContent 存在 |
| 图像预算 | 单图/多图、连续截图、超大图、非法格式和上下文不足均有明确终态，原图收据不因裁剪或拒绝丢失 |
| QSpin 错误桥 | V2/V3分别注入返回错误、已发未回、取消与恢复；UNKNOWN进入待结算，后续零重放；原 intent 可查询 |
| 交互桌面 | 指定测试机上的窗口观察→获准动作→API/界面双证据复核；锁屏/切用户/窗口变化阻止旧动作 |
| 授权与隔离 | MCP 恶意只读标注、未批准工具、跨机器/会话目标、过期授权、图片敏感区均被边界规则拦截 |
| 真实模型任务 | 实际模型能引用截图内容并使用QSpin工具完成限定验收；受控成功不代替真实模型，也不代替硬件许可 |

建议先以内部 QSpin 测试实例验证，不把诊断服务默认升级为通用远控。产品范围可扩展到获准的验证任务，但每次硬件动作继续遵守其原有授权和实验约束。

## 增补最终复审

2026-11-01，独立 Codex reviewer `/root/core_protocol_design` 复核主稿新增的 `verification-capabilities`、`verification-loop` 及关联原稿边界，并检查实际生成 HTML 的关键内容。最终绑定：

- `build-report.mjs` SHA256：`864f84f08873ad10014ddb8aa0db793bd16482290f4839ce15d9d28d798a938e`。
- `design.html` SHA256：`1d0abacd3dd20b0e509bc3810b053b52e98f592d96dbad6616707225cf32458f`，141433 bytes。

本次 Core/MCP、模型输入与安全边界复审为 0 个未解决发现。主稿准确区分 MCP 图片转换与 Core 模态接线缺口，要求实际 provider 出站验证；区分 JSONL 内置 MCP 和普通文本 tool_reply；将 QSpin UNKNOWN 的结构化桥放在通用错误文本转换之前；可信副作用分类覆盖不可信 readOnlyHint。初审提出的截图隐私缺口已补入：图像进入 Core/MCP 与模型前遮蔽敏感区域，原图与脱敏图分别留 hash、来源信息和受限留存期限。

设计仍明确保留视觉接线未修、MCP/QSpin错误桥与UI适配未实施、真实模型/桌面/客户硬件验收未执行的状态。本次仅接受设计内容与层次划分；未运行浏览器视觉验收或产品测试。V2/V3 SDK 与 Windows UI 实现细节由对应专项调查核验，本条不冒充这些范围的全量复审。
