# Lite 收尾记录（2026-10-30）

当前分支为 `dev`，基线为 `4640b9fa495d538c7a4fd36b18825312a6e7cdb4`。本轮直接在原目录工作，没有创建 worktree；未提交或推送。原有 HandWave 与知识文档改动保留。

## 已完成

- `lite-eval oracle <task-id> --artifacts <dir> --out <file>` 已实现，复用 runner 的独立判题逻辑。支持可选 `--final-text`；推理和逆向任务缺少文本时拒绝启动。输出及判题副本在原产物之外，已有结果拒绝覆盖。Sealed Holdout 仍通过 `run` 路径判题，以保留隔离和污染记录。
- CLI 进程测试 3/3、既有 Python Oracle 适配测试 5/5 通过。
- S1 树外消费者按固定 Run_Policy 运行：1 Run、11 Logical_Call、进程耗时 33.652 秒；Host 编译并验证公开 README 示例，进程外 Oracle hidden 13/13。S1 没有登记 public 用例数，未补成 0/0。CAS 回显 `deepseek-flash` 仍属于允许但未独立确认身份的别名。
- 用户选择方案 A。试验 Host 增加可选 `--host-review-rounds`，默认 0；显式策略分别记录真实公开检查结果和人为延期审查理由。当前 driver、脚本与重建 sidecar 的受控复验耗时 45.365 秒：压缩 1、Host 续写 2、Length 续写 1、OS kill/resume 1；独立 checker 7 项通过。
- 22 个测试文件的 23 处 Property 配置恢复为 256 例；支持矩阵补入四条上下文窗口测试引用。
- 最终依赖审计发现 MCP path 依赖缺少版本约束。仅补 Core `=0.1.1` 和 Harness `=0.1.0`，不修改锁文件或 deny 策略。复审通过；审计结果为既有失败 7、新增失败 0、未执行 0。

## 验证状态

原完整门禁及补测已执行结束；下表保留该批次结果。后续deadline与公网检查已通过针对性复验，任务61仍未通过，见文末更新。原批次汇总见 [final-round-20261030-codex.json](final-round-20261030-codex.json)。

| 检查 | 结果 |
|---|---|
| Core 默认测试 | 294 通过、0 失败、2 ignored |
| Harness / JSONL Rust / Node | 61 / 12 / 30 通过，均无失败 |
| JSONL Python | 114 通过、1 失败；共 115 项完整执行 |
| Support_Matrix | 84/84 引用逐项通过 |
| lite-eval | 34 个 integration targets 与 doctests 均已执行；最终汇总 155 通过、2 失败、0 ignored |
| MCP / consumer unit / Host policy unit | 24 / 3 / 4 通过 |
| Oracle CLI / Python adapters | 3 / 5 通过 |
| 消费者 build、clippy、稳定接口扫描与破坏演示 | 通过；演示中移除公开字段后如期编译失败，错误命名匹配 |
| 显式执行的 ignored 专项 | capture_events_v1 通过；live Web 失败，DDG 返回 challenge，Python 文档 fetch 成功 |
| 最终依赖差异审计 | known 7、new 0、未执行 0，新增依赖 8 项；不是全部依赖零失败 |

两项 lite-eval 失败均为文件 symlink 的 WinError 1314，用户决定本轮保留权限阻断。JSONL deadline 用例原全量及一次精确复验均未通过，见 [deadline 用例记录](CASE-20261030-COMPACTION-DEADLINE.md)；未修改期望或继续重试。现有证据支持 fixture 时序余量不足，尚无产品请求漏发或越界续发的确认依据。

完整质量门快照为 `1ac911a55c10251957a1067b13ad39efbdb7c94e321c4098dcd5ad2036b6fd48`。之后仅修正 `runner_props.rs` 的 probe 总数，使 `cases=256` 同时驱动配置与断言；受影响 target 的 fmt、clippy 与 256 例复验通过。该批代码收尾 CodeVersion 为 `0a08ac7ebecfb987f7b03da1e720d0f5e791bfc5bb428cb4692c3adcd0ad29e0`，没有在此快照重跑完整门禁。

消费者记为 `done 7/7`。受控机制验证与原两次真实长任务记录分开保存；`M-P5-LONG` 仍为 `open 4/6`，不能据受控试验宣称真实长任务整体验收通过。Full、产品部署、发布和 Headroom 接入均未验收。

台账当前 62 条记录，verified 检查点仍为 0/36。矩阵更新为 done 5/5，稳定接口演示与最终依赖差异审计已补证；质量门保留 done（实现完成），明确记录执行失败。M-MCP-CORE 保留 8/9：审计已运行，但新增依赖逐项许可证与引入原因尚无完整归属记录。`ledger revalidate` 已执行，未自动提升任何模块。

[P4](stage-acceptance-P4.md) 与 [P5](stage-acceptance-P5.md) 各有 9 项有限证据、6 项未覆盖，四个目标均未整体验收。五项长任务机制标为受控测试；S1 保留运行时 `task_accepted=false`。阶段生成器只检查证据路径，本次已人工核对声明的通过结果，并补写各未覆盖项的具体原因。

## 证据

- 消费者：[result.json](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/consumer-20261030T085626Z/result.json)
- 受控机制：[final-verification.json](D:/WorkSpace/tmp/ara-lite-capability/long-mechanism-codex-20261030-final/final-verification.json)
- 独立源码审查：[review-final-source-01a18bf7.json](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/review-final-source-01a18bf7.json)
- 依赖修正复审：[review-mcp-version-appendix.json](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/review-mcp-version-appendix.json)
- 独立证据审查：[review-final-evidence-01a18bf7.json](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/review-final-evidence-01a18bf7.json)
- 最终独立审查：[final-count-linked Codex](reviews/final-count-linked-01a18bf7-0a08ac7e-codex-20261030T112534Z.json)、[ara-git-review](reviews/final-count-linked-01a18bf7-0a08ac7e-ara-git-review-20261030T112535Z.json)、[ara-rust-core-review](reviews/final-count-linked-01a18bf7-0a08ac7e-ara-rust-core-review-20261030T112535Z.json)
- 台账：[progress-ledger.md](progress-ledger.md)

## 待处理

- Headroom 工程与许可评估已完成。固定上游为 `17d38e5699b7d7c69ac6addcd762f39e42dd7f00`。option-ext 0.2.0 的 MPL、webpki-roots 1.0.11 的 CDLA 可表达单包精确版本例外；paste 1.0.15 公告为未维护，未提供修复版本，cargo-deny 忽略仅支持 advisory ID，不能限制包版本。根策略 ID 例外也影响共享策略中的 Full 依赖图。
- 最早两次候选审计超时保留。独立 `cargo fetch --locked --offline` 实际退出 0 后，再次对照审计：三项候选退出 0、0 errors、9 warnings，耗时 26.33 秒；仅许可证候选退出 1，唯一 error 为 paste 未维护。原策略、上游 manifest/lock/metadata 共 7 个 SHA 前后不变。详见 [候选审计结果](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/headroom-postfetch-candidate-dry-audits.json)。
- 用户于2026-10-30批准三项，含paste根策略公告ID例外，已应用根`deny.toml`。实际固定H0审计退出0、0errors/9warnings、25.099秒，独立复审无发现；H0记done6/6。P2-f仍未接入，原55检查点未完成。详见[批准与义务](headroom/dependency-policy-20261030.md)和[H0收尾](headroom/closeout-20261030.json)。
- 符号链接权限及真实长任务仍有未通过或未覆盖项；deadline夹具与公网检查已在后续针对性复验通过。最新差异未重跑完整门禁；60.4、61.2、最终检查点不勾选。
- 本轮独立审查覆盖相对 `4640b9f` 的明确变更，未把历史审查缺口改写为已完成的独立审查。

## 本轮过程中的无效或失败记录

- 首次消费者门禁因并行源码修改而版本失配，记录为 stale，未启动模型。冻结后重跑通过。
- 首轮全量检查发现 MCP 新增 wildcard 失败，在构建阶段停止自有进程树；修正后重新执行。原输出保留。
- 历史默认 sidecar 在一次受控复验中启动拒绝，0 模型请求；重建后使用新目录复验通过。未覆盖原失败记录。

## 收尾状态

外部规格61.1、61.3（本轮范围）、61.4（记录与通知过程）已完成，61.2与任务61整体仍未通过。最终钉钉通知于2026-10-30 19:31:48+08:00确认errcode=0，起点为16:40:33.981+08:00，累计10274秒。后续Headroom三项已全部获批准并应用；本次追加步骤完成后Tick结束，不再等待同一决定。

根策略变更后的Headroom复验只覆盖固定H0依赖图；Headroom批准阶段CodeVersion为`96070a37ef538298118ae240110ddd84eb7951c9f98bdc8c84fee13ab68c3204`，根策略SHA为`b14368d2baec9d53ea187cea02f0c501943f8956fd345ff22da5a10337efd9f5`。原final-round记录保留其执行时`0a08ac7e`快照和未批准状态，作为历史证据。本次没有重跑Lite完整门禁或Full审计，也没有提升其验收状态。H0复用原构建与行为收据；当前缓存exe缺失，未重建或重新运行，原LNK4098原因仍未查。

## 任务61修复尝试更新

用户要求继续尝试后，deadline夹具按实际截止契约修正，最终compaction6/6通过；公网原精确ignored用例1/1通过（7.06秒），DDG返回官方Python文档结果，fetch成功，未改搜索代码或配置。旧公网通用错误同时覆盖非成功响应与challenge，不能仅据旧日志确定具体HTTP状态。

本次唯一新增代码差异是JSONL的Python测试文件。独立复审0项未解决发现；中间UI丢帧和持久计时假SLA方案均撤除，原始记录保留。当前CodeVersion为`886470acb34d96725244c43194c4fc125be289f90407d7b662fc0a8868b8f5f6`；最终测试源码SHA为`377ee03b07d0b410ab8d99638aaad049872da7d8a410bd96640e72da5fa57dcf`。生产Rust源码与二进制未改，本次执行绑定原sidecar SHA；没有重跑整套11命令或全115项Python测试，不将不同快照的结果合并声称新完整门禁通过。

见[本次汇总](task61-repair-20261030-codex.json)与[案例修复记录](CASE-20261030-COMPACTION-DEADLINE.md)。当前台账62条、verified0/36。剩余实际失败为用户决定保留的两项symlink权限测试；原55检查点与真实长任务仍open，P2-f保持未接入。

本次修复结果通知于2026-10-30 21:55:00+08:00确认errcode=0。当前会话起点16:40:33.981+08:00，截至通知累计18866秒（含等待）；Tick追加步骤17项全部收尾，任务61验收仍未通过。

## 提交准备

用户已授权提交并推送本轮Lite改动。本提交标为`WIP (unfinished)`，包含已审查的36个代码/配置文件及本轮证据与台账。HandWave与通用知识文档的既有WIP保留在工作区。任务61仍未通过，符号链接权限阻断、完整新快照门禁和真实长任务验收缺口不因提交而关闭。
