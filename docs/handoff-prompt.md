# Prompt for the first implementation AI

> Continuing work? Use the latest run handoff instead: [2026-09-26](handoffs/2026-09-26.md). This file is the original bootstrap prompt.

Copy the following prompt into the next AI's task. Supply access to this new repository and a **separate** checkout of the pinned OMP source. Do not give it a populated `.env` or paste API keys.

```text
你在 E:\repos\ara-rust 的新 Git 仓库实现 ARA Rust Agent Core。先读 README.md、AGENTS.md、docs/knowledge/INDEX.md、docs/roadmap.md、docs/upstream-sync.md、docs/acceptance.md、.ara/skills/INDEX.md、upstream/omp.lock.json。这里没有旧 ARA/OMP Git 历史，不能把旧仓库源码、状态库、密钥或历史整体搬入。旧 E:\repos\ara 只能作为只读参考；不要修改。

第一目标是把 Oh My Pi v18.1.8 的精确提交 596f2da7101178214aa27a753529d15e6b7ad91d 的 Agent 行为完整、有证据地 Rust 化。先在单独参考 checkout 核对 SHA、MIT 许可、packages/agent、packages/ai、packages/coding-agent 及对应测试，形成完整行为清单和测试映射。不要把旧 P5 卡数当作全量分母。每项写入 docs/upstream/feature-ledger.md：上游源码/测试位置、Rust owner、差异理由、实跑命令/结果/产物、状态。完成 P0 后按有界功能切片实施 P1，优先贯通真实 Rust 入口的 Session/Run、模型回合、工具调用与持久回执。共享 Core 不导入 HandWave、Lantern/Paseo、Lumen 的产品状态。

每个交付点先用 .ara/skills/ara-git-review/SKILL.md 固定目标版本与覆盖范围，并按改动使用 ara-rust-core-review 或 ara-provider-review；再按 ara-backend-verification 和 point-delivery-audit 在交付快照上实测：源行为 fixture、真实 Rust 进程/宿主链、错误/取消/恢复、实际工具或文件产物及状态。必要时使用可控假上游制造故障。对选定集成点，核对 ARA Manager → OMP 管理 → CAS 的当前模型 ID/协议/授权并做有界真实任务测试；可用 OpenRouter 当前 free 模型替代。密钥只通过环境变量或 CI Secret，绝不提交或打印。模型 HTTP 200、编译通过和测试代码存在都不等于目标完成。按 docs/acceptance.md 记录 docs/evidence/<id>.md，记录实际独立审查工具/结论或缺口，审计 diff 后才标 accepted。项目 Skill 是贡献流程；Rust 运行时 Skill 加载器尚未实现，不能声称已接通。

先给出可核查的 P0 清单、Rust crate/host 边界和第一有界切片，再直接实现、运行测试、提交有范围的本地 commit。及时报告当前有界批次进度与总目标边界，未能实测的点明确保持 open。不要提前把 upstream/omp.lock.json 的 ported_through_commit 从 null 改掉；后续同步按旧精确 SHA 到新精确 SHA 增量审查。还要读 docs/knowledge/agent-evolution.md：它记录 OMP 之后 ARA 自有的任务完成、持久参考、反馈进化、后台执行和 Omni 方向；这些是后续独立验收目标，不能用当前 OMP 对齐结果冒充已实现。
```
