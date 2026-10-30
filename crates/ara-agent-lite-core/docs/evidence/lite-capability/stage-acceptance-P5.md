# 阶段验收记录：P5

## 阶段验收目标

- 目标 1：未完成整体验收
- 目标 2：未完成整体验收
- 目标 3：未完成整体验收
- 目标 4：未完成整体验收

## 已覆盖

| 目标 | 子项 | 需求 | 证据 | 验证方式 |
|---|---|---|---|---|
| 1 | 多文件 C++ 编码真实产物与独立检查 | 31 | `D:\WorkSpace\tmp\ara-lite-capability\current-01a18bf7\consumer-20261030T085626Z\result.json` | 有界真实模型试验 |
| 2 | 压缩 | 35 | `D:\WorkSpace\tmp\ara-lite-capability\long-mechanism-codex-20261030-final\final-verification.json` | 受控测试 |
| 2 | 多次续写 | 35 | `D:\WorkSpace\tmp\ara-lite-capability\long-mechanism-codex-20261030-final\final-verification.json` | 受控测试 |
| 2 | 长度截断 | 35 | `D:\WorkSpace\tmp\ara-lite-capability\long-mechanism-codex-20261030-final\final-verification.json` | 受控测试 |
| 2 | 进程中断 | 35 | `D:\WorkSpace\tmp\ara-lite-capability\long-mechanism-codex-20261030-final\final-verification.json` | 受控测试 |
| 2 | 回执核对 | 35 | `D:\WorkSpace\tmp\ara-lite-capability\long-mechanism-codex-20261030-final\final-verification.json` | 受控测试 |
| 3 | 稳定接口与树外消费者 | 31 | `D:\WorkSpace\tmp\ara-lite-capability\current-01a18bf7\consumer-20261030T085626Z\result.json` | 有界真实模型试验 |
| 4 | 独立审查 | 32 | `E:\repos\ara-github\crates\ara-agent-lite-core\docs\evidence\lite-capability\reviews\final-count-linked-01a18bf7-0a08ac7e-ara-git-review-20261030T112535Z.json` | 受控测试 |
| 4 | 失败记录与未覆盖项保留 | 36 | `E:\repos\ara-github\crates\ara-agent-lite-core\docs\evidence\lite-capability\final-round-20261030-codex.json` | 受控测试 |

## 未覆盖

| 目标 | 子项 | 原因 |
|---|---|---|
| 1 | 逻辑推理真实产物与独立检查 | 已有P2样本保留；R1即使hidden28/28仍有3项预测错误，Oracle未通过，本轮无新的通过验收。 |
| 1 | 逆向分析真实产物与独立检查 | H2各配置均未通过，部分答案格式不符而无计数；本轮未重跑。 |
| 1 | 同模型同任务同预算前后对比 | 已有5份Comparable记录，每配置仅1次；结果翻转，不能归因单项P2；B0包含关闭开关的P2代码，不满足纯改动前基线。 |
| 3 | 职责边界与依赖方向 | 已有边界测试及矩阵通过，但本轮未重新接受完整Full/Lite边界目标；保留原模块偏差，不以单个消费者验收整个目标。 |
| 4 | 回归测试 | 最新针对性验证：compaction6/6、公网原用例1/1通过；原全量中的2项symlink权限失败按用户决定保留，最新差异未重跑完整门禁，不标verified。 |
| 4 | 依赖门禁 | Lite相对基线审计新增失败0，既有7项未清零；Headroom三项已批准应用且实际根策略固定H0审计通过，尚未验证正式适配锁文件或Full全图。 |

## 待审查的成功 Run

无

## Task 验收

- Task `S1`：Host 未通过

## 验收限制

受控机制仅证明机制覆盖，原两次真实长任务仍覆盖不足。S1运行记录保留task_accepted=false；公开示例与hidden通过不自动接受业务Task。阶段生成器只检查证据是否存在，本次人工核对了所声明的通过结果；未覆盖原因依据final-round-20261030-codex.json补充。重新生成阶段记录后须保留本段限制及具体原因。
