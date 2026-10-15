# Agent 上下文与缓存效率：研究及本次落地

检索日期：2026-10-16。官方网页日期表示本次检索时间，不表示文章发布日期。研究对照起点为 Node `56fd2ab`，依赖固定为 rpi `8ec462672d90e62712d96074638efef89436cebe`；后文单独记录本次 0.2.0 制品验收，避免把旧源码观察当作交付状态。

目标是在独立 Rust 被控端中减少重复上下文开销，并让调用用量、进度和原始证据可检查。本次没有替换 Agent 框架、接入新的收费模型或变更生产配置。

## 官方事实

| 来源 | 经核对的协议事实 | 对实现的约束 |
| --- | --- | --- |
| [DeepSeek KV Cache](https://api-docs.deepseek.com/guides/kv_cache/)、[Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion/)、[Responses API](https://api-docs.deepseek.com/guides/responses_api/) | 自动、尽力而为的前缀缓存；prompt 总量等于 hit 加 miss；流式 usage 可以出现在最终内容 chunk；Responses 接口不支持缓存 key/retention 参数。 | miss 不是 cache write。不能要求 usage 一定单独成帧，不能把旧公告的 64-token 单位泛化为当前所有接口契约。KV 页面直接读取失败，采用官方页面搜索索引内容，并与 API 文档交叉核对。 |
| [OpenAI Prompt Caching](https://developers.openai.com/api/docs/guides/prompt-caching) | 当前 GPT-5.6 及以后文档使用 `prompt_cache_options`；缓存写入为普通输入价的 1.25 倍，读取价格依模型而异；较早模型采用不同 retention/key 机制。 | 保持前缀稳定有利于复用，但不能保证命中。不能沿用“缓存写入总是免费”的假设，也不能向全部 OpenAI-compatible 端点发送同一参数。 |
| [Anthropic Prompt Caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) | 工具、system、messages 构成前缀层级；支持 5 分钟与 1 小时策略，最低缓存长度依模型而异；input 不含 cache read/creation 数量。 | 分别核对 native endpoint、断点与回看限制。不能将 normalized input 再当作已含缓存量的输入总量。 |
| [OpenRouter Prompt Caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching) | 可用稳定 session 标识帮助 provider 粘性路由；用量与缓存能力依上游而异；写入可能产生负的 cache discount。 | 网关是否保留这些字段仍需请求级证据，不能把 discount 一律展示成节省额。 |

以上是提供方事实，不是当前 HandWave 路由已经支持或已经命中的证明。缓存是前缀计算复用，不是把旧模型答案直接当作新答案。

## 固定 commit 的开源实现

| 项目与固定版本 | 直接源码证据 | 可借鉴点及边界 |
| --- | --- | --- |
| Pi，2026-10-11，`15e7bdc14cae38786c4cde24870ffce888e062cd` | [compaction.ts](https://github.com/badlogic/pi-mono/blob/15e7bdc14cae38786c4cde24870ffce888e062cd/packages/coding-agent/src/core/compaction/compaction.ts)、[utils.ts](https://github.com/badlogic/pi-mono/blob/15e7bdc14cae38786c4cde24870ffce888e062cd/packages/coding-agent/src/core/compaction/utils.ts) | 一次性摘要显式使用 `cacheRetention: none` 并保留 summary usage；摘要输入中每个 tool result 最多 2000 字符。这是摘要输入裁剪，不能代替原件留存。 |
| Codex，2026-10-14，`cb03f28a4034b6ee901885808d27182ac586830a` | [context_manager/history.rs](https://github.com/openai/codex/blob/cb03f28a4034b6ee901885808d27182ac586830a/codex-rs/core/src/context_manager/history.rs) | 热历史裁剪与完整 rollout 原件分开。GitHub 上关于通用输出外存的提案不能当成已实现能力。 |
| OpenHands SDK，2026-10-09，`9e76fe72b43f7be65b48a243486c2e30b5104a22` | [agent.py](https://github.com/OpenHands/software-agent-sdk/blob/9e76fe72b43f7be65b48a243486c2e30b5104a22/openhands-sdk/openhands/sdk/agent/agent.py) | 静态 system 与每会话变化的内容分块；动态块不附缓存标记。借鉴数据布局，不搬运其产品状态或执行权限。 |
| OpenHands SDK，2026-09-28，`a130c8f9edf38c7a46825f7e6761d2497a00907b` | [llm_summarizing_condenser.py](https://github.com/OpenHands/software-agent-sdk/blob/a130c8f9edf38c7a46825f7e6761d2497a00907b/openhands-sdk/openhands/sdk/context/condenser/llm_summarizing_condenser.py) | 以事件/容量触发摘要，保留原子单位并归属摘要用量；不能据此直接确定 HandWave 的最佳阈值。 |

本次没有执行下载的上游代码，也没有从上述源码复制产品实现。一个旧 OpenHands 仓库路径的历史查询失败，随后定位到 SDK 的上述固定文件；失败不是功能不存在的证据。本窄项研究不判定 Claude Code 完整运行时的开源或授权状态。

## 本次落地及边界

### 用量与上下文观察

固定 rpi 的 OpenAI-compatible parser 已能处理 DeepSeek hit 和 OpenAI cached/write 用量，无需重新实现同一解析器。其 `Usage.input` 表示非缓存输入，`AssistantMessage.usage` 不带可靠的“原始字段是否出现”标记，缺失值可能默认成零。

Node 因而采用保守回执：agent 与 summary 的 `model_result` 可展示一致的正数 adapter counts；来源明确为 `provider_adapter_not_raw_presence_or_billing`。缺失或全零保持 `unknown`，费用保持 `null`，不推断“零缓存命中”“零费用”。正数回执中的零字段也不能区分未报告与显式零。agent 回执另含耗时；`model_call` 提供 system 字节数/hash 等内容无关观察，字节数不冒充 tokens。

这没有解决提供方原始 presence/partial usage 的完整归属，也没有把异常或取消标为成功计费。

### 稳定 system 与派生状态

变化的 Goal/Job 观察通过标明数据性质的 Host runtime carrier 进入投影，不再将当前 GoalState 序列化进 system。固定规则和模式指令仍在 system；目标、权限与预算仍由 Host 执行。carrier 纳入投影预算，并且不作为原始用户消息持久化到 raw journal。

一次性摘要显式使用 `CacheRetention::None`。agent 当前同样保持 None，没有在未验证的 Anthropic 网关启用新缓存标记，也没有加入通用 cache key、session key 或预热。该枚举在固定 OpenAI-compatible adapter 中不会关闭 DeepSeek/OpenAI 自身的自动缓存。

### Execute 进度

Execute 可调用 `task_update` 维护当前 Run 的有界 checklist，复用 id 更新进度；done 需要非空观察证据。实际事件使用 `step_id/output/detail/status`，完整 checklist 通过 tool result 返回并写入原始 journal。Plan 和 Goal 不公布此工具，Goal 使用既有 `goal_update`。这不是跨项目 Task 数据库，也不构成用户接受任务完成。

### 制品与可执行验收

`tests/verify_efficiency.py` 使用实际 Windows Rust Node、实际 Go Hub 和隔离 loopback SSE fixture。固定总预算为 180 秒，不访问远端模型，证据目录必须为空，失败原件保留。

| 制品 | 身份 | 结果 |
| --- | --- | --- |
| 早期 Release 候选，非最终独立制品 | `handwave-node 0.2.0`；SHA256 `5f47d78b5e369510520c8795523ad63b41d30a74b842c8b69d03686837daea83` | 效率 fixture PASS，12 个请求，12.517 秒；之后 PE 检查发现动态 VC runtime 依赖，不能作为最终独立 EXE |
| 最终静态 CRT Node | `handwave-node 0.2.0`；4922880 bytes；SHA256 `cb676ce827a0a496a23decd05246d0f78d5e24e0f6b5c8f569dc1e7eb5e304b1` | 同一效率 fixture PASS，12 个请求，36.457 秒，watchdog 未触发；本次直接解析 PE imports，未见动态 MSVC CRT DLL |
| Maintained Windows Hub | SHA256 `83e922282c577feba768ff88146611d61238ef5f6380cd81b58d9c9d1bbe6cb0` | 同一验收运行 |
| Test | SHA256 `1112bc1311afd1b66e2eceb3d0fc7a827c65f902f426575838278601a7586226` | debug 与 release 使用同一文件 |

最终制品来自正式 `build.ps1` 的静态 CRT 构建；效率验收前直接核对了 SHA、长度和 `--version`。PE imports 为 Windows 系统库 `kernel32.dll`、`bcryptprimitives.dll`、`ws2_32.dll`、`crypt32.dll`、`api-ms-win-core-synch-l1-2-0.dll`、`ntdll.dll`、`bcrypt.dll`、`advapi32.dll`。这是该制品导入表证据，不扩展为所有 Windows 版本/干净机器已验收。

验收观察：

1. DeepSeek 最终有 choices 的 chunk：prompt 1000、hit 700、output 20，得到 uncached 300、read 700、total 1020；包含没有文本的纯 tool-call assistant。
2. OpenAI usage-only chunk：prompt 100、cached 40、write 10、output 20，得到 uncached 50、read 40、write 10、total 120。
3. 缺失 usage 与显式全零均得到 unknown、cost null，不生成零缓存 miss。
4. Execute 实际调用两次 `task_update`，running → done；核对精确核心字段、Host 事件身份、完整原始 tool-result journal。
5. Plan/Goal 请求不含 `task_update`；同 cwd/rules/mode 的两 Goal objective/revision 不同，实际 wire system 相同，变化状态出现在带标签 carrier，原始 journal 不持久化 carrier。

最终静态制品的本机私有证据在 `D:/WorkSpace/tmp/handwave-version-20261016/standalone-efficiency/`，包括 `result.json`、控制器事件、模型请求、SSE 响应、进程日志及 journal；旧候选的证据保留在 `efficiency-release/`。首次尝试最终制品时文件尚未复制，启动失败在创建进程之前，原 stderr 保留；文件到位后核验 hash 再正式执行。先前 debug 首次失败是测试将附加了 Host identity/timestamp 的事件与核心字段 dict 全等比较；实际核心字段正确。测试修正为核心字段精确校验与 Host 身份单独校验，原失败保留在 `efficiency-debug/`，修正后 debug 在 `efficiency-debug-v2/` 通过。没有重放未知外部副作用。

复现入口（参数使用待验收制品路径和新的空目录）：

```text
python -B apps/handwave-node/tests/verify_efficiency.py --binary <node.exe> --hub-binary <maintained-hub.exe> --output <fresh-evidence-directory>
```

## 明确延期与未验收

- 原始 usage presence、部分流中断、native Anthropic start/delta 和真实网关账单的完整归属，需要 provider 层契约和独立 fixtures；当前正数 adapter counts 不冒充原始字段证据。
- provider-specific cache 参数、会话路由、Anthropic retention 和任何预热均需逐模型/协议核对，不能全局开启。
- 当前前台命令只保留有界 head/tail，丢弃的 stdout 中段没有自动保留原件；后续若支持 ranged artifact read，必须先实现限额、归属、重启/取消和清理，再承诺可恢复。
- 压缩阈值和检索策略应比较任务成功率、输入 tokens、摘要次数、额外读取、耗时与磁盘占用；现有原始 journal、history 检索和 Job logs 应优先复用。
- 本次正式测试未触发摘要，不证明摘要请求的真实缓存效果；没有验收真实 provider 缓存命中、费用节省、推理质量提升、生产 UI 或云端部署。

下一阶段优先取得有来源的真实路由用量和受控任务基线，再决定缓存与压缩参数。摘要、工具输出和候选经验不得未经来源与接受流程自动晋升为可信独立记忆。
