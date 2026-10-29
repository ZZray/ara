# ara-agent-lite-mcp

## 目录

- [用途](#用途)：这个 crate 做什么，和 Core/Harness 的关系。
- [配置](#配置)：与 Claude Code `.mcp.json` 对照的配置示例和展开规则。
- [宿主接入](#宿主接入)：`McpSession` 的用法。
- [行为](#行为)：命名、握手、结果转换、超时与未知副作用。
- [范围外](#范围外)：不支持的能力及其报错方式。
- [支持状态与已知限制](#支持状态与已知限制)：稳定级别、支持矩阵中的状态，以及与 Claude Code 行为的差异。
- [测试](#测试)：测试文件和运行方式。

## 用途

ARA Agent Lite 的 MCP 工具来源（需求 42）。宿主用 Claude Code 的 `mcpServers` 配置接入 MCP server，本 crate 负责解析配置、连接 stdio 与 Streamable HTTP server、握手、读取工具，并把每个工具包装成普通的 `AgentTool`。Core 不依赖本 crate，agent 循环只看到工具；Harness 也不依赖它，宿主把 `session.tools()` 放进 `RunRequest::extra_tools`。

## 配置

Lite 读取的就是 Claude Code `.mcp.json` 的格式。左边是 Claude Code 项目里的 `.mcp.json`，右边是 Lite 宿主读取的同一份文件：

| Claude Code `.mcp.json` | Lite（`McpConfig::from_json_str`） |
|---|---|
| `{"mcpServers": {...}}` | 相同 |
| stdio：`command`、`args`、`env`，`type` 可省略或为 `"stdio"` | 相同 |
| HTTP：`type: "http"`、`url`、`headers` | 相同 |
| `${VAR}`、`${VAR:-default}` 在 `command`、`args`、`env`、`url`、`headers` 中展开 | 相同，按宿主进程环境展开 |
| stdio 子进程继承当前环境再叠加 `env` | 相同 |
| `sse`、`ws`、OAuth、`headersHelper` | 不支持，返回 `unsupported_transport` / `unsupported_field` |

```json
{
  "mcpServers": {
    "files": {
      "command": "python",
      "args": ["-m", "my_files_server", "--root", "${PROJECT_ROOT:-.}"],
      "env": { "FILES_TOKEN": "${FILES_TOKEN}" }
    },
    "tickets": {
      "type": "http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer ${TICKETS_TOKEN}" }
    }
  }
}
```

规则：

- 变量已设置（包括空字符串）时用变量值，否则用默认值；两者都没有时这个 server 的配置无效，状态为 `Failed`，原因只写 server 名和变量名。
- 结构错误（类型不对、未知字段、不支持的传输）在解析时报错，错误码见 `ConfigError::code()`。
- `env` 和 `headers` 的值、以及展开时代入的变量值不写盘，也不出现在错误、状态或 `Debug` 输出里。server 的 stderr 只保留最后 4096 字节，写入失败原因前先去掉这些值。
- Windows 上 `npx` 这类 `.cmd` 程序需写成 `"command": "cmd", "args": ["/c", "npx", ...]`，与 Claude Code 的要求相同。

## 宿主接入

```rust
use ara_agent_lite_mcp::{McpConfig, McpOptions, McpSession};

let config = McpConfig::from_json_str(&std::fs::read_to_string(".mcp.json")?)?;
let options = McpOptions { required: ["files".to_string()].into(), ..Default::default() };
let session = McpSession::connect(config, options, cancel.clone()).await?;
for tool in session.tools() {
    request.extra_tools.push(ToolRegistration { tool, allow_in_plan: false });
}
// session.status(): 每个 server 的 Connected { tools, instructions } 或 Failed { reason }
// session.excluded(): 被排除的工具及原因
// ... 运行 Task ...
session.shutdown().await;
```

`McpOptions` 默认值：启动超时 30 秒，单次调用超时 600 秒，`required` 为空，`results_dir` 为空。server 的 `instructions` 在 `ServerStatus::Connected` 中，宿主可以写进系统提示。

## 行为

- **命名**：`mcp__<server>__<tool>`，两段中 `[A-Za-z0-9_-]` 以外的每个 UTF-16 码元替换为 `_`（与 Claude Code `normalizeNameForMCP` 一致）。全名超过 128 字符、规范化后重名（后到者）、`inputSchema` 不是对象的工具被排除。
- **schema**：根级 `anyOf`/`oneOf`/`allOf` 合并成一个对象 schema；`anyOf`/`oneOf` 的分组写在工具说明开头。说明超过 2048 字符截断。
- **握手**：请求 `2025-11-25`，接受 `2025-11-25`、`2025-06-18`、`2025-03-26`，其他版本判失败；之后发送 `notifications/initialized`，分页读取 `tools/list`（最多 32 页、512 个工具，超出时在 `excluded()` 里记一条 `tool: "*"`）。各 server 并发连接，单个失败只影响它自己，`required` 中的 server 失败则整体返回 `McpError::RequiredFailed` 并关闭其他连接。
- **HTTP**：每个请求一次 POST，回复可以是 JSON 或 SSE；保存并回送 `Mcp-Session-Id`，握手后带 `MCP-Protocol-Version`；不跟随重定向；关闭时发送 DELETE。
- **结果**：`text` 转文本，`image` 转图片内容，`resource`、`resource_link` 等转 JSON 文本，`structuredContent` 写入 details（content 为空时也作为文本）。`isError: true` 返回工具错误，超过 11000 字符时保留前后各 5000 字符。成功文本超过 50000 字符时，配置了 `results_dir` 则写入该目录并返回路径和前 2000 字符，否则截断到 50000 字符。
- **未知副作用**：调用超时、被取消（会发送 `notifications/cancelled`）或回复前断线时，非只读工具返回 details `{"effects_unknown": true, "source": "mcp_call_timeout" | "mcp_call_cancelled" | "mcp_connection_lost"}`，Harness 据此停在 `ReconciliationRequired`。`annotations.readOnlyHint: true` 的工具在这些情况下返回普通工具错误。调用从不自动重试。
- **资源**：任一已连接 server 声明 `resources` 时提供 `list_mcp_resources`（可选 `server`）和 `read_mcp_resource`（`server`、`uri`）。
- **进程**：stdio server 在 Windows 上放进带 kill-on-close 的 Job 对象，在 Unix 上放进独立进程组。`shutdown()` 先关 stdin，等 2 秒，然后结束整个进程树；宿主进程退出时 Job 句柄关闭，子进程树也会结束。

## 范围外

| 能力 | 处理方式 |
|---|---|
| `sse`、`ws`、`sse-ide`、`ws-ide`、`sdk`、`claudeai-proxy` 传输 | 配置解析返回 `unsupported_transport` |
| OAuth、`headersHelper` | 配置解析返回 `unsupported_field` |
| prompts、elicitation、sampling、roots、tasks 扩展 | server 发来的请求回 JSON-RPC `-32601`（`ping` 照常回复） |
| `listChanged` 动态刷新、HTTP GET 推送流、SSE 断点续传 | 不实现；工具列表在连接时读取一次 |
| 多来源配置合并、企业策略、权限审批 | 由宿主决定传入哪份配置 |
| Computer use | 不在需求 42 内 |

## 支持状态与已知限制

- 稳定级别：[稳定性规则](../ara-agent-lite-core/docs/stability.md) 把 `ara_agent_lite_mcp::*` 整体列为 experimental（版本 0.1.0），任何版本都可能变更，树外消费者引用会被门禁报告。宿主拿到的工具是普通 `AgentTool`，经 `RunRequest.extra_tools`（stable）注册。
- 支持矩阵：[支持矩阵](../ara-agent-lite-core/docs/support-matrix.md) 中 `tools.mcp` 为“部分支持”，缺口即上面的范围外各项，另有 Unix 进程路径只编译未运行；`jsonl.mcp`（start 帧 `mcp_servers` 与 `mcp_status` 事件）为“已支持”。
- 实现时记录的偏差（与 Claude Code 或设计稿不同，阶段 3 记录，本次按代码核对）：
  - 顶层和 server 条目中的未知字段直接报错（`invalid_config`、`unsupported_field`），不忽略。
  - `isError: true` 且带 `structuredContent` 时，`structuredContent` 以 `structuredContent: <JSON>` 附在错误文本后。
  - 合法的 `image` 内容转为 Core 图片块，不转成文本说明；base64 无效时才写 `[image: <MIME>, invalid base64 data omitted]`。
- 其他限制：
  - Windows 上子进程在 spawn 之后才加入 Job 对象，中间有极短的窗口；子进程若在这段时间里再派生进程，孙进程不受 Job 约束。
  - 发给模型的工具结果不做脱敏，server 返回的内容原样进入对话。

## 测试

只运行本 crate 的测试目标（需要 PATH 上有 `python`）：

```powershell
$env:CARGO_TARGET_DIR = 'D:\workspace\env\cache\rust-target-c'
cargo test --quiet --test names_props      # Property 46
cargo test --quiet --test config_props     # Property 47
cargo test --quiet --test result_props     # Property 48
cargo test --quiet --test stdio            # 受控 stdio server
cargo test --quiet --test http             # 测试内 Streamable HTTP server
cargo test --quiet --test interrupt_props  # Property 49
cargo test --quiet --test harness_mcp      # 真实 Core execute + Harness
```

受控 server 是 `tests/fixtures/fake_mcp_server.py`，用参数控制延迟、断线、错误、server→client 请求、资源和孙进程。
