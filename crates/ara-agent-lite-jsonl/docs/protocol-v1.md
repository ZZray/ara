# JSONL v1 协议

每条消息是严格 UTF-8 JSON 对象，以 LF 结束。整帧含 LF 不超过 262144 字节；
工具 `result` 子对象不超过 131072 序列化字节。输入拒绝未知字段、错误版本、
重复回复和未请求的回复。stdout 仅作协议通道；stderr 只报告固定错误类别。

## 初始帧

一个进程只接受一个 `start_task`。下面是结构示例；Host 必须提供实际模型入口、
有效的绝对截止时间和新目录，不应直接执行示例中的占位值。

```json
{
  "kind": "start_task",
  "version": 1,
  "task": {
    "task_id": "task-1",
    "criteria_revision": "criteria-v1",
    "authorization_revision": "grant-v1",
    "kind": "Coding",
    "evidence_dir": "D:/isolated/new-task-evidence",
    "max_runs": 2,
    "max_calls": 8,
    "deadline_unix_ms": 0
  },
  "run": {
    "run_id": "run-1",
    "objective": "Use the registered business tools and submit the actual artifact.",
    "cwd": "D:/isolated/workspace",
    "session_path": "D:/isolated/state/session.json",
    "environment_state": "D:/isolated/state/environment",
    "device_fp": "host-device",
    "session_key": "host-session",
    "mode": "execute",
    "endpoint": "http://127.0.0.1:12345/v1",
    "model": "host-model-id",
    "max_calls": 8,
    "max_seconds": 120,
    "max_output": 2048,
    "model_resolution": {
      "kind": "host_declared_loopback",
      "capabilities": { "context_window": null, "max_tokens": 8192, "input": ["text"] }
    },
    "tools": [{
      "name": "lookup",
      "description": "Host-owned lookup",
      "parameters": { "type": "object", "properties": {}, "additionalProperties": false },
      "allow_in_plan": true
    }]
  },
  "cancel_grace_ms": 5000,
  "emit_events": false
}
```

`model_resolution` 也可以为 `{"kind":"catalog"}`。`max_output` 范围为 1 至 8192；
`cancel_grace_ms` 为 1000 至 15000，默认 5000。`kind` 使用公共 Harness 的
`TaskKind` 编码。凭据只来自子环境 `ARA_AGENT_MODEL_TOKEN`。

可选 `run.length_recovery` 使用字符串 `"stop"`（默认）或 `"replan_once"`。
后者仅允许 Execute，并由 Core 在同一 Run 最多安排一次无工具 Length 重规划。
它不增加预算、不修改原截断消息；再次无工具 Length、未知效果、取消和持久化失败仍停止。
`prepare_reply` 不能更换这一固定 Host 策略。

可选 `run.generation_policy` 是 Core 的封闭枚举：`"provider_default"`（默认）或
`"thinking_type_disabled"`。后者在解析后的 `openai-completions` 路由上发送固定
`thinking.type=disabled`，其余协议由 Core 在模型和工具调用前拒绝。宿主负责确认
实际服务支持这一契约，不能从模型名称推断。它不改变模型、输出上限、累计调用数或截止时间。
策略固定于整个 Task，反馈后的 Run 和同 Run 长度恢复继续沿用。
null、对象和未知字符串被拒绝；初始帧仍不接受任意 provider body 或 sampling 覆盖。
旧 JSONL 程序不支持这个字段，消费者必须固定支持该字段的源码或镜像。

## 工具回调

sidecar 发 `tool_call`：

```json
{
  "kind": "tool_call", "version": 1,
  "rpc_id": "rpc-1", "task_id": "task-1", "run_id": "run-1",
  "native_call_id": "call-1", "tool_name": "lookup",
  "args_sha256": "original-argument-hash", "arguments": {}
}
```

Host 回 `tool_reply`，精确复制原关联字段，不能重算或改写参数后复用旧 hash：

```json
{
  "kind": "tool_reply", "version": 1,
  "rpc_id": "rpc-1", "task_id": "task-1", "run_id": "run-1",
  "native_call_id": "call-1", "tool_name": "lookup",
  "args_sha256": "original-argument-hash",
  "result": {
    "value": { "found": true }, "is_error": false,
    "settlement": "Settled", "operation_receipt": "original-host-receipt"
  }
}
```

`settlement` 为 `Settled` 或 `Unknown`。传输完成、业务成功和效果结算分别表达；
已知失败可以结算，未知效果会阻止成功和自动续作。Host 保管原操作收据，sidecar
记录回调关联与结果。工具文本不能触发控制回调。

## 产物检查和续作

所有控制请求及回复都有 `kind/version/rpc_id/task_id/run_id`。

| sidecar 请求 | 内容 | Host 回复 |
| --- | --- | --- |
| `verify_request` | `candidate` 或 `candidate_ref`，二选一 | `verify_reply`，包含 `receipt` |
| `prepare_request` | `continuation` 或 `continuation_ref`，二选一 | `prepare_reply`，包含 `run_id_next` 和 `objective` |

`candidate` 是公共 Harness 的完整 `Candidate`，包含 `basis/output/goal`。
Host 原样复制完整 basis 到 `receipt.basis`，实际保存证据后返回：

```json
{
  "basis": "copy-the-complete-actual-candidate-basis-object",
  "committed": true,
  "settlement": "Settled",
  "verdict": { "NeedsChanges": { "feedback": "Actual check failed; correct the authorized artifact." } },
  "evidence": ["original-host-check-receipt"]
}
```

这里的 basis 字符串仅说明位置；线上必须是原始对象。`verdict` 的其他编码是字符串
`Verified` 和 `Unknown`。`continuation` 包含原 `previous`、检查 `receipt`、
`next_run_number`、`remaining_calls` 和原 `deadline_unix_ms`。
Host 的新 objective 应包含实际失败反馈；reply 无法更换其余配置。

## 原件引用与终态

超限候选和续作使用 `{ "path": "absolute-original-path", "sha256": "actual-bytes-sha256" }`
引用。客户端只接受原 Task 证据目录内、实际字节 hash 匹配的文件。Python 客户端
读取上限为 8 MiB，Node 默认为 16 MiB；Rust 原件读取函数没有该 SDK 限额。
这些检查不构成不可信文件系统沙箱。引用中的完整 JSON 必须与本地权威对象一致。

`task_result` 总是包含 `outcome_ref`，并附 `task_id/stop/task_accepted/runs/observed_calls/`
`fence_presence/reconciliation_fence/local_callbacks_finished`。客户端核对引用原件与摘要；
`Released` 使用 `released.json`，其余使用 `outcome.json`。
`task_accepted` 始终为 false，产品自行接受业务 Task。

## 取消、断连和事件

Host 可发送 `{"kind":"cancel","version":1,"task_id":"task-1"}`。
sidecar 对在途回调发 `callback_cancel`，包含原关联字段及 `reason`。
Host 需要在结算原操作后回复同一 RPC；不能用新操作或新 Session 代替。
grace、EOF 或进程退出无法证明远端回调已结束或效果已回滚。

开启 `emit_events` 后会收到 `event`，带 `seq/event_type` 和
`delivery="best_effort_not_complete_evidence"`。慢观察者可以丢 UI 事件，原 Core 事件
仍由 Harness 持久化。控制帧写失败会保守停止；不重发原 RPC。
若终态传输失败，先查原件和收据，不能仅凭没收到 `task_result` 推断外部效果。

原始 Core 的 `length_recovery` 事件标记 `scheduled`，包含截断原消息的全局 raw index、
typed source SHA、自动控制 reference ID/SHA 和当时预算。它不是 HTTP 已执行或恢复成功
的收据。`source_hash_format=serde_json_agent_message` 要求将原记录解析成固定版本的
typed `AgentMessage` 后直接 serde 序列化；不能用 checkpoint 字典的 JSON hash 代替。
固定 rpi 的 `Content` typed 编码包含重复的同值 `type` 标签，`Value` 会将其归一。
原消息 typed hash、Journal 文件/链 hash、控制正文 UTF-8 hash 分别校验。
