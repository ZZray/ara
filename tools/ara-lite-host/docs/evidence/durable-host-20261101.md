# 持久任务 Host 实现与验证

日期：2026-11-01，Asia/Taipei。状态：限定参考 Host 的实现与运行验证通过，最终证据审计待记录。产品接入和发布门禁为 OPEN。

## 版本与范围

基线 `e229fb9ab800b46313047b89b73ac41e60c9efb4` 加本轮工作区变更。交付源码、测试、配置、README 共 14 份文件及二进制的 SHA-256 见 [snapshot.json](durable-host-20261101/snapshot.json)。执行使用该快照对应的原生 `ara-lite-host.exe`。

思想来源及此前 TaskSet 验证见 [TaskSet 记录](../../../../crates/ara-agent-lite-harness/docs/evidence/task-set-20261101.md)。本期完成适合 Lite 的持久编排机制；未复制 DeepSeek 代码，也没有实现其完整产品。

| 保留不动 | 本次修改 | 明确延期 |
|---|---|---|
| Full/OMP、上游锁、既有 ara-lite 协议与实现、产品安装、他人 WIP | 独立可选 `tools/ara-lite-host`，连接公开 Store 与 Harness；补充 Lite 宿主知识 | 外部平台连接器、产品迁移、部署、跨平台验收、性能收益对照 |

业务 Task、worker、generation、lease、人审仍归 ara-lite Store；原 Session、模型循环、累计预算、取消与效果结算归 Harness/Core。Host SQLite 只保存事件、执行绑定、问题、来源授权和 outbox。锁文件从既有 Harness 锁起步，新增 ara-lite/SQLite 所需包，既有协调/Core/Harness 的 Cargo 文件不变。

## 方案覆盖

| 方案 | 实现与可观察结果 |
|---|---|
| 接待和执行分离 | 有界并发，A 等待人工时 B 完成；状态查询不调用模型 |
| 持久事件与任务归属 | source/event_id 加完整内容摘要去重；后续输入必须带原 Task/generation，校验 actor、工作区及授权 revision |
| 受控主动工作 | off/observe 不启动推断任务，auto 才允许；明确请求仍须授权 |
| 重启恢复 | 先存投递意图；中断 Dispatching 转 Unknown，禁止自动重投；原 Session、预算、截止、适配与策略指纹保持 |
| 人工交接与验收 | 持久问题在原回调等待；原 Task 人审、返工和累计预算继续，核对实物与已结算收据后才提交业务审核 |
| 按需取源 | 明确跨 Task 授权、version/hash、撤销和 UTF-8 窗口；来源上限 8 MiB、窗口上限 16 KiB |
| 低噪声通知 | 问题、审核、失败和最终结果使用稳定 outbox ID；输出前持久预留、输出后显式 ACK；Unknown 须明确结算 |
| 外部 Watchdog | 显式运行、只管理自有子进程、有限重启、持续停止标记；活着但心跳过期只报告 |
| 可运行接入 | 本机可信操作员 stdin/stdout JSONL；受限写文件、提问和来源工具；未安装服务或监听网络端口 |

接口、配置和恢复操作见 [README](../../README.md)。模型工具输出不能修改授权；本机 actor 字段本身不构成外部身份认证，工具 allowlist 也不是 OS 沙箱。

## Rust 验证

Windows，现有 Rust 工具链，`CARGO_BUILD_JOBS=2`，共享 target 位于 `D:/workspace/env/cache/rust-target`。最终 [测试输出](durable-host-20261101/rust-tests.log)：**25/25 通过，0 failed/ignored**，其中 Watchdog 9、Journal 7、Runtime 9。编译 1.10 秒，三组执行分别 1.82、0.03、4.42 秒。

```powershell
$env:CARGO_BUILD_JOBS='2'
cargo test --manifest-path tools/ara-lite-host/Cargo.toml --target-dir D:/workspace/env/cache/rust-target --locked --offline --workspace --all-targets
cargo clippy --manifest-path tools/ara-lite-host/Cargo.toml --target-dir D:/workspace/env/cache/rust-target --locked --offline --workspace --all-targets -- -D warnings
cargo build --manifest-path tools/ara-lite-host/Cargo.toml --target-dir D:/workspace/env/cache/rust-target --locked --offline
cargo fmt --manifest-path tools/ara-lite-host/Cargo.toml --all -- --check
```

最后 Clippy 通过（0.70 秒），构建通过（1.72 秒）。最终 fmt、所属 tracked diff whitespace 检查及新 Host 文件 whitespace 检查通过；14 份源码和二进制哈希一致，四份所属文档的 44 个本地链接均存在。Doc tests 通过但 0 例，不计行为测试。此前 Harness 同源码的 69/69 全目标测试见 TaskSet 记录；本轮核对其六份源码快照未变，没有重复执行这 69 项。

## 原生进程与真实模型

最终五场景全部通过，总耗时 **88.237 秒**。汇总见 [process-result.json](durable-host-20261101/process-result.json)，每场景保留独立收据。

```text
python tools/ara-lite-host/tests/reference_process.py --binary D:/workspace/env/cache/rust-target/debug/ara-lite-host.exe --output NEW_EVIDENCE_DIRECTORY --live-endpoint AUTHORIZED_HTTPS_V1 --live-model deepseek-v4.1-flash --live-key-env RAY_GATEWAY_KEY
```

真实试验使用已核对的授权 OpenAI-compatible 路由，凭据仅从环境读取。脚本作为测试操作员发送回答及明确的 review 决定，验证人审入口和状态流转；没有实际产品用户参与验收。仓内不保存路由私有配置、凭据或完整原始状态目录。

| 场景 | 实际结果 |
|---|---|
| [并发、来源、人工返工](durable-host-20261101/workflow/receipt.json) | A 提问等待，B 完成；B→A 显式来源读取；A changes_requested 后沿原 Session 完成，2 Runs / 6 calls；实际文件检查通过 |
| [投递中 kill/restart](durable-host-20261101/crash/receipt.json) | Dispatching→Unknown，不重投、不偷开模型；原问题回答与明确结算后沿原 Task/Session 恢复。2 Runs / 10 次逻辑预算计数，实际 HTTP 调用 3 次：中断 Run 保守计满已准入的 8 次上限 |
| [待人审 kill/restart](durable-host-20261101/review-recovery/receipt.json) | 原候选经校验恢复；同 offer blocked→明确 recover→pass；模型调用始终 2，原生 journal/fence 不变；ACK 后第二次重启不重发结果 |
| [畸形 stdin](durable-host-20261101/malformed-stdin/receipt.json) | 超长 ASCII 行丢弃到换行，Stop 样式后缀不执行；合法 status 仍可用。非法 UTF-8 仅报一次并关闭读端，后缀不执行，无停止标记或模型调用 |
| [真实 DeepSeek v4.1 Flash](durable-host-20261101/live/receipt.json) | 实际 request_input、source_list/source_read、write_result，人审 pass；产物字节为 `42`，SHA-256 `73475cb40a568e8da8a045ced110137e159f890ac4da883b6b17dc651b3a8049`。1 Run / 6 calls，上限 12 calls、3 Runs、90 秒，max output 4096 |

普通 Harness 结果仍为 `task_accepted=false`，业务 passed 由 Host 人审产生。待人审崩溃后的 pass 使用独立 `recovered_human_review`，明确 `native_fence_retained=true`、`not_a_harness_task_outcome=true`；没有伪造 Released 或 TaskOutcome，没有清 fence，该 Session 不再启动。changes_requested 才可在原剩余预算内明确恢复。

## 发现、修复与审查

独立 Codex `lite_architecture_check`、`durable_safety_design`、`task_set_tests` 从实施、维护及并发/权限三个视角一致建议独立 Host 桥接。源码 PRE/POST 审查发现并修复：准入后 handle 所有权丢失、关机意外派发、无效收据提前提交业务验收、回调 phase 被旧 Running 覆盖、审核操作身份复用、claim-before-Starting 恢复缺口、来源边界和 stdin 后缀解析。最终源码 POST APPROVE，无未解决实质发现。审查者阅读源码与证据，执行结果由主代理运行核对。

首次原生运行暴露 heartbeat 与 submit 的 version 竞争；修复后保留原验收条件。claim 已提交而执行尚未启动时，旧路径拒绝恢复；新增明确 `not_started=true` 且证明原 Session/journal/evidence 不存在的恢复入口，继续原 Run ID 与预算。测试保留这条进程切点。

Windows 心跳文件替换曾返回 WinError 5，导致受控子进程退出及一次额外重启。修复只对 OS 5/32/33 做最多 8 次、5ms 间隔的有界重试，失败继续报错；读取句柄显式允许 delete sharing 并在解析前关闭。新增真实拒绝删除句柄的确定性测试，验证短暂竞争恢复与持续失败上界，没有放宽重启次数预期。

最终独立 [point-delivery-audit](durable-host-20261101/review.json)：`lite_architecture_check` 给出 **APPROVE_SCOPED**，无发现。其直接核对 14 份源码及二进制、25 测试日志、五场景原始收据、实际产物、原生工具结果、候选事件 hash、只读协调 DB、原预算恢复及保留 fence。review fence 的收据 hash 使用紧凑有序 JSON 表示，未声称等于 pretty-file 原始字节 hash。审计没有重跑编译、模型或产品；最后补充的 verification.json 和测试操作员说明由主代理复核。产品与发布保持 OPEN。

## 未覆盖与发布门禁

- `cargo deny --locked --offline check`：advisories 通过；bans/licenses/sources 失败，涉及既有 rpi Git 来源、webpki-roots CDLA、wildcard/bans 等策略。没有放宽策略，不能据此发布。
- 最终再次运行 `python scripts/verify_bootstrap.py`，仍报告 61 条历史断链；所属文档链接检查通过，不改他人 WIP 或归档。
- 未连接 HandWave、AI Code Review、LabSlot 或外部消息平台；未提交、推送、部署、修改安装或安装后台服务。
- 未验收跨平台、多日长跑、大规模并发/数据迁移、真实业务工具及同模型同任务性能 A/B。有限参考任务通过不能证明这些范围或业务收益。
- 本机原始状态保存在执行收据注明的 cache 目录；含私有协调状态，不整体复制入仓。仓内仅保留选定收据、哈希和测试日志。
