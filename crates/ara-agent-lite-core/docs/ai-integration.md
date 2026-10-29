# AI 接入 ara-agent-lite-core

适用于把当前精简 Agent 嵌入其他 Rust 业务进程。接口依据 `0.1.1` 当前源码，
完整示例见 [business_host.rs](../examples/business_host.rs)。接入前固定已审查的 Git 提交，
并核对 [验收记录](evidence/extraction-20261021.html) 的适用范围。

本轮 Core 版本 `0.1.1`，对应 HandWave Node `0.2.10`。源码提交和部署证据见
[0.2.10 发布记录](../../../apps/handwave-node/docs/evidence/stage-publication-0.2.10-20261022.md)。
此前 `0.1.0` 的运行库提交为 `0e4d81a91682196e9ef8be4a5ffae469ddd99bcc`，其
[0.2.9 发布记录](../../../apps/handwave-node/docs/evidence/stage-publication-0.2.9-20261021.md)仅证明当时的版本。
GitHub 的 `ZZray/ara` 是本库所在仓库；部署 Ray 的 Go 宿主仓库不是本库入口。

## 1. 代码与业务边界

| 目录 | 用途 | 接入时的处理 |
| --- | --- | --- |
| `crates/ara-agent-lite-core` | 当前精简运行库，独立 workspace | 业务通过 Cargo 依赖使用 |
| `apps/handwave-node` | 已使用这个库的 HandWave 宿主，独立 workspace | 参考适配方式，保留其设备认证、云端连接、调度与界面 |
| 根 workspace 的 `crates/ara-agent`、`ara-ai` 等 | 仍在开发的完整版 ARA／OMP | 本次业务接入不修改、不引入 |
| `tools/ara-lite` | 仓库内另一程序 | 不作为本库入口 |

Core 执行 Agent 循环，保存原生 Session、工具收据和上下文，提供可选本机工具。
宿主负责用户／设备身份、业务授权、模型凭据、存储路径、事件展示、队列与定时调度。
成功 Run 是否完成业务 Task，仍由宿主验收。

同一 Session 的全部调用必须串行，跨进程也只允许一个写入者。宿主需加锁或串行调度；
不要把 Core 内部的 Mutex 当成跨请求或跨进程的 Session 锁。不同租户使用不同的
Session、环境状态与知识根目录。

## 2. 选择依赖与构建入口

本库有自己的 `[workspace]` 与 `Cargo.lock`。从业务项目使用路径依赖时，
不要把它加入完整版根 workspace，也不要执行仓库根目录的 Cargo 来代替本库验证。
业务的最终依赖解析以业务自己的 `Cargo.lock` 为准，不能假定依赖库的锁文件会传递给业务。

路径依赖示例，路径相对业务 `Cargo.toml`：

```toml
[dependencies]
ara-agent-lite-core = { path = "../ara-github/crates/ara-agent-lite-core" }
anyhow = "1"
async-trait = "0.1"
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }
tokio-util = { version = "0.7", features = ["rt"] }
```

远端仓库为 `https://github.com/ZZray/ara.git`，开发分支 `dev`。已验证的独立消费方式是
检出固定提交后使用路径依赖。当前未发布到 crates.io。

```shell
git clone https://github.com/ZZray/ara.git ara-agent-source
git -C ara-agent-source checkout --detach 0e4d81a91682196e9ef8be4a5ffae469ddd99bcc
```

直接 Git 依赖应固定 `rev`，包名明确写为 `ara-agent-lite-core`：

```toml
ara-agent-lite-core = { git = "https://github.com/ZZray/ara.git", rev = "0e4d81a91682196e9ef8be4a5ffae469ddd99bcc", package = "ara-agent-lite-core" }
```

直接 Git 依赖仍需在目标业务中验证；本轮执行记录来自路径依赖宿主。Cargo Git 依赖按包名
查找，不能靠仓库根目录或添加 `subdir` 字段选包。仓库包含多个 crate 和独立 workspace；
若 Cargo 在整仓扫描时报告同名包或 workspace 歧义，保留错误记录，改用固定提交检出加路径依赖，
不要通过改名或修改完整版清单来绕过。

运行业务的 `cargo metadata --format-version 1`，核对包名、版本、`source` 和
`manifest_path`，确认实际依赖的是本库。保留本库要求的 TLS 修复版本下界与固定的
`rpi-ai`／`rpi-agent` 提交；更新业务锁文件后再检查依赖图，不能使用旧锁文件强行降级。

## 3. 创建请求与消费事件

先由宿主验证当前用户能否执行这项业务，再调用
`RunRequest::new(id, objective, RuntimePaths::new(cwd, session_path, environment_state))`。

| 字段 | 宿主应提供的内容 |
| --- | --- |
| `id` | 唯一 Run ID；重试前先查看此 ID 的已有收据 |
| `objective` | 本次用户任务，不能把网页或工具输出提升为用户授权 |
| `cwd` | 已存在、可访问的工作目录 |
| `state_path`／`session_path` | 当前 Session 的固定原生状态文件；继续会话保持路径不变 |
| `environment_state` | 当前业务运行环境的独立状态目录 |
| `device_fp`、`session_key` | 宿主确认的稳定归属标识；使用外部快照或 Goal lease 时需一致 |
| `endpoint`、`token`、`route.model`、`route.protocol` | 已授权的模型端点、内存中的凭据和具体模型／协议 |
| `max_calls`、`max_seconds`、`max_output` | 宿主限定的预算；构造器默认值分别为 12、120、4096 |

`business_host` 示例另有自己的输入限制：`max_calls` 默认 8、范围 1..32，
`max_seconds` 默认 45、范围 1..900，`max_output` 默认 2048、范围 256..8192。
这些是示例 Host 的限制，不是 Core 构造器的默认值。经示例启动时，请求 40 次调用
实际只得到 32 次；调用方应核对原生预算观察，不能只按输入配置记录可用额度。

`RunRequest::new` 默认 `RunMode::Plan`、`enable_local_tools = false`，
没有 Worker、外部快照或业务工具。需要执行时由宿主设置 `RunMode::Execute`；
需要本机工具时另行设置 `enable_local_tools = true`。仅修改模式不会自动开启本机工具。

`execute(request, cancellation, events)` 运行期间必须同时消费有界事件通道。
示例中的顺序如下；完整错误处理和取消定时器使用 [现有示例](../examples/business_host.rs)：

```rust
let (events, mut receiver) = mpsc::channel(64);
let reader = tokio::spawn(async move {
    while let Some(event) = receiver.recv().await {
        println!("{event}");
    }
});
let result = execute(request, cancellation, events).await;
reader.await?;
let result = result?;
```

业务 UI 的取消操作调用对应的 `CancellationToken::cancel()`。不要先等待 `execute`
结束再读取通道，也不要靠丢弃执行 Future 表示外部命令已停止。
检查返回的 `Result` 和 `RunResult.code/error`，并结合原生工具收据判断结果。

取消、超时、断连或进程退出后，文件写入、网络请求及子进程效果可能已经发生。
遇到 pending 或 effects unknown，保留原 Session 和操作标识，先检查收据、文件与进程；
不要换 Session、Run ID 或 `op_id` 重放同一操作。继续正常会话还需保持 cwd 和持久化
`Route { model, protocol, binding, decision }` 一致。

### 3.1 调用预算停止后的原生证据

Execute 在最后一个原生工具轮次完成后耗尽调用预算时，原 Run 继续返回失败，
`RunResult` 的公开字段及 `code=-1` 不变。只有明确命中该停止原因、原生消息与工具
结果完整配对、循环已结束且持久化成功时，唯一的 `agent_end` 才可附带：

```json
{
  "termination": {
    "version": 1,
    "cause": "call_budget_exhausted",
    "native_round_closed": true,
    "model_final_stop": false
  }
}
```

公开类型位于 `ara_agent_lite_core::termination`。普通终态省略 `termination`；调用准入
被拒、长度截断、取消、超时、上下文或持久化故障不能单凭预算已满获得这份证明。
Goal、后台 Job 和历史让出继续使用原有流程。

`native_round_closed` 只说明原生协议与持久状态闭合。它可以与外部 Host 的未知副作用
收据同时存在；产物是否正确、外部操作是否结算仍须单独检查。直接使用 Core 的宿主
不得据此将 Run 或业务 Task 改成成功。需要产物复核与反馈续作时，可显式使用
[Harness Lite 的预算快照入口](../../ara-agent-lite-harness/README.md#预算停止的可选产物检查)，
由 Host 核查实际产物和操作收据。验证范围见
[预算停止交接记录](../../ara-agent-lite-jsonl/docs/evidence/closed-budget-snapshot-20261025.html)。

### 3.2 恢复被 fence 的 Task

进程在工具执行中被杀、Task 被取消或以 `ReconciliationRequired` 结束时，Harness 保留 fence。
恢复只能在原 Task、原 Session 中继续，由 Host 先结算留下的开放调用。帧格式与字段表见
[JSONL v1 协议](../../ara-agent-lite-jsonl/docs/protocol-v1.md)“恢复被 fence 的 Task”一节，
Host 义务见 [宿主边界](knowledge/host-contracts.md)“resume 结算义务”一节。

按顺序执行：

1. 确认 Task 处于 fence 状态：Session 所在目录有 `.ara-lite-harness-*.fence.json`
   （以 `ReconciliationRequired` 结束、`task_result.fence_presence` 为 `Present` 的 Task 也属于此类）。
   读取 fence 文件字节，计算 SHA-256。
2. 启动一个新的 JSONL sidecar 进程，发送带 `resume` 的 start 帧。`task` 与原 Task 相同，
   只把 `evidence_dir` 换成新的、不存在的目录；`run` 只改 `run_id`、`objective`，
   `max_calls`、`max_seconds` 可以等于或小于原值。`resume.fence_sha256` 填第 1 步的哈希。
3. 读取 `reconcile_request`。对 `review.interrupted_run.open_tool_calls` 中的每个调用，
   依据 Host 自己的操作回执判断它是否执行、结果是什么，按原顺序构造 `reconcile_reply`，
   原样复制 `version`、`rpc_id`、`task_id`、`run_id`。
4. 在 Task 的 `deadline_unix_ms` 之前把 `reconcile_reply` 写入 stdin。
5. 读取最终 `task_result` 和它的 `outcome_ref` 指向的 `TaskOutcome`。`task_accepted` 恒为 false，
   Task 是否验收由 Host 自己决定。

Python 客户端通过 `reconcile_resume(review, cancelled)`、Node 客户端通过
`callbacks.reconcileResume(review, {signal, frame})` 完成第 3、4 步。

被拒绝时，下列各项的共同状态是：fence 文件与原有回执不变，上游 HTTP 请求数为零，
Session 中的 pending_run 和开放调用保留，sidecar 以退出码 2 结束。
前四项在 Harness 调用 Host 之前拒绝，stdout 零帧，不创建新证据目录；
其余各项已发出 `reconcile_request`，新证据目录写 `jsonl-resume-rejected.json`，类别见表。

| 拒绝原因 | 触发条件 | 拒绝后的状态 | 集成方的下一步 |
|---|---|---|---|
| resume 字段格式无效 | `resume` 为 null、不是对象、缺少或多出子字段、`version` 不为 1、`fence_sha256` 不是 64 位小写十六进制 | stderr `start_config_rejected`，零帧 | 修正字段，用新的 sidecar 和新证据目录重试 |
| Task 条款或 Session 不一致 | `task_id`、`criteria_revision`、`authorization_revision`、`deadline_unix_ms` 或 `session_path` 与 fence 记录不同；fence 哈希与 `fence_sha256` 不同也在此拒绝 | stderr `task_admission_rejected`，零帧 | 按 Host 保存的原 Task 记录和刚读取的 fence 重新填写 |
| authority 哈希改变 | `run` 中除 `run_id`、`objective`、`max_calls`、`max_seconds` 外的字段改变，如 endpoint、model、tools、cwd | 同上 | 恢复原 `run` 配置；授权确实已变时不能恢复这个 Task，由业务另行处理其开放调用 |
| 预算或单 Run 上限增大 | `task.max_runs`、`task.max_calls` 与原值不同，或 `run.max_calls`、`run.max_seconds` 大于原 Task 首次准入的值 | 同上 | 使用原预算，单 Run 上限不超过原值 |
| reconcile_reply 缺失或 stdin 已关闭 | 回复之前 stdin 关闭 | 一帧 `reconcile_request`；类别 `stdin_closed` | 新的 sidecar 与新证据目录，这次在 deadline 前写入回复 |
| 超过 Task deadline | deadline 到达时仍未回复：sidecar 先发 `callback_cancel`，等 `cancel_grace_ms` 后拒绝；start 时 deadline 已过则在调用 Host 前拒绝、零帧 | 类别 `reconcile_timeout` | deadline 属于原 Task 条款，不能延长；这个 Task 不能再恢复，由 Host 按自己的回执在业务侧结算 |
| 请求 ID 不匹配 | 回复的 `rpc_id`、`task_id`、`run_id` 或 `version` 与待处理请求不同，或回复是另一种帧 | 回复不交给 Harness；类别 `rpc_mismatch` | 原样复制请求的关联字段后，用新的 sidecar 重试 |
| reconcile_reply 字段无效 | 不是合法单行 JSON、`kind` 未知、`receipt` 缺少或多出字段、字段类型不符 | 回复不交给 Harness；类别 `invalid_reply_fields` | 按字段表修正后重试 |
| committed 为 false | `receipt.committed` 为 false | 回复已写盘，Harness 拒绝；类别 `receipt_uncommitted` | 先把结算结果保存到 Host 自己的记录，再重试 |
| 结算为 Unknown | `receipt.settlement` 为 `Unknown` | 类别 `settlement_unknown` | 继续用 Host 的收据和外部系统状态核查，直到能确定每个调用的结果 |
| 证据为空 | `receipt.evidence` 为空数组 | 类别 `evidence_empty` | 填入 Host 自己的操作回执引用 |
| 回执与开放调用不对应 | `receipt.tool_calls` 的数量、顺序或 `call_id` 与 `open_tool_calls` 不同 | 类别 `call_mismatch` | 按 `open_tool_calls` 的顺序为每个调用恰写一条结算 |

回复前收到 Host 的 `cancel` 时类别为 `resume_cancelled`，状态同上。

被拒绝的 resume 不会转为新对话：sidecar 不新建 Task、不新建 Session，也不调用模型。
它也不会重放任何效果未知的调用；开放调用只能由 Host 的结算关闭。
重试必须用新的 sidecar 进程和新的证据目录，fence 和原证据目录不要删除或改写。

## 4. 模型适配

默认 `CatalogModelHost` 读取 `endpoint + /models`，校验具体模型、协议和声明能力。
公网接口需要有效 HTTPS；明文开发端点只接受字面量环回 IP。端点不提供兼容目录时，
由宿主实现 `model::ModelHost`：

```rust
async fn resolve(
    &self,
    route: Route,
    endpoint: &str,
    token: &str,
    run: &str,
) -> anyhow::Result<ModelSelection>;
```

这是当前 trait 方法签名的摘录，完整定义见 [model.rs](../src/model.rs)。
返回 `ModelSelection { route, client, capabilities, headers }`，类型分别为 `Route`、
`reqwest::Client`、`model_caps::ModelCaps` 和 `BTreeMap<String, String>`。
宿主负责校验模型选择、凭据和 headers 的授权，避免重定向或自定义 client 泄露凭据。
Core 在原有 Run 准入位置调用 resolver，仍检查协议、容量预算和旧 Session 路由连续性。

当前执行协议为 `openai-completions`／`openrouter-chat`，输入为文本。
目录声明 1M 上下文或图像能力，不代表当前请求会传输图像，也不代表会把整个上下文窗口填满。
容量预算采用保守字节估算；未知 token 用量仍为未知。不要把 provider 缓存或省 token 写成既定效果。

## 5. 注册业务工具与开启本机工具

实现通过本库重导出的 `rpi_agent::AgentTool`，使用重导出的 `rpi_ai::{Tool, Schema}`
定义 schema。工具返回 `AgentToolResult` 或 `AgentError`，在执行前检查取消信号和业务授权，
按 schema 验证参数。JSON schema 不能代替执行端校验。

业务工具通过 `request.extra_tools` 注册。示例的注册代码见
[business_host.rs](../examples/business_host.rs) 中的 `BusinessLookup` 和 `ToolRegistration`：

```rust
ToolRegistration {
    tool: Arc::new(BusinessLookup { schema }),
    allow_in_plan: true,
}
```

以上是字段用法摘录。接入代码应复用完整示例中的 trait 实现，并把示例常量结果替换成
经过授权的业务查询。`allow_in_plan = true` 表示宿主明确允许该工具在 Plan 使用，
适合已确认无修改副作用的查询；它不会把实现变成 OS 只读执行。
内置工具名即使关闭也保留。空名、非法名、重复名及覆盖内置名称均在 Run 入场前拒绝。

`enable_local_tools = true` 会同时启用文件、Shell、脚本、搜索、系统信息、联网、
知识库与 Skills，以及符合单独授权条件的作业工具。这个开关授予的能力较广，
只需要业务 API 工具的服务应保持关闭。
本机工具使用宿主 OS 账户权限，允许绝对路径，cwd 不构成文件系统隔离。
多租户服务需要的账户、容器或其他隔离措施由宿主部署负责。

### 5.1 版本化读取与编辑（0.1.1）

本机工具开启且运行模式允许修改时，建议按下面顺序编辑已有 UTF-8 文件。
这些是 Agent 工具参数，不是新增的 Rust 公共方法；业务可让模型调用工具，
或在自有业务工具中实现自己的编辑策略。

```json
{"path":"src/example.py","snapshot":true}
```

`read_file` 返回完整 `content`、精确原始字节的 `sha256`、`bytes` 和
`truncated:false`。文件上限 1 MiB；`snapshot:true` 不与 `offset/limit/tail` 混用。
不加 `snapshot` 的旧请求仍返回原始文本。BOM、CRLF 和 Unicode 不做规范化。

读取后将返回的哈希传给 `edit_file`，可以先预览，再以相同参数提交：

```json
{
  "path":"src/example.py",
  "old_text":"old_first",
  "new_text":"new_first",
  "expected_sha256":"<read_file 返回的 64 位 SHA256>",
  "additional_edits":[{"old_text":"old_second","new_text":"new_second"}],
  "dry_run":true
}
```

正式提交将 `dry_run` 改为 `false`。所有块在同一份原始文本中匹配，必须唯一且互不重叠；
最多 64 块，old/new 参数合计不超过 2 MiB，结果文件不超过 1 MiB。
哈希不符、任一块不匹配、重叠、参数错误或提交前取消，均不执行此次写入。

增强请求返回 JSON 收据：`before_sha256`、`candidate_sha256`、`committed`、
`observed_after_sha256` 及有界变更预览。预览的 `committed:false`，候选哈希不代表已落盘；
`no_change` 也不声称发生提交。旧请求不包含任何新增编辑字段时，返回原有文本格式。
可选字段若传 `null` 会报错，不能用它代替省略。

写入保持同一文件句柄，以保留 inode、ACL 和硬链接语义；这不是文件系统 CAS 或原子事务。
外部写入者、路径替换仍可能竞争。I/O 或提交后读回失败返回 `effects_unknown`，
文件可能已部分改变，必须先读取新快照核对，不能自动重放。
提交成功也只证明文件编辑；宿主仍需运行项目检查并验收业务结果。

受控实际 Host 验收入口为
[verify_versioned_edits.py](../tests/verify_versioned_edits.py)，真实模型编码基线为
[verify_coding_live.py](../tests/verify_coding_live.py)。前者验证工具行为，后者使用
独立隐藏样本检查修复结果；模型表现和 token 节省不能由受控 provider 推断。

### 5.2 可选进程输出归档

本轮工作区为 `exec` 和 `run_script` 增加可选 `capture_dir`，尚未发布新安装包。
省略该字段保留原返回格式。对于长编译、测试或分析输出，可以保留原件，再按需回读：

```json
{"language":"python","script":"print('diagnostic')","output_limit":1024,"capture_dir":"build-output-001"}
```

相对路径基于本次工具的 `cwd`，也接受绝对路径。字段不能为 `null`、空白或超过
4096 UTF-8 字节；父目录必须存在，最终目录必须全新，准备失败不启动进程。
不会覆盖、复用或自动删除目录。调用者负责原始日志的访问、留存和清理。

目录内的 `stdout.log`、`stderr.log` 保存精确原字节，各最多 8 MiB。
超过配额继续排空管道，收据分别给出 `observed_bytes`、`stored_bytes`、
`quota_exceeded`、`eof`、`complete`、`sha256` 和错误。SHA256 对应收据生成时
已保存并读回核验的字节，不能证明之后文件不变或断电持久化。
配额不足保持进程原退出状态；归档 I/O 错误则返回工具错误，同时保留已知进程状态和部分日志。

模型摘要与 Host 工具事件都会保留 `[process output capture]` 后的 JSON 收据。
进程是否完成、退出码和错误在 `process` 字段中；流的 `complete:true` 只描述输出捕获。
非零退出、超时和取消仍不代表任务成功，已有副作用不能靠归档撤销或自动重放。
`run_script` 取消保持原 `Abort` 合同，文本收据可能不再进入模型，但原文件仍在已知目录。

默认 `read_file` 最多读取 1 MiB。更大的 UTF-8 日志用 `offset/limit/tail` 分段读取；
非 UTF-8 内容需用 `run_script` 显式解码。同步磁盘 I/O 没有独立硬截止保证。
实际 Host 验证器为 [verify_process_capture.py](../tests/verify_process_capture.py)，
覆盖长日志中段回读、修复后执行、非零退出、配额、前置拒绝和超时。
受控 provider 验证工具机制，不证明模型会自主选择归档，也不证明一般逆向能力提升。

### 5.3 可选源码符号与静态二进制分析

工作区新增 `analysis_tools`；宿主按需注册，默认工具集不变，尚未发布。
宿主先准备专属的现存归档目录，再提供明确的后端可执行文件：

```rust
request.extra_tools.extend(analysis_tools::register(analysis_tools::AnalysisConfig {
    cwd: project_dir,
    artifact_root: analysis_artifacts,
    ctags: Some(ctags_executable),
    objdump: Some(objdump_executable),
    nm: Some(nm_executable),
}).await?);
```

路径使用 `PathBuf`。后端可分别省略，但不能全部为空。注册时查询实际版本，
要求 Universal Ctags 支持 JSON，或使用 GNU objdump/nm；执行前后核对后端 SHA256。
Windows 的部分 GNU 工具不能处理 `\\?\` 工作目录。宿主应提供后端可访问的
绝对 `artifact_root`；库保留这一路径表示，并核对其规范路径身份，不自动改用其他目录。

`source_symbols` 接受单个文件的 `path`，可选 `language`。常见扩展名可推断语言；
其他扩展名需明确指定后端支持的语言。结果包含定义的名称、种类和行号，
`end/scope/signature` 依解析器而定。它不提供引用图、调用图或 LSP 重构。

`binary_inspect` 接受 `path` 和固定 `action`：`headers`、`symbols`、`disassemble`。
反汇编还需十六进制字符串 `start_vma/stop_vma`，范围递增且跨度不超过 65536 字节。
VMA 是链接地址，不能当作文件偏移或运行时 ASLR 地址。结果保留后端文本；
无符号是可能的正常结果，不据此推断程序没有功能。工具不会执行被分析文件。

两个工具先读取有上限的输入并写入新快照，再从快照目录使用固定 ASCII 文件名分析。
源码上限 4 MiB，二进制上限 64 MiB；单次后端执行限时 10 秒，随后沿既有流程收尾进程树。
原始 stdout/stderr 每流最多 8 MiB；解析最多 20000 条记录、单条 64 KiB，最终 JSON 最多 16 KiB。
注册探测、同步文件 I/O 和进程收尾不由这 10 秒形成统一硬截止。

`offset/limit` 分页，默认每页源码 30 条、二进制文本 40 行，最多 100 条。
后续页同时提供原结果的 `input.sha256` 为 `expected_sha256`，以及
`page.page_token` 为 `expected_page_token`。指纹绑定输入、原路径、后端和查询范围；
每页重新分析，不表示降低了后端总计算量。单条记录无法装入结果预算时明确失败，
不会返回游标不前进的空页。

`status`、`analysis.complete`、`analysis.backend_complete` 与 `page.complete`
分别描述工具结果、解析覆盖、后端输出捕获和分页覆盖；`partial` 不能当作完整分析。
结果包含原路径、输入和后端哈希、快照及双流收据。前后哈希可发现持久变化，
不提供文件系统事务或对同账号恶意并发替换的隔离保证。

注册项明确允许 Plan 查询：这会运行本机解析器并写宿主归档目录。
工具许可不等于操作系统沙箱；原始源码、样本和日志的权限、留存及清理由宿主管理。
受控 Host 合同、真实模型任务和总体能力增益分别验收，见
[增强进展](evidence/coding-reverse-enhancement-20261022.html)。

### 5.4 宿主显式生成策略

`model::ModelHost::generation_policy(&Route)` 是同步的纯配置入口，默认返回
`GenerationPolicy::ProviderDefault`，保留原请求行为。Core 在解析实际路由后调用它；
实现不能做 I/O，也不能依赖并发 Run 共享的“上次解析模型”状态。

宿主确认服务支持相应协议后，可返回 `GenerationPolicy::ThinkingTypeDisabled`。
该策略仅接受 `openai-completions` 路由，在主任务请求根部发送
`"thinking":{"type":"disabled"}`。其他协议在模型或工具调用前拒绝；
接口不接受任意 `extra_body` 或 sampling 参数覆盖。

`business_host` 示例支持可选 JSON 字段
`"generation_policy":"thinking_type_disabled"`；省略或填写 `provider_default`
均维持默认行为。非默认策略发出 `generation_policy` 事件，记录实际路由、请求与
有效输出预算，并明确 `provider_effect_verified:false`。事件证明本地配置，
服务端是否执行仍需核对实际请求、响应和任务结果。

示例也接受可选字符串 `brief`，原样交给既有 `RunRequest.brief`。
宿主预取的静态事实可由此传入，但必须保留来源与覆盖状态；这不计为模型自主取证。

输出预算继续取宿主请求、目录解析上限与本地 8192 上限的最小值。
目录缺少容量时采用本地回退值，不能把 8192 写成服务端声明的能力。
上下文摘要继续克隆已选模型并应用既有摘要配置，输出预算最多 4096；
诊断时应按 `model_call.phase` 区分主任务与摘要，分别核对实际请求。
启用 thinking 的工具历史回放协议未由这一改动补齐；纯思考耗尽预算仍按原规则失败。

8 项受控原生场景已核验默认请求、显式字段、预算裁剪、两轮工具和失败边界。
这只验收生成策略的请求契约；真实逆向任务和能力收益另行验证。

### 5.5 读取与分析工具的可选 null 参数

当前工作区的 `read_file`、`source_symbols` 和 `binary_inspect` 在 schema 中
为可选参数增加 `null` 类型，表示未使用该参数。例如：

```json
{"path":"README.md","snapshot":true,"offset":null,"limit":null,"tail":null}
```

这个请求返回完整版本化快照。只移除当前工具白名单中的 optional null，
之后执行原有参数解析；`path/action` 没有 null 分支，未知字段也不会被这一处理移除。
必填参数的实际失败阶段可能受固定 rpi 的类型转换影响，应核对原生错误回执。

快照与非 null 范围参数冲突、非反汇编动作携带非 null VMA、错误指纹、
后续页缺少绑定参数等情况仍被拒绝。反汇编的 VMA 若为 null，归为未提供，
仍因缺少必要范围失败。源码语言为 null 时采用原有扩展名推断；未知扩展名仍需明确语言。
编辑、下载及其他工具的参数规则不随这一改动扩展。

真实模型曾填入互斥的所有可选参数，且原始服务响应就包含这些值。
本地 adapter 没有补参，但上游转换与模型自行选参尚未区分。
本点提供一种明确的未使用参数表示，不能据此认定已修复远端转换或提升模型能力。
当前受控原生链路与原回归已有执行记录，真实任务另行验收，见
[增强进展](evidence/coding-reverse-enhancement-20261022.html)。

### 5.6 原始来源检索与分窗读取

`reference_search` 在当前 Session 的 active 来源中查找字面量，每个来源返回第一个
命中，最多返回 20 个来源。结果包含 `id`、`version`、整篇 `hash`、总 UTF-8
`bytes`、`match_offset`、`match_bytes` 和命中附近的 `excerpt`。偏移按原文 UTF-8
字节计数。摘录通常最多 512 字节；较长检索词保留完整命中，摘录最多 1120 字节。
摘录的实际区间为 `excerpt_offset..excerpt_next_offset`。

`reference_read` 必填 `id`，可选 `offset` 和 `limit`。默认从 0 开始返回最多
8192 字节，单次最多 16384 字节。可把检索结果的 `match_offset` 传入，直接读取
有关段落。小来源在默认范围内仍完整返回；较大来源的默认响应改为一个窗口，
依赖旧版整篇返回的消费者需要处理分页。

```json
{"id":"ref-2","offset":24576,"limit":4096}
```

返回值保留来源的原有元数据，`text` 为本次窗口。`offset` 与 `next_offset`
表示实际返回区间，`bytes` 为整篇长度，`hash_scope` 固定为
`full_source_text_utf8`。`hash` 始终对应完整原文，不能拿窗口文本重新计算后直接比较。
任何前缀或后缀未返回时，`truncated` 都为 true，包括尾页和非空来源的 EOF 空窗。
全文返回需满足 `offset == 0 && next_offset == bytes`；继续读取需满足
`next_offset < bytes`，不能只看 `truncated`。

偏移必须位于原文的 UTF-8 字符边界内，允许等于整篇长度。窗口末尾向前收齐到
字符边界，不超出长度上限。如果窗口装不下下一个 Unicode 字符，工具明确报错。
非法范围、已被更正或撤销的来源 ID 均被拒绝。读取不会修改存储的正文、哈希或采纳记忆。

窗口上限约束原文的字节数；JSON 转义和累积工具历史仍可能超过上下文预算。
宿主和模型应只读取当前任务需要的段落，Core 的上下文超限检查继续生效。

## 6. 知识库、Skills 与外部记忆

启用本机工具后，默认目录解析由 [environment_library.rs](../src/environment_library.rs)
负责：运行环境状态下的 `library/knowledge` 与 `library/skills`，以及最近项目根下的
`docs/knowledge` 和 `.handwave/skills`。知识采用索引加内容，读取时按需加载。
本机工具关闭时，不初始化这些目录，也不读取其目录快照。

需要独立业务布局时设置 `request.knowledge_roots = Some(environment_library::Roots { ... })`。
实际字段为 `environment_dir`、`global_knowledge`、`project_knowledge`、`global_skills`、
`project_skills`，均由宿主给出明确路径。`request.global_knowledge` 只覆盖默认全局知识路径，
不能代替整套 Roots 配置。区分设备独立知识和明确共享的项目／个人知识，避免不同用户意外共用目录。

外部云端记忆由宿主读取并鉴权，通过
`request.context_snapshot: Option<context_memory::CloudSnapshot>` 注入。
快照包含 `version/status/device_id/epoch/records`；Core 检查与 `device_fp` 的身份关系、
scope、revision、内容哈希及大小，仍将其视为来源数据。
当前 scope 为 `device/project/workspace`；不要自行增加 `personal` 枚举，宿主需在已有范围中
明确其映射与访问权限。快照不授予工具权限，也不自动采纳记忆。

普通 `brief` 不会被 Core 识别为 HandWave 云端载体。需要兼容旧载体时由宿主解析；
不要修改 Session、Journal、索引标记或 `.handwave` 兼容文件名来进行接入。

## 7. 后台进程与终端入口

需要 Worker 时使用同步 `main`，首先执行下列现有入口，再解析 CLI、加锁、启动业务服务或 Tokio runtime：

```rust
fn main() -> anyhow::Result<()> {
    if let Some(result) = ara_agent_lite_core::managed_jobs::worker_entry_from_args() {
        return result;
    }
    // 此后进入业务配置与运行时。
    Ok(())
}
```

确认目标 EXE 实现这个入口后，宿主才设置
`request.worker_executable = Some(std::env::current_exe()?)`，或提供另一个兼容程序的绝对路径。
仅设置路径不会授予后台权限。通过 Agent 循环使用作业时，还需本机工具开关、对应运行模式及
宿主授权的 Goal lease；lease 的身份、期限、预算和 `allow_jobs` 不能由模型自行生成或续期。

没有 Worker 配置时不提供启动或恢复修改工具，`Manager::start` 也会在写启动意图前拒绝。
同一 `op_id` 读取已有作业收据；有未知结果时先核对原收据。
终端输入通过 `input_id` 去重；凭据输入使用宿主专用私密通道，不能把验证码或密码放入聊天工具参数。

示例的 `job_smoke`／`pty_smoke` 会创建文件和实际 Python 子进程，仅在临时验收目录运行。
终端模式是 PTY／ConPTY，合并终端输出，不提供桌面 computer use。

## 8. 接入验收

先在本库已检出的目录构建独立示例，再接业务宿主。以下命令从仓库根目录执行：

```powershell
cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline
cargo build --manifest-path crates/ara-agent-lite-core/Cargo.toml --example business_host --locked --offline
python crates/ara-agent-lite-core/tests/verify_embedding.py --binary PATH_TO_BUSINESS_HOST --output FRESH_EVIDENCE_DIRECTORY
```

`PATH_TO_BUSINESS_HOST` 填刚构建的示例可执行文件，`FRESH_EVIDENCE_DIRECTORY` 必须是新的临时目录。
`--offline` 适用于已有依赖缓存的机器；新环境先按本库锁文件取得固定依赖，不能为了编译随意升级上游。
受控验收器使用本地确定性 provider，不调用真实模型。业务宿主还需运行自己的端到端用例：

- 查询一次业务工具，核对真实返回值、事件和原生收据；关闭本机工具时模型不能调用本机写工具。
- 新进程继续同一 Session，保留旧消息；重复 Run ID 不重复工具副作用，切换路由被明确拒绝。
- 取消、模型失败、事件接收端关闭后能结束，错误不标成功；未知副作用保留原收据待查。
- 开启 Worker 的业务验证实际启动、输入输出、去重和进程退出；未配置 Worker 时无启动意图。
- 知识目录隔离有效，外部快照跨身份、错误哈希和超限输入被拒绝。

编译通过、受控 provider 通过、真实模型试验与生产验收分别记录。
当前 Windows 有实际执行记录。发布阶段还验证了 GNU Linux x86_64 的原生 debug 构建、
28 项知识／技能／预算测试和 5 组实际 Host 用例；未验证 Linux release/LTO、其他架构或 macOS。
“新会话自主读取已沉淀知识正文”仍为 FAIL／OPEN；通用自我进化与可量化 token／费用节省也未验收。
后续验收不得直接继承 HandWave 或完整版 Core 的结论。

## 9. 给其他 AI 的任务模板

复制下面内容，填完字段后交给负责业务接入的 AI：

```text
请把 ara-agent-lite-core 接入以下业务，只做最小集成。

业务仓库／分支：<路径、分支、现有 dirty 范围>
Core 固定版本：<已审查的完整 Git SHA、检出路径>
目标系统与架构：<Windows/Linux/macOS、架构>
接入位置：<已有请求入口、会话服务、事件展示入口>
模型：<endpoint、具体 model ID、protocol、ModelHost 方案>
凭据来源：<环境变量或宿主密钥服务；不要贴凭据>
身份与授权：<用户/设备/session_key 来源、允许执行的业务范围>
状态路径：<cwd、Session 路径映射、environment_state、是否独占串行写>
业务工具：<名称、schema、实际业务实现、Plan 许可、幂等操作标识>
本机工具：<保持关闭，或明确授权开启的原因与OS账户范围>
知识与记忆：<Roots、设备/共享范围、typed snapshot来源和鉴权>
后台进程：<不启用，或兼容Worker路径、main-first入口、Goal lease来源>
预算与取消：<max_calls/max_seconds/max_output、取消入口>
验收：<受控工具与重启/去重/错误/隔离用例、证据目录>

先读 Core docs/ai-integration.md、公共接口和 business_host 示例。
保持完整版 ARA/OMP、HandWave 认证/云端/UI 及无关 WIP 不动。
同Session串行写，事件与execute并行消费；默认Plan且不开本机工具。
保留未知副作用和原生收据，不换ID重放。只使用当前API，不假定图像或其他provider已支持。
先列实际修改范围，再实现、验证并独立审查；报告未覆盖项，不自行扩大部署范围。
```
