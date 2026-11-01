# ara-lite-host

可选的本机参考 Host，将 `tools/ara-lite` 的持久协调服务与 Lite Harness 接起来。
业务 Task、worker 所有权、generation 和人审状态仍由原协调 Store 管理；
模型循环、Session、原 Task 预算和工具结算仍由 Harness 管理。
新 Host 仅保存入站事件、执行绑定、人工问题、来源授权和通知回执。

## 本期能力

| 能力 | 实现和行为 |
|---|---|
| 接待与后台执行分离 | 有界并发执行，接待循环可查询状态、回答、审核和取消；简单状态查询不调用模型 |
| 持久事件收件箱 | `(source, event_id)` 与完整输入摘要去重，原件落盘后处理；不受原协调消息 24 小时保留期限制 |
| 明确的任务归属 | 后续事件带 Task ID 和 generation，重新检查授权 revision；按主题相似自动合并没有启用 |
| 受控主动工作 | `off`/`observe` 不执行推断的新任务，`auto` 才允许；明确请求仍需 actor 和工作区授权 |
| 重启与对账 | 投递中断变为 Unknown，不自动重投；持久执行绑定保留原 Task、Session、授权、模型适配配置指纹、预算与截止 |
| 人工问题 | `request_input` 保存问题和通知，在原工具回调内等待明确回答，不消耗模型轮询 |
| 人工验收与返工 | 实物检查通过后进入原协调 `awaiting_review`；返工沿同一 Harness Task/Session 和总预算继续 |
| 按需取源 | 明确授权来源 ID、版本、hash 和读取窗口，支持跨任务引用及撤销；不复制完整历史 |
| 低噪声通知 | 仅问题、待审结果、失败和最终结果进入 outbox；稳定通知 ID、先输出后 ACK，未知投递需明确结算 |
| 外部 Watchdog | 只管理自己启动的子进程，失败时有限重启；显式停止后不复活，活着但心跳过期只报告并保留管理 |

这些接口为 experimental。它们是共享 Lite 上的参考 Host，不是已部署的 HandWave、
AI Code Review 或 LabSlot 功能。[本期验证记录](docs/evidence/durable-host-20261101.md)
区分受控故障、真实模型和未验收范围。

## 运行

选择隔离的状态目录，以及已存在的 Git 工作区根。控制状态必须在工具工作区之外。
状态目录由本机当前用户管理，不提供 OS 沙箱；不要用生产协调服务的数据目录。
参考程序直接嵌入原 `ara-lite::Store`，不占用默认服务端口、不替换已有服务。

```text
cargo build --manifest-path tools/ara-lite-host/Cargo.toml --locked
ara-lite-host run CONFIG.json
ara-lite-host watch CONFIG.json
ara-lite-host stop CONFIG.json
ara-lite-host start CONFIG.json
```

`run` 用本机 stdin/stdout JSONL 接收操作员命令。`watch` 是显式运行的外部监督进程，
最多重启三次失败子进程，不安装系统服务或后台监控。`stop` 标记持续保留；
只有操作员显式使用 `start` 才清除此标记。正常退出不重启。

配置示例（凭据只写环境变量名）：

```json
{
  "runtime": {
    "state_dir": "D:/host-demo/state",
    "workspace_root": "D:/host-demo/work",
    "authorization_revision": "demo-grant-v1",
    "allowed_actors": ["operator"],
    "max_parallel": 2,
    "max_calls": 12,
    "max_runs": 3,
    "max_seconds": 120,
    "accept_mode": "observe",
    "review_mode": "human"
  },
  "model": {
    "endpoint": "https://YOUR_AUTHORIZED_ENDPOINT/v1",
    "model": "YOUR_MODEL_ID",
    "key_env": "HOST_MODEL_KEY",
    "expected_result": "42",
    "max_run_calls": 8,
    "max_output": 4096
  }
}
```

参考适配器只提供写 `result.txt`、人工问题和授权来源读取，禁用通用本地工具。
其实物验收是“结果文件等于受信任配置的 expected_result”，用于验证 Host 接线。
真实业务实现 `runtime::Adapter`，返回自己的模型入口、业务工具及 `HarnessHost` 实物检查器，
不需要复制 Agent 循环。`verified_artifact` 是显式的 Host 自动验收策略，默认示例采用人审。

## 本机 JSONL 命令

输入每行一个 JSON，最多 64 KiB。事件示例：

```json
{"op":"event","event":{"source":"local-operator","event_id":"request-001","actor":"operator","authorization_revision":"demo-grant-v1","explicit":true,"body":{"kind":"work","title":"读取来源并生成结果","workspace":"D:/host-demo/work/task-a","objective":"先询问操作员是否就绪，再读取已注册来源，将原文写入result.txt。"}}}
{"op":"status"}
```

响应返回原事件 key；`status` 列出业务状态、执行阶段、待处理/拒绝/未知事件及通知。
后续 `followup`、`answer`、`cancel` 事件的 body 必须包含原 `task_id` 和 `generation`。
`answer` 另带 `question_id`、`text`；`followup` 另带 `text`。

控制命令与普通来源事件分开：

| op | 字段与用途 |
|---|---|
| `review` | `task_id, review_id, snapshot, decision, reason`；明确原候选，决定 pass/changes_requested/blocked |
| `ack` | `notice_id`；只确认已输出的通知，不接受业务 Task |
| `settle_notice` | `notice_id, evidence`；操作员核对 Unknown 通知的实际投递结果 |
| `settle_event` | `key, state, evidence`；将 Unknown 结算为 applied/unapplied/rejected，不自动重投 |
| `register_source` | `source` 元数据：id/task_id/generation/version/path/sha256/allowed_tasks/active；撤销需提高版本 |
| `inspect_recovery` | `task_id`；读取原 binding、fence 哈希及未闭合工具调用 |
| `recover` | `task_id, not_started, evidence, proof`；明确核对原效果后恢复，proof 格式见源码的 `Reconciliation` |
| `stop` | 保存显式停止标记，取消并等待原 Task 结算，不再派发 queued 工作 |

stdin 是受信任的本机操作员边界。`actor` 和 `explicit` 字段本身不提供身份认证；
GitHub、飞书等外部适配器必须先完成平台鉴权，再调用此层。当前没有连接这些平台，
也不会自动向其他人发送消息。模型看到的来源与工具输出不能修改配置或授权。

## 恢复规则

- `Dispatching` 事件在崩溃后为 Unknown。缺少内存回执不证明没有投递，不能重发来试。
- claim 已保存但原执行尚未准入时，操作员可以证明没有原 Session、原生 journal 或
  evidence 目录，再以 `not_started=true` 使用原 Run ID/预算启动；其余执行走原 Harness resume。
- `recover` 不能增大原调用数、Run 数或截止。适配配置、actor 规则、工作区授权或
  review 策略变化会阻止旧绑定重新启动。现成 Released 结果可以在截止后只读补账。
- 人审期间退出会恢复原候选和审核交接。审核 pass 后可记录独立的
  `recovered_human_review` 结果，并保留原生 Session fence；它明确不是新的
  `TaskOutcome` 或 Released 记录，该 Session 不再启动。返工才在剩余预算内明确恢复。
- 未知工具效果仍需操作员提供对应原调用的实际结算依据，库不清 fence、换会话或
  自动确认旧 worker 已停止。超时、取消或断线不证明外部效果已撤销。

来源每份最多 8 MiB，读取窗口最多 16 KiB UTF-8；哈希基于该次完整有界快照。
这是明确的资源限制，不适用于直接读取任意大小日志；大来源应由业务 Host 注册分片。

## 验证入口

```text
cargo test --manifest-path tools/ara-lite-host/Cargo.toml --locked --workspace --all-targets
python tools/ara-lite-host/tests/reference_process.py --binary ABS_EXE --output NEW_DIRECTORY
```

进程脚本默认使用受控 HTTP/SSE provider，执行实际 Host、SQLite、Git 工作区和
原生 Session。可另加 `--live-endpoint HTTPS_URL --live-model MODEL --live-key-env ENV_NAME`
运行有界真实模型任务。保持新证据目录，不覆盖原失败或原生收据。
