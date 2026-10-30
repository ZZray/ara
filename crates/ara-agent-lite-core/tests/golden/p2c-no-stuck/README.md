# p2c-no-stuck：未配置 Stuck_Detector 阈值的黄金序列

## 目录

- 内容：本目录各文件的含义
- 采集偏差：为什么是在 P2-c 代码之后采集的
- 规范化：比较前处理了哪些易变值
- 重新生成：怎样更新本目录

## 内容

需求 18.8、任务 47.1。由 `tests/stuck.rs` 的 `unconfigured_detector_emits_nothing_and_injects_nothing` 读取和比较。

场景：`build` 工具每次都返回错误 `exit 1`，模型连续调用 4 次后回复文本 `gave up`，`max_calls` 为 8，`policies.stuck` 为空。

- `events.json`：事件 `type` 序列（按下文规范顺序）。
- `session.json`：Run 结束后 Session journal 的全部消息。

## 采集偏差

任务 47.1 要求在 P2-c 代码合入之前采集。实际采集时 `stuck_detector.rs` 及其 Core 接入已经存在，本黄金序列取自现有代码的“未配置阈值”路径，不是改动前二进制的输出。它能发现之后的改动对该路径的影响，但不能证明 P2-c 合入本身没有改变该路径。这一偏差需记入执行记录。

## 规范化

与 `../p2b-no-gate/README.md` 的“规范化”一节相同，实现见 `tests/golden_support/mod.rs`。

## 重新生成

```powershell
$env:ARA_UPDATE_GOLDEN='1'; cargo test --test stuck; Remove-Item Env:ARA_UPDATE_GOLDEN
```

默认测试只比较，不写文件。
