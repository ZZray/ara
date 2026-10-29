# 上下文优化候选：OpenCode 后台预计算摘要

2026-10-27。候选 `CTX-OPT-BG-01`，状态：**调研记录，暂不实施**。用户要求将截图中的 OpenCode 实验功能作为后续优化选项保存。本条不新增当前交付需求、任务、依赖、开关或后台作业，不改变 Headroom/P0–P5 的实施顺序，也不改变固定 OMP 对齐标记。

## 调研结论与固定来源

OpenCode 是 [MIT 开源项目](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/LICENSE)。其 V2 后台压缩已经合入并写入 [v2.1.4 更新说明](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/CHANGELOG.md#L5-L16)，仍为默认关闭的实验选项。建议保留其“提前准备候选、需要时校验并发布”的思路，待 ARA 的预算、恢复和任务评估基础就绪后再决定是否实现；本次不建议直接移植。

| 来源 | 本次核对范围 |
|---|---|
| 观察源码 | `anomalyco/opencode`，固定 dev SHA `560b11bfc264f9b1925e6d2015666d1cabe8a5ca`；下文源码链接均固定到此提交 |
| 发布交叉核对 | tag `v2.1.4` → `68bc26b7da1990ce10ffecfbc1e09eb13aa5d824`；核对 compaction 的预计算入口、默认关闭和 85%/64k 常量，未把 tag 与观察源码当成完全相同版本 |
| 引入与改名 | [PR #57444](https://github.com/anomalyco/opencode/pull/57444)，合并提交 `6a5973d2636936317de15726ab37e3681f678be3`；[PR #58294](https://github.com/anomalyco/opencode/pull/58294)，合并提交 `f523f7713816163b75be81d17186ab539a56f76a` |
| 官方说明 | [V2 Compaction](https://dev.opencode.ai/v2/docs/compaction/#background-compaction)，检索于 2026-10-27；页面可能继续变化，以固定源码作为本记录的版本依据 |
| ARA 对照 | `56d55d209ecea00e80dfefecdd3e9d47fb62c47a` 加本机既有 dirty 工作区；对应文件哈希见[研究收据](../evidence/opencode-background-compaction-20261027.json) |

输入截图只作为选题来源。截图关于 Codex、Claude Code 的实现和默认开关不在本次核验范围内，不能由 OpenCode 的 PR 转述推定为已证实事实。

## 上游实际机制（源码支持，未运行）

公开配置为 `experimental.background_compaction: true`。旧开发名称 `precompute_compaction` 在发布前改名；本次核对的配置插件只读取新键，未完整核验旧键的解析与迁移路径。插件只在新键严格为 true 时启用；服务初始 `precompute` 为 false。普通 `compaction.auto` 默认 true，不能与后台预计算的默认关闭混淆。[配置接线](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/config/plugin/compaction.ts#L14-L22)、[默认值](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L257-L261)。

1. **提前生成。** 开启后台及自动压缩后，估算上下文达到压缩 ceiling 的 85%、尚未真正到阈值，且最近对应模型的已测量 prompt 输入（含缓存读写）至少 64,000 tokens，才考虑启动。原生 provider compaction 不走此分支；模型切换后先等新模型产生响应。[启动条件](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L337-L383)。85% 指压缩阈值的比例，不是模型总窗口的比例。
2. **只暂存候选。** 摘要覆盖最近响应之前的前缀；该响应及后续消息用 `keepID` 界定，继续保留。每个 Session 暂存一个候选，记录首消息、模型/variant、指令、边界与尾部限额，在后台生成时不改写当前上下文，以保留前缀缓存的复用机会。[候选结构](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L110-L121)。
3. **需要时再取用。** 真正压缩时核对会话起点、模型/variant、指令、保留边界和尾部长度。候选已完成且有效时可避免再发摘要请求；仍在生成时会等待；不适用或生成失败则回到普通摘要。overflow 恢复和 native compaction 不使用预计算候选。[发布入口](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L301-L325)、[取用校验](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L395-L418)、[边界和尾部限制](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L929-L945)。
4. **会产生额外成本。** 后台调用完成时记录用量，未采用的摘要也有成本；源码保留失败次数限制。候选只存在内存中，重启后丢失。缓存复用是设计意图，不保证任意 provider 的实际命中，也不证明总费用下降。[计费与事件](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/src/session/compaction.ts#L849-L867)。

上游[测试定义](https://github.com/anomalyco/opencode/blob/560b11bfc264f9b1925e6d2015666d1cabe8a5ca/packages/core/test/session-compaction.test.ts#L1004-L1200)覆盖默认关闭、无额外摘要调用的取用、等待进行中候选、模型切换、长尾失效与失败后再尝试。本次没有安装或运行 OpenCode，也没有执行这些测试或测量真实长会话；上游 PR 的验证叙述不作为 ARA 的运行证据。

## 局限与适用条件

- **可能减小前台停顿，不能保证无等待。** 候选必须在触发压缩前完成并仍适用；否则等待或重新生成仍在关键路径。若上下文增长很快、经常切模型或重启，预计算可能反复浪费。
- **不提高摘要的信息保真度。** 提前生成不消除摘要丢失细节的问题。旧摘要若被错误使用，还会遗漏后来发生的纠正或状态变化；保留原始来源、近期尾部与当前状态仍是必要条件。
- **固定阈值不宜照搬。** [issue #58818](https://github.com/anomalyco/opencode/issues/58818)在本次查询时为 open，报告 64k 下限使小窗口无法触发。固定源码仍有该常量；本次未运行其复现。该问题足以要求未来按实际窗口、预算和负载验证阈值，不能把 85%/64k 当成硬约束。
- **插件与恢复另有边界。** 引入 PR 明示，压缩插件 hook 也可能看到最终丢弃的后台摘要，候选不跨重启保留。未来 Host 必须区分候选观察和正式发布，不把准备结果当作已生效状态或任务验收。

## 与 ARA 当前工作的关系

| 路径 | 当前已存在或计划的职责 | 与本候选的关系 |
|---|---|---|
| Lite 同步摘要 | `agent.rs:1320–1369` 先校验和预留调用，再等待摘要，clone → install → save → 替换 Store；有取消、截止和投影预算检查 | 后续可评估把生成与发布分离；不重写现有来源和预算机制 |
| Lite 原始/投影分离 | `context_memory.rs:548–671` 保留原始来源、完整旧轮次和摘要链 | 预计算不是解决当前 Run 单轮过大或原始工具输出过大的替代方案 |
| Harness 调用结算 | `runner.rs:911–937` 用单个 pending 配对 agent/summary 调用事件 | 直接并发两个调用会被判失败；不能只加 `tokio::spawn`，须先设计准入、关联和结算契约 |
| Full Session 安全提交 | `ara-session/src/lib.rs:3540,3656` 对比原快照与当前状态，不一致拒绝 `StaleSnapshot` | 前台追加历史会使旧快照失效；若未来接入，需明确固定前缀候选契约，不能放松现有 guard。本次未审全体 Full Host 调度能力 |
| Kiro P2-d / 需求 19 | 摘要安装后重新读取当时磁盘文件和当前计划 | 刷新应在候选发布时发生，不能以后台开始时的文件快照冒充当前状态 |
| Kiro 需求 25 | 摘要调用与 agent 调用共用 Task 预算 | 后台候选即使丢弃，也须计入预留、调用和时长，不另开 Session 绕预算 |
| Headroom P2-f | 对模型请求投影中的工具结果做确定性体积压缩，保留原文取回 | 可互补，但影响不同；应分别 A/B，再评估组合，不能混合归因 |

## 重新评估的前置条件和验收

当前只登记候选，不列工期，不进入任务 1–66 或当前验收分母。将来要实施时再单独立项、审查范围和预算：

1. 先有可重复的长任务基线，证明前台摘要等待占了显著耗时；现有连续性、共享预算、未知效果门禁和发布时刷新可用。
2. Host 显式 opt-in，并拥有候选的 Session/Run、来源前缀和版本、模型/策略/授权、取消和有效期。原始 Journal 不改；候选不得成为原始人类输入或已采纳知识。
3. 仅发布经过复验的候选，未覆盖的新尾部逐条、按序保留且不重复。分支/reset、来源撤销或纠正、模型/指令/权限变化使不匹配候选失效；不存在可用结果时保留原来的有界前台路径，不盲目重试或重放工具。
4. 摘要启动前就占用同一 Task 额度。为前台/后台交错设计可审查的调用 ID 与事件配对；丢弃、超时或取消不退还预留，未知 usage 不记零。暂停候选或恢复候选的语义都需明确，不能默认创建常驻调度。
5. 用受控 provider 和真实进程验证候选完成/未完成/失败、长尾、分支切换、授权变化、取消、重启与双调用结算；再以同模型、任务、预算做有界 A/B，比较前台阻塞、总耗时、总用量/费用、缓存用量、候选命中与丢弃率、来源和纠正保持率。正确性或成本退化时保留关闭。

以上是后续决策门槛，不是已实现或已通过的验收。若复用上游代码，另行保留 MIT 许可与版权，并固定目标版本；本次没有复制运行时代码、变更依赖或修改 OMP baseline。
