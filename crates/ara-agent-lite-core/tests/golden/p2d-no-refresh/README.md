# p2d-no-refresh：未配置 Compaction_Refresh 的黄金序列

## 目录

- 内容：本目录各文件的含义
- 采集偏差：为什么是在 P2-d 代码之后采集的
- 规范化：比较前处理了哪些易变值
- 重新生成：怎样更新本目录

## 内容

需求 19.7、任务 50.1。由 `tests/compaction_refresh.rs` 的 `without_the_policy_no_refresh_message_or_event_appears` 读取和比较。

场景：Run 1 写 `a.txt`、`b.txt` 并更新计划后输出约 16 KiB；随后改动磁盘文件，Run 2 以约 16 KiB 的目标启动，声明的上下文窗口迫使 Run 2 先压缩 Run 1。Run 2 的 `policies.compaction_refresh` 为空。

- `summary.json`：压缩后 context sidecar 中的摘要文本。
- `projection.json`：压缩后第一个 agent 请求实际发送的 `messages`。
- `session.json`：Run 2 结束后 Session journal 的全部消息。
- `events.json`：Run 2 的事件 `type` 序列（按下文规范顺序）。

## 采集偏差

任务 50.1 要求在 P2-d 代码合入之前采集。实际采集时 `compaction_refresh.rs` 及其在 `transform_context` 中的接入已经存在，本黄金序列取自现有代码的“未配置刷新”路径，不是改动前二进制的输出。它能发现之后的改动对该路径的影响，但不能证明 P2-d 合入本身没有改变该路径。这一偏差需记入执行记录。

## 规范化

与 `../p2b-no-gate/README.md` 的“规范化”一节相同，实现见 `tests/golden_support/mod.rs`。本目录还依赖其中两条：当前 Run 预算载体里的 `milliseconds_remaining` 置 0；16 KiB 填充文本记为 `%REPEAT(c,n)%`。

## 重新生成

```powershell
$env:ARA_UPDATE_GOLDEN='1'; cargo test --test compaction_refresh; Remove-Item Env:ARA_UPDATE_GOLDEN
```

默认测试只比较，不写文件。
