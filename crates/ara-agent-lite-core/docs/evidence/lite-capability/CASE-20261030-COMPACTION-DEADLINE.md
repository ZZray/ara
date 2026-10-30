# 压缩 deadline 用例未稳定进入预期路径

状态：测试夹具已修复，最终6项进程回归通过；历史失败保留。2026-10-30，`dev` / `4640b9f`，本轮候选差异 `1ac911a5`。

## 已观察结果

当前候选 JSONL Python 全量运行 115 项，114 通过；`test_original_deadline_bounds_inflight_summary_across_runs` 失败。该用例固定 Task deadline 为 5000ms，要求先完成第一轮工具及 Host 审查、进入下一轮实际 summary HTTP，然后由继承的 deadline 中止。

- 全量运行：只有两次 agent HTTP、没有 summary。原 Task deadline 在第一次 Host review 期间到达，结果为 `Cancelled`，未覆盖目标场景。
- 一次精确复验：三次 HTTP、一次 summary，没有后续 agent HTTP；但最终为 `Cancelled`，summary `model_result` 数为 0，未满足原 `ReconciliationRequired` 和 summary 结果断言。该次用时 7.641 秒。

两次均使用绑定 SHA 的实际 sidecar。精确复验解释器为隔离环境 Python 3.13.14，未改源码、预期或 ignore，也没有再次手动重试。

## 源码支持与结论边界

Harness `runner.rs` 在 Run 准入持久化之前将剩余 Task 秒数向下取整；Core `agent.rs::execute` 在执行入口创建相对 deadline。准备和证据 I/O 会消耗两者之间的时间，外层原 Task deadline 可能先到并取消当前操作。测试注释中的“Core 的继承 deadline 可能更早停止”不保证每次都由 Core 分支先结束。

现有结果没有证明产品漏发请求或在 deadline 后继续请求。也不能只扩大 5000ms 或允许 `Cancelled` 就声称修复：provider 的固定等待时间及 summary 结果断言仍需一起设计和验证。后续应建立可观察的时序控制，保留实际 summary 进行中、无后续 HTTP、原 deadline 未重置、原 journal 未改写和停止状态的验收。

## 证据

- 原全量：[Python stderr](D:/WorkSpace/tmp/ara-lite-capability/gates/M-P5-QUALITY-20261030T095856Z/10-ara-agent-lite-jsonl-python.stderr.txt)
- 原 case：[证据目录](D:/WorkSpace/tmp/ara-lite-capability/gates/M-P5-QUALITY-20261030T095856Z/python-evidence/compaction/test_original_deadline_bounds_inflight_summary_across_runs-1793355593564350500)
- 精确复验：[证据目录](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/deadline-exact-20261030T103210Z)
- 分析与源码指针：[audit.json](D:/WorkSpace/tmp/ara-lite-capability/current-01a18bf7/compaction-deadline-fixture-audit.json)

以上为修复前的失败记录，原始产物保留。后续修复与最终验收如下。

## 2026-10-30 修复与复验

重新读取实际分支后确认：deadline/cancel不要求summary `model_result`，该断言只属于length/fake_source用例。先前分析将它误归到deadline，仓外audit已补纠正。源码还明确允许原Task deadline先取消并join同一个Core，最终记`Cancelled`；Core先到则可能记`ReconciliationRequired`。旧测试强求唯一终态，且把落盘完成限在deadline后1秒，超出了现有契约。

本次只修改`test_compaction_process.py`：Task输入窗口从5秒改15秒以容纳启动与准入，绝对deadline在两个Run中始终相同；summary真实在途且至少提前1秒出现，服务端等待显式释放，超时明确失败。按实际边界逐项匹配停止原因与`core_joined/stopped`收据；保留3次HTTP、1次summary、1次工具效果、1次Host审查、无新产物/compact、原始引用及fence检查。最终读取持久原生终态，不依赖UI帧或文件首次到达时间。25秒watchdog用于触发夹具取消；随后的既有客户端清理仍可能增加墙钟时间，不把它写成产品结算SLA。

最终6项compaction真实子进程用例全部通过，测试耗时56.614秒、进程57.287秒；测试源码SHA为`377ee03b07d0b410ab8d99638aaad049872da7d8a410bd96640e72da5fa57dcf`，sidecar SHA仍为`2103dbf693b6ab4a71835526351e04c7fe2e1cfc32f50b0b5ea78fa745dbf3b9`。deadline样本的summary提前10.806秒在途，Run上限14→11秒；最终为Core先到的`ReconciliationRequired`，原Core已join。完成时间晚于原deadline 83ms仅为本次观察，不作为延迟保证。

两个中间方案保留：UI计时因未收到best-effort终态而失败；持久文件计时虽单次通过，复审确认仍包含未承诺的落盘延迟，因此撤除。它们均不替代最终6项结果。

独立Codex按`ara-git-review`与`ara-rust-core-review`复审最终单文件差异，0项未解决发现。见[最终复审](reviews/task61-repair-independent-20261030.json)与[本次汇总](task61-repair-20261030-codex.json)。本次未改生产计时器、未重建Rust；两个symlink权限测试及完整新快照门禁仍未验收。
