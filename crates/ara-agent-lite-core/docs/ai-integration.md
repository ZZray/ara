# AI 接入 ara-agent-lite-core

适用于把当前精简 Agent 嵌入其他 Rust 业务进程。接口依据 `0.1.0` 当前源码，
完整示例见 [business_host.rs](../examples/business_host.rs)。接入前固定已审查的 Git 提交，
并核对 [验收记录](evidence/extraction-20261021.html) 的适用范围。

本轮已发布的运行库代码提交为 `0e4d81a91682196e9ef8be4a5ffae469ddd99bcc`，
Core 版本 `0.1.0`，对应 HandWave Node `0.2.9`。部署和新增 GNU Linux 检查见
[0.2.9 发布记录](../../../apps/handwave-node/docs/evidence/stage-publication-0.2.9-20261021.md)。
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
