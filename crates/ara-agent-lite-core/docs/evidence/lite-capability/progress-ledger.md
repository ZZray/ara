# 进度台账摘要

由 `lite-eval ledger render` 从 `progress-ledger.jsonl` 生成，共 16 条记录。

- 总进度：verified 检查点 0/36
- 模块状态：verified 0，done 8，open 1，未开始 29

| 模块 | 阶段 | 需求 | 状态 | 完成标准 | 更新时间 | 证据 | 未覆盖 |
|---|---|---|---|---|---|---|---|
| M-P0-TASKSET | P0 | 1 固定评估任务集 | done | 4/7 | 2026-10-29T15:11:56+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/gates/M-P0-TASKSET-local-20261029T150951.json | SH1 登记见任务 10；6 项完整校验见任务 11.4；1.4 逐任务 fixture SHA 复核与 1.5 Manifest 逐项记录见任务 13；完整 11 命令门禁见任务 61 |
| M-P0-HOLDOUT | P0 | 2 新增密封 C++ holdout 任务 | open | 7/8 | 2026-10-29T21:47:11+08:00 | tools/lite-eval/tests/isolation_props.rs | 10.6 file symlink case fails: no SeCreateSymbolicLinkPrivilege, Developer Mode off (os error 1314) |
| M-P0-RUNNER | P0 | 3 一键评估运行器与固定运行策略 | 未开始 | 0/8 | — | — | — |
| M-P0-METRICS | P0 | 4 结果度量与未知值记录 | 未开始 | 0/8 | — | — | — |
| M-P0-FAILURE | P0 | 5 失败记录与分类 | 未开始 | 0/7 | — | — | — |
| M-P0-FORK | P0 | 6 同前缀 Session 分叉（可选） | 未开始 | 0/7 | — | — | — |
| M-P0-BASELINE | P0 | 7 基线证据 | 未开始 | 0/7 | — | — | — |
| M-P1-RESUME-BASE | P1 | 8 已实现基线：Core 中断 Run 结算与 Harness resume | done | 7/8 | 2026-10-29T13:32:33+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/gates/req8-start.json | resume_checked 与 ResumeRefusal 待实现（任务 19）；8.5 每次交付重跑 |
| M-P1-JSONL-START | P1 | 9 JSONL resume 启动帧 | 未开始 | 0/4 | — | — | — |
| M-P1-JSONL-RECONCILE | P1 | 10 JSONL 结算请求与回复帧 | 未开始 | 0/6 | — | — | — |
| M-P1-PY | P1 | 11 Python 客户端 reconcile_resume 回调 | 未开始 | 0/5 | — | — | — |
| M-P1-NODE | P1 | 12 Node 客户端对齐 | 未开始 | 0/5 | — | — | — |
| M-P1-DOCS | P1 | 13 resume 协议与集成文档 | 未开始 | 0/6 | — | — | — |
| M-P1-PROCESS | P1 | 14 真实进程中断与恢复 | 未开始 | 0/6 | — | — | — |
| M-P2A-OUTPUT | P2 | 15 输出上限核实与 Host 策略 | 未开始 | 0/7 | — | — | — |
| M-P2A-LENGTH | P2 | 16 任务级长度截断续写 | 未开始 | 0/9 | — | — | — |
| M-P2B-GATE | P2 | 17 完成门 | 未开始 | 0/10 | — | — | — |
| M-P2C-STUCK | P2 | 18 无进展检测 | 未开始 | 0/10 | — | — | — |
| M-P2D-REFRESH | P2 | 19 压缩后刷新 | 未开始 | 0/9 | — | — | — |
| M-P2E-EDIT | P2 | 20 能力测试 Host 使用 edit_file | 未开始 | 0/7 | — | — | — |
| M-P2-COMPARE | P2 | 21 改动前后对比记录 | 未开始 | 0/7 | — | — | — |
| M-P3-UNKNOWN | P3 | 22 类型化未知副作用记录 | 未开始 | 0/8 | — | — | — |
| M-P3-JSONL-UNKNOWN | P3 | 23 JSONL Unknown 结算类型化 | 未开始 | 0/7 | — | — | — |
| M-P3-REPLAY | P3 | 24 重放前置条件 | 未开始 | 0/7 | — | — | — |
| M-P3-SUMMARY | P3 | 25 Harness 内压缩共享 Task 预算 | 未开始 | 0/6 | — | — | — |
| M-P3-BYTES | P3 | 26 NUL 与 Unicode 字节保留 | 未开始 | 0/6 | — | — | — |
| M-P4-DEPS | P4 | 27 职责边界与依赖方向 | 未开始 | 0/6 | — | — | — |
| M-P4-EVENTS | P4 | 28 版本化类型事件枚举 | 未开始 | 0/7 | — | — | — |
| M-P4-STABILITY | P4 | 29 公共接口稳定性规则 | 未开始 | 0/5 | — | — | — |
| M-P4-MATRIX | P4 | 30 Full/Lite 支持矩阵 | 未开始 | 0/5 | — | — | — |
| M-P4-CONSUMER | P4 | 31 树外消费者 crate | 未开始 | 0/7 | — | — | — |
| M-P5-REVIEW | P5 | 32 模块独立审查 | done | 5/5 | 2026-10-29T14:11:09+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews/checkpoint7-c3f6c9ec-codex-20261029T061109Z.json | 审查缺口：codex review is a recorded gap: 用户 2026-10-29 指定按技能自行审查，不调用外部 Codex；独立 Codex reviewer 未运行 |
| M-P5-QUALITY | P5 | 33 交付代码质量门 | done | 4/4 | 2026-10-29T14:11:09+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews/checkpoint7-c3f6c9ec-codex-20261029T061109Z.json | 审查缺口：codex review is a recorded gap: 用户 2026-10-29 指定按技能自行审查，不调用外部 Codex；独立 Codex reviewer 未运行 |
| M-P5-DENY | P5 | 34 依赖审计现状记录 | done | 5/5 | 2026-10-29T14:11:09+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews/checkpoint7-c3f6c9ec-codex-20261029T061109Z.json | 审查缺口：codex review is a recorded gap: 用户 2026-10-29 指定按技能自行审查，不调用外部 Codex；独立 Codex reviewer 未运行 |
| M-P5-LONG | P5 | 35 长任务综合可靠性验收 | 未开始 | 0/6 | — | — | — |
| M-P5-STAGE | P5 | 36 证据保留与阶段验收记录 | done | 7/7 | 2026-10-29T14:11:09+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews/checkpoint7-c3f6c9ec-codex-20261029T061109Z.json | 审查缺口：codex review is a recorded gap: 用户 2026-10-29 指定按技能自行审查，不调用外部 Codex；独立 Codex reviewer 未运行 |
| M-X-LEDGER | 横向 | 37 进度台账 | done | 5/5 | 2026-10-29T14:11:09+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews/checkpoint7-c3f6c9ec-codex-20261029T061109Z.json | 审查缺口：codex review is a recorded gap: 用户 2026-10-29 指定按技能自行审查，不调用外部 Codex；独立 Codex reviewer 未运行 |
| M-X-NOTIFY | 横向 | 38 钉钉进度通知 | done | 9/9 | 2026-10-29T14:11:09+08:00 | crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews/checkpoint7-c3f6c9ec-codex-20261029T061109Z.json | 审查缺口：codex review is a recorded gap: 用户 2026-10-29 指定按技能自行审查，不调用外部 Codex；独立 Codex reviewer 未运行 |

## 未解除的阻断

- **M-P0-HOLDOUT**（seq 16）：file symlink privilege unavailable on this machine；需要的决定：run as administrator or enable Developer Mode, then rerun isolation_props
  - 已尝试：std::os::windows::fs::symlink_file → os error 1314
