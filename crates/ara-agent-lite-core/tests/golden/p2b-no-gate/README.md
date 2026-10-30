# p2b-no-gate：未注册 Completion_Gate 的黄金序列

## 目录

- 内容：本目录各文件的含义
- 采集偏差：为什么是在 P2-b 代码之后采集的
- 规范化：比较前处理了哪些易变值
- 重新生成：怎样更新本目录

## 内容

需求 17.7、任务 44.1。由 `tests/completion_gate_golden.rs` 读取和比较。

场景：loopback provider 依次返回一次 `probe` 工具调用和文本 `answer-2`，`max_calls` 为 4，`policies.completion_gate` 为空。

- `events.json`：事件 `type` 序列（按下文规范顺序）。
- `stop.json`：`RunResult` 的 `code`、`error`、`text`。
- `journal.json`：Run 结束后 Session journal 的全部原始条目。

## 采集偏差

任务 44.1 要求在 P2-b 代码合入之前采集。实际采集时 P2-b（`completion_gate.rs` 及其在 `get_follow_up_messages` 中的接入）已经存在，因此本黄金序列取自现有代码的“未注册 gate”路径，不是改动前二进制的输出。它能发现之后的改动让未注册路径发生的变化，但不能证明 P2-b 合入本身没有改变该路径。这一偏差需记入执行记录。

## 规范化

`tests/golden_support/mod.rs` 的 `Normalizer` 在比较前处理：

- 临时根目录（含 `\\?\` 前缀、反斜杠转义和正斜杠形式）替换为 `%ROOT%`，loopback 地址替换为 `%ENDPOINT%`。
- 数值字段 `timestamp`、`timestamp_ms`、`milliseconds_remaining`、`duration_ms`、`elapsed_ms`、`deadline_unix_ms` 置 0，嵌在字符串中的同名 JSON 字段也置 0。
- 24 位小写十六进制串（`fresh_id`，如 Run ID）按首次出现编号为 `%ID-n%`，64 位十六进制串（SHA-256）编号为 `%SHA-n%`；同值同号，保留相等关系。
- 连续 64 个及以上的同一字符记为 `%REPEAT(c,n)%`。
- 事件顺序：每个 `model_result` 移到与其竞争的连续 `text_delta` 之前，与 `event_compat.rs`（DR-P4-1）的规范顺序相同。

## 重新生成

```powershell
$env:ARA_UPDATE_GOLDEN='1'; cargo test --test completion_gate_golden; Remove-Item Env:ARA_UPDATE_GOLDEN
```

默认（未设置该变量）测试只比较，不写文件。重新生成会覆盖兼容基线，需要在评审中说明原因。
