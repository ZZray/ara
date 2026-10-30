# ara-agent-lite-core 项目知识

这个库从 HandWave 精简端提取。完整版 ARA／OMP 继续在原 workspace 开发，
这里的验证记录不代替完整版的功能对齐或验收。

- [Headroom Rust 接入计划与 H0](../evidence/lite-capability/headroom/README.md)：需求 39–41、任务 63–66 已补入 Kiro；仓外构建和离线案例通过，依赖门禁未通过，Lite 正式接入与真实任务对比尚未开始。

- [来源读取与压缩续作](../evidence/context-continuity-20261025.html)：大来源分窗、命中定位、跨进程三次压缩及错误路径；受控机制通过，单次真实模型取源失败。默认 8 KiB 窗口的兼容规则见接口说明。
- [公共生成策略与历史修复](../../../ara-agent-lite-jsonl/docs/evidence/generation-policy-20261025.html)：JSONL 接线与空提交反馈已验证；原 Task 的预算停止和失败收据保留。
- [预算停止后的产物复核](../../../ara-agent-lite-jsonl/docs/evidence/closed-budget-snapshot-20261025.html)：Core 提供有限的原生闭合证据，上层通过专用回调核验实际产物；未知外部效果仍阻断。真实模型补丁通过七项回归，但补充检查发现合法 NUL 被改写，完整修复未验收。普通 Stop 与受控预算快照分别记录。

- [0.1.1 版本化读取与编辑](../evidence/coding-edits-20261022.md)：精确快照、多块预检、同句柄提交、未知副作用与实际 Host 检查；[AI 接入说明](../ai-integration.md#51-版本化读取与编辑011)列出工具参数。
- [编码与逆向增强采用清单](../evidence/coding-reverse-enhancement-20261022.html)：24 项能力与 Full/Lite 的复用边界，本轮可选输出归档的实际验证和后续逆向验收；不改变完整 OMP 或产品发布状态。

- [编码与逆向能力研究](../evidence/coding-reverse-research-20261022.html)：模型、工具、运行框架与环境的分工，固定开源实现、外置分析后端、增强顺序和真实验收方案；[独立审查](../evidence/coding-reverse-research-20261022.review.json)仅接受研究，未改变运行时或部署。
- [AI 接入说明](../ai-integration.md)：给其他 AI 的实施步骤、字段清单、公开接口和受控验收。
- [0.2.9 发布记录](../../../../apps/handwave-node/docs/evidence/stage-publication-0.2.9-20261021.md)：固定源码提交、Windows 与 GNU Linux 检查、下载和部署边界。
- [业务接入](../../README.md)：公共接口、默认权限、路径、模型和 Worker 入口。
- [公共接口稳定性规则](../stability.md)（初稿）：`RunRequest` 字段、`rpi_*` 重导出与 Harness 入口的 stable／experimental 分级和变更规则；P0–P4 新增项暂为 experimental，待任务 57.1 定稿。
- [Full/Lite 支持矩阵](../support-matrix.md)（初稿）：7 个类别 39 项能力的两侧状态与依据，机器可读副本供交付门禁运行引用测试；本阶段不合并 Full 与 Lite。
- [宿主边界与兼容](host-contracts.md)：Core、Harness、JSONL 与 Host 四方职责和依赖方向，身份和授权归属、存储兼容、未知副作用；包含 Windows 分析路径、空分页、本地 Off 与实际 thinking 请求、rpi 的 optional null 处理，以及请求预算与示例 Host 实际额度的区别。
- [2026-10-21 提取与验收](../evidence/extraction-20261021.html)：实际检查、镜像身份、失败记录与未覆盖项。
- [机器可读收据](../evidence/extraction-20261021.json)：源文件与运行证据哈希。
