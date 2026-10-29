# ara-agent-lite-core

供业务进程嵌入的 Rust Agent 运行库，版本 `0.1.1`。代码从 HandWave
精简被控端提取，HandWave 已改用这个库。完整版 ARA／OMP Core 继续在原目录开发，
两者不互相依赖，也不共用验收结论。

其他业务或 AI 接入前，先读 [AI 接入说明](docs/ai-integration.md)，核对依赖、宿主授权和验收范围。

## 范围

库包含 Agent 循环、工具调用、取消与调用预算、原生会话日志与恢复、上下文压缩、
参考与记忆、目标进度，以及可选的本地文件、脚本、搜索、联网、知识库、Skills、
后台进程和 PTY／ConPTY 工具。

业务宿主负责用户与设备身份、授权、模型凭据、状态路径、事件展示、消息调度，
以及成功 Run 的任务验收。库不会自动连接 HandWave、注册设备、申请授权或安排定时任务。
当前模型调用支持 OpenAI-compatible completions／OpenRouter chat；
不包含完整版 Core 的全部协议和 OMP 能力。模型声明的图像能力不等于本库已有图像传输。

## 输出长度恢复

Host 可通过 `ModelHost::length_recovery_policy` 显式选择
`model::LengthRecoveryPolicy::ReplanOnce`。默认值是 `Stop`，既有消费者保持原行为。
该策略属于 Host 执行配置；它不表示模型目录支持某项能力，也不修改生成参数。
显式启用只支持 Execute，Plan / Goal 会拒绝该配置。

Core 仅在当前 Run 的完整 `Length` 消息不含工具调用、原工具轮次闭合、没有检测到
未知效果且仍有原调用和时间预算时，最多安排一次重规划。它沿用原循环、Run ID、
Session、模型和授权。原截断消息保留，不拼接正文、工具参数或私有 thinking。
带工具调用的截断消息仍由固定 rpi 的原有保护整批拒绝执行。

自动控制引用先保存到 context，标明原消息索引、typed SHA 和非人类来源，再交给原
循环。`length_recovery` 事件的 `scheduled` 只表示已安排，下一请求仍由原预算入口
准入。取消、截止、未知效果或保存失败会阻止继续。Host 仍负责实际外部效果结算。
再次出现无工具调用的 `Length` 仍失败；该策略不保证任意模型一定完成任务。

JSONL 产品可设置 `run.length_recovery = "replan_once"`；省略或 `"stop"` 保持默认。
每个 Run 最多一次，Harness 的多个 Run 继续共用原 Task 调用预算与绝对截止。
当前受控回归及真实模型结果见
[长度恢复验证](docs/evidence/length-recovery-20261025.html)。

## Cargo 依赖

这是独立 workspace，可通过路径依赖接入。不要把业务代码复制进库。

```toml
[dependencies]
ara-agent-lite-core = { path = "../ara-github/crates/ara-agent-lite-core" }
anyhow = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }
tokio-util = { version = "0.7", features = ["rt"] }
```

发布前应固定业务仓库使用的版本或 Git 提交。当前提取结果尚未发布到 crates.io。
上游 `rpi-ai`／`rpi-agent` 仍固定于
`8ec462672d90e62712d96074638efef89436cebe`；原版权与许可文本保留在
[第三方声明](THIRD-PARTY-NOTICES.md) 和 `licenses/`。

## 调用一次 Run

宿主应先完成授权检查，再创建请求。三个路径分别控制工作目录、原生会话文件、
环境知识与技能状态。不同用户或业务实例应使用各自的状态目录。

```rust
use ara_agent_lite_core::{execute, RunRequest, RuntimePaths};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut request = RunRequest::new(
        "business-run-001",
        "说明这个业务请求的处理步骤。",
        RuntimePaths::new("./work", "./state/session.json", "./state/environment"),
    );
    request.endpoint = std::env::var("MODEL_BASE_URL")?; // 例如 https://host/v1
    request.token = std::env::var("MODEL_API_KEY")?;
    request.route.model = std::env::var("MODEL_ID")?;
    request.route.protocol = "openai-completions".into();
    request.max_calls = 8;
    request.max_seconds = 60;
    // 默认 Plan，不注册本机工具、不启动 Worker、不初始化本地知识库。

    let (events, mut receiver) = mpsc::channel(64);
    let reader = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            println!("{event}");
        }
    });
    let result = execute(request, CancellationToken::new(), events).await;
    reader.await?;
    let result = result?;
    println!("code={} {}", result.code, result.text);
    Ok(())
}
```

调用前创建 `work` 和 `state` 目录。事件通道必须在 Run 期间持续消费；
先等待 `execute` 再读有界通道会造成背压。取消、连接关闭及超时不能证明外部副作用已撤销，
后续处理应先查原生收据，不自动重放操作。

## 宿主配置

| 配置 | 行为 |
| --- | --- |
| `RunRequest::new` | 默认 Plan、12 次调用、120 秒、4096 输出上限；模型地址和凭据由宿主提供 |
| `enable_local_tools = true` | 启用文件、Shell、脚本、联网、知识库与 Skills 工具；Plan 仍禁止写操作 |
| `mode = RunMode::Execute` | 宿主明确允许执行；本机工具还需单独开启 |
| `extra_tools` | 注入业务 `AgentTool`；内置名称和重复名称会在 Run 入场前被拒绝 |
| `ToolRegistration::allow_in_plan` | 宿主允许该业务工具在 Plan 使用；这是声明，不是 OS 只读沙箱 |
| `model_host` | 注入异步 `ModelHost`，提供选定 route、HTTP client、模型能力和 headers |
| `knowledge_roots` | 指定知识／技能根目录；不指定时沿用原有目录解析规则 |
| `context_snapshot` | 注入结构化外部记忆；身份、范围、哈希和大小仍会校验，不视为授权或已采纳记忆 |
| `worker_executable` | 指定实现 Worker 入口的绝对 EXE 路径；默认 `None`，禁止后台启动 |

默认 `CatalogModelHost` 读取显式 endpoint 的 `/models`，检查所选模型、协议和能力。
只有字面量环回地址允许明文 HTTP；公网模型接口需要有效 HTTPS。
HandWave 的 grant、binding headers 和设备认证保留在它自己的宿主中。

本地执行使用业务进程的 OS 账户权限。工具清单、Plan 和路径检查均不提供 OS 沙箱。
服务端业务若需要限制文件与进程权限，应在宿主／部署环境另行实施。

## 后台进程与终端

如需后台进程，宿主 `main` 必须在解析业务 CLI、加锁或启动服务之前处理 Worker：

```rust
fn main() -> anyhow::Result<()> {
    if let Some(result) = ara_agent_lite_core::managed_jobs::worker_entry_from_args() {
        return result;
    }
    // 之后再启动业务入口。
    Ok(())
}
```

确认入口存在后，宿主才可设置 `worker_executable = Some(std::env::current_exe()?)`。
只有路径还不够：目标程序必须实现相同 Worker 入口；否则任务可能失败。
库不会推断当前 EXE 支持 Worker。Goal lease、归属、期限和作业配额仍由宿主明确提供。
相同 `op_id` 读取已有收据，不重新启动任务；未知执行结果需先检查。

## 兼容与示例

Execute 在调用预算耗尽时仍保留原 Run 失败。满足严格原生工具轮次闭合与持久化条件时，
唯一的 `agent_end` 可附带版本化 `termination`；它不证明外部副作用已结算或业务 Task
完成。公开 `RunResult` 字段保持不变。需要检查最后一次工具操作的实际产物时，宿主可
选择上层 Harness Lite 的独立预算快照回调，详见
[终态接入说明](docs/ai-integration.md#31-调用预算停止后的原生证据)。

Session、Journal、Goal、知识索引和 Skill 的既有磁盘格式保持不变。
`.handwave/skills`、`handwave-knowledge-v1` 等名称是保留的格式标识，
不代表新业务会接入 HandWave。环境知识默认位于宿主给定状态目录，
项目知识默认位于最近项目根的 `docs/knowledge/`；需要隔离时设置 `knowledge_roots`。

[business_host 示例](examples/business_host.rs) 演示无 HandWave 的独立消费程序、
业务工具、并行事件消费和 Worker 入口。通过 JSON 文件配置路径及模型，凭据只从指定环境变量读取。
它还提供本地 `job_smoke`／`pty_smoke` 动作，供检查实际进程、重复操作和终端输入输出；
这些动作会在指定临时目录创建文件，仅用于受控验收。

```powershell
cargo test --manifest-path crates/ara-agent-lite-core/Cargo.toml --locked --offline
cargo build --manifest-path crates/ara-agent-lite-core/Cargo.toml --example business_host --locked --offline
python crates/ara-agent-lite-core/tests/verify_embedding.py --binary PATH_TO_BUSINESS_HOST --output FRESH_EVIDENCE_DIRECTORY
```

当前执行结果与未覆盖项见仓库交付记录。既有“新会话自主读取已沉淀知识正文”验收仍未通过，
不会因为改为共享库而自动变成已完成；本库也不宣称通用自我进化已经验收。

0.1.1 增加精确 SHA256 快照、多块原文编辑和只读预览，参数与兼容规则见
[AI 接入说明](docs/ai-integration.md#51-版本化读取与编辑011)。
[发布验证与部署范围](../../apps/handwave-node/docs/evidence/stage-publication-0.2.10-20261022.md)
另行记录；此前提取报告保留原时点的验收边界。
