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

## MCP server（可选）

start 帧可带两个可选字段。不带时帧与行为和以前相同。

| 字段 | 类型 | 说明 |
|---|---|---|
| `mcp_servers` | 对象 | 与 Claude Code `.mcp.json` 的 `mcpServers` 相同：键是 server 名，值是 stdio（`command`、`args`、`env`）或 `{"type":"http","url":..,"headers":..}`。`${VAR}` 与 `${VAR:-default}` 按 sidecar 进程环境展开。解析由 `ara-agent-lite-mcp` 完成，规则见该 crate 的 README。 |
| `mcp_options` | 对象，可选 | `required`（server 名数组，其中任一连接失败即拒绝 start）、`startup_timeout_ms`、`call_timeout_ms`。没有 `mcp_servers` 时出现此字段视为配置错误。 |

- sidecar 在 Task 准入之前连接全部 server。配置无效、`required` 中有未配置的名字，或 `required` server 连接失败时，stderr 写 `start_config_rejected`，退出码 2，stdout 零帧，不向上游发请求。
- 连接完成后、任何 Task 帧之前输出一帧 `{"kind":"mcp_status","version":1,"task_id":..,"servers":{<name>:{"status":"connected","tools":n,"instructions":..}|{"status":"failed","reason":..}},"excluded":[{"server":..,"tool":..,"reason":..}]}`。错误与原因中不出现 `env`、`headers` 的取值。
- MCP 工具以 `mcp__<server>__<tool>` 命名加入本次 Task，与 Host 工具或内置工具重名时拒绝 start。Plan 模式下不提供 MCP 工具。Task 结束后 sidecar 关闭全部 server。
- stdio server 继承 sidecar 的环境再叠加 `env`。Python 与 Node 客户端只把白名单变量传给 sidecar（含 `PATH`、`SYSTEMROOT`、`SYSTEMDRIVE`、`TEMP`、`USERPROFILE`、`APPDATA`、`LOCALAPPDATA`、`PROGRAMDATA` 等 Windows 基础变量），server 需要的其他变量写进它自己的 `env`。

## 长度截断停止审查（可选）

start 帧 `length_stop_review` 与 Harness `TaskOptions.length_stop_review` 相同，只在 Execute 模式可用，默认关闭。开启后 Run 因模型输出达到长度上限停止时，sidecar 依次发出：

1. `verify_stop_request`：body 为 `{"snapshot":..}`，超过帧上限时改为 `{"snapshot_ref":{"path":..,"sha256":..}}`。Host 回 `{"kind":"verify_stop_reply","version":1,"rpc_id":..,"task_id":..,"run_id":..,"receipt":<ReviewReceipt>}`。
2. receipt 为 `NeedsChanges` 且还有 Run 预算时，发 `prepare_stop_request`：body 为 `{"stop_continuation":{"previous":..,"receipt":..,"next_run_number":..,"remaining_calls":..,"deadline_unix_ms":..}}`，过大时为 `{"stop_continuation_ref":..}`，内容同时写入证据目录 `jsonl-stop-continuation-<n>.json`。Host 回 `{"kind":"prepare_stop_reply","version":1,"rpc_id":..,"task_id":..,"run_id":..,"run_id_next":..,"objective":..}`。

快照或续作文件在 Host 处理期间被改动时，sidecar 以 `stop_snapshot_changed_during_verify` 或 `stop_continuation_changed_during_prepare` 失败。Python 客户端对应 `verify_stop_snapshot` / `prepare_stop_continuation`，Node 客户端对应 `callbacks.verifyStopSnapshot` / `callbacks.prepareStopContinuation`；开启该选项却缺少回调时，客户端在启动 sidecar 之前报错。

## 未知副作用回执引用

外部工具回调失败，或 Host 回复 `settlement=Unknown` 时，该调用的工具结果 details 为 `{"executed":"unknown","source":"jsonl_external_callback","receipt_ref":{"path":..,"sha256":..}}`。`receipt_ref` 指向证据目录中的 `jsonl-rpc-<rpc_id>-unknown.json`，其中记录 `native_call_id`、`tool_name`、`args_sha256` 与 `reason`（`callback_unknown` 或 `host_declared_unknown`）。Task 结束时 `outcome.json` 的 `unknown_effects` 列出这些调用；`task_result` 帧的键集合不变。Python 客户端用 `parse_unknown_effects(outcome)` 读取，Node 客户端在 `wait()` 结果中提供 `unknownEffects`。

## 恢复被 fence 的 Task（resume，可选）

start 帧顶层可带 `resume`。省略时 start 帧按原有行为执行，帧序列与以前相同。带 `resume` 时，sidecar 不新建 Task，而是对 `task` 与 `run` 指定的 Session 调用一次 Harness `resume_checked`（`refuse_larger_run_caps=true`）。

`task` 必须与原 Task 相同，`evidence_dir` 除外，它必须是新的、尚不存在的目录。`run` 中只有 `run_id`、`objective`、`max_calls`、`max_seconds` 可以不同，其余字段进入 Host authority 哈希；`max_calls`、`max_seconds` 不得大于原 Task 首次准入的值。

| 字段 | JSON 类型 | 必填 | 约束 | 含义 |
|---|---|---|---|---|
| `resume.version` | 整数 | 是 | 等于 1 | resume 子对象版本 |
| `resume.fence_sha256` | 字符串 | 是 | 64 位小写十六进制 | Host 检查过的 fence 文件字节的 SHA-256；fence 已变化时拒绝 |

`resume` 为 `null`、类型不符、缺少子字段或含未知子字段时，stderr 写 `start_config_rejected`，退出码 2，stdout 零帧。

sidecar 在 Harness 准入之前启动 stdin 读取，然后发出一帧结算请求：

| 字段 | JSON 类型 | 必填 | 含义 |
|---|---|---|---|
| `kind` | 字符串 | 是 | `reconcile_request` |
| `version`、`rpc_id`、`task_id`、`run_id` | 整数、字符串 | 是 | 与其他回调相同；`run_id` 是 start 帧的 `run.run_id` |
| `review` | 对象 | 二选一 | Harness `ResumeReview`：`task_id`、`session_path`、`resume_evidence_dir`、`fenced_evidence_dir`、`fence`、`fenced_outcome`、`interrupted_run`、`consumed_runs`、`consumed_calls` |
| `review_ref` | `{"path","sha256"}` | 二选一 | 帧超过上限时代替 `review`，指向新证据目录下的 `jsonl-resume-review.json` |

Host 在 Task deadline 之前回一帧：

| 字段 | JSON 类型 | 必填 | 约束 |
|---|---|---|---|
| `kind` | 字符串 | 是 | `reconcile_reply` |
| `version`、`rpc_id`、`task_id`、`run_id` | 整数、字符串 | 是 | 与对应 `reconcile_request` 完全相同 |
| `receipt.committed` | 布尔 | 是 | |
| `receipt.settlement` | 字符串 | 是 | `Settled` 或 `Unknown` |
| `receipt.tool_calls` | 数组 | 是 | 每项恰为 `{"call_id","settlement"}`；`settlement` 为 `{"kind":"not_executed"}`，或 `kind` 为 `completed`、`failed` 并带字符串 `output`，与 `interrupted_run.open_tool_calls` 一一对应且顺序相同 |
| `receipt.evidence` | 字符串数组 | 是 | Host 自己的操作回执引用 |

请求与回复按现有方式写入新证据目录的 `jsonl-rpc-<n>-request.json`、`jsonl-rpc-<n>-reply.json`。回复写盘后才交给 Harness 校验，是否放行以 Harness 结果为准。请求只发一次，从不重发。

任一拒绝都发生在首次上游请求之前：stderr 只写 `task_admission_rejected`，退出码 2，fence 文件与原有回执不变。Harness 在调用 Host 前就拒绝时（Task 不存在、没有 fence、fence 哈希不符、Task 条款或 authority 改变、预算改变或单 Run 上限增大），stdout 零帧，也不创建新证据目录。已发出 `reconcile_request` 后被拒绝时，新证据目录中写 `jsonl-resume-rejected.json`：`{"category":<类别>,"no_replay":true}`。类别：

| 类别 | 原因 |
|---|---|
| `rpc_mismatch` | 回复的 `rpc_id`、`task_id`、`run_id` 或 `version` 与待处理请求不一致，或它是另一种已知回复 |
| `invalid_reply_fields` | 回复不是合法的单行 JSON、`kind` 未知，或缺少字段、含未知字段、字段类型不符 |
| `stdin_closed` | 回复之前 stdin 已关闭 |
| `reconcile_timeout` | Task deadline 到达：sidecar 发 `callback_cancel`，等待 `cancel_grace_ms` 后拒绝 |
| `resume_cancelled` | 回复之前收到 `cancel` |
| `receipt_uncommitted`、`settlement_unknown`、`evidence_empty`、`call_mismatch` | Harness 拒绝了已关联的回执 |
| `harness_refused` | 其他 Harness 拒绝 |

Python 客户端：Host 提供 `async reconcile_resume(review, cancelled)`。config 含 `resume` 而没有该回调时，在启动 sidecar 之前抛 `ValueError`。回调异常、返回值字段或类型不符、回复帧超过 262144 字节、deadline 前未返回时，不写回复，抛 `ReconciliationRequired`，消息首段为 `reconcile_callback_exception`、`reconcile_receipt_invalid`、`reconcile_frame_too_large` 或 `reconcile_timeout`。不含 `resume` 时收到 `reconcile_request`，或同一次 `run_task` 收到第二个，消息首段为 `reconcile_request_unexpected`。

Node 客户端：`callbacks.reconcileResume(review, {signal, frame})`。`start.resume` 存在而回调缺失时，`startTask` 在 spawn 之前抛 `Error('reconcile_callback_missing')`。失败时不写回复，`wait()` 结果的 `fault` 为 `reconcile_callback_threw`、`reconcile_receipt_invalid`、`reconcile_timeout`、`reconcile_aborted` 或 `reconcile_request_unexpected`，不含回调异常原文和 `modelToken`。`cancel()` 与 deadline 都会中止传给回调的 `signal`。
