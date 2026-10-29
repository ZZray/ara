# Design Records：ARA Lite Core 能力提升

本文件记录实施前必须核实的约束。每条记录在它阻塞的改动开始之前完成；完成之前，相关结论一律按推断处理。

- 规格：`.kiro/specs/ara-lite-core-capability/`（requirements.md、design.md、tasks.md）
- 代码起点：分支 `dev`，HEAD `56d55d209ecea00e80dfefecdd3e9d47fb62c47a`，工作区含既有未提交改动
- 本目录 `crates/ara-agent-lite-core/docs/evidence/lite-capability/` 不计入 `lite-eval-worktree-diff-v1` 代码版本 diff，原因见 tasks.md“执行约定”

字段说明：

- 约束：本条要回答的问题，以及它阻塞的任务与需求条款。
- 核实结论：核实得到的结论；未核实时写“待核实”。
- 来源位置：`文件:行 @ 版本或提交`，或证据文件路径加 SHA-256。括号内标“候选”的位置取自 design.md，尚未在源码中确认。
- 证据等级：源码支持、推断、运行确认之一。
- 日期：完成核实的日期，YYYY-MM-DD。
- 后续动作：结论对后续任务的影响，或尚未完成的核实。

其他需要记录的核实结论（任务 3.1 的 proptest 版本，任务 35.1、57.1 的稳定性分级审查）按同样字段追加为新小节。

## DR-P0-1 现有任务的 fixture、Oracle 与工具族

- 约束：为 S1、S2、S3、R1、H2 分别核实 fixture 源路径、Oracle 入口函数、用例数、`TaskKind` 与工具族，逐项标注证据等级。需求登记值为 S3 public 10/hidden 48、H2 128，需与脚本核对。阻塞 Task_Set 登记（任务 9.3；需求 1.1、1.2）。
- 核实结论：5 项任务的 fixture、Oracle 与用例数都能从脚本和仓外目录核对，需求登记值 S3 public 10/hidden 48、H2 128 与实际一致。下表路径相对 `B = D:/WorkSpace/tmp/ara-core-capability-20261022`。

  | 任务 | fixture 源路径（文件数） | Oracle 入口与类型 | 用例数 | `TaskKind` | 既有试验的工具族 |
  |---|---|---|---|---|---|
  | S1 | `B/analysis-private/S1-seed`（13） | `cpp_oracle`：g++ `-std=c++17` 编译候选程序后逐例比对标准输出；用例由 `cpp_cases('S1')` 以种子 `0xA621` 生成，存于 `S1-oracle.json` | hidden 13 | Coding | execute 模式 + `enable_local_tools`（`read_file`、`search_files`、`list_dir`、`write_file`、`edit_file`、`exec` 等内置工具）；new 变体另注册 `source_symbols`、`binary_inspect`（ctags、objdump、nm 后端） |
  | S2 | `B/analysis-private/S2-seed`（13） | 同 S1，`cpp_cases('S2')`，`S2-oracle.json` | hidden 35 | Coding | 同 S1 |
  | S3 | `B/coding-stream-stage/public-template`（18） | `fresh_evaluate`：cmake + Ninja + WinLibs g++ 全新构建，ctest 跑 public，再逐例跑 `author/S3-hidden-oracle.json` | public 10，hidden 48 | Coding | execute + 内置工具；S3-symbols 臂另注册 `source_symbols`（ctags） |
  | R1 | `B/analysis-live/R1/base/project`（3：README.md、specimen.exe、sample.hex） | `reverse_oracle`：从最终回复提取 python 代码块与 json 预测块，在 Python 中逐例比对；用例由 `binary_cases()` 以种子 `0xB621` 生成，存于 `analysis-private/R1-oracle.json` | hidden 28，另有 3 个预测输入 | ReverseAnalysis | plan 模式，只剩只读内置工具（`read_file`、`search_files`、`list_dir` 等）；new 变体另注册 `binary_inspect`、`source_symbols`（`allow_in_plan`）；`exec`、`run_script`、`write_file`、`edit_file` 出现即判试验无效 |
  | H2 | `B/reverse-holdout-stage/public-template`（3：README.md、specimen.exe、sample.hex） | `evaluate_submission`：同 R1 的提取方式，8 秒超时，比对 `author/H2-oracle.json`（SHA-256 `7a234ff0…f49f`） | hidden 128（有效 80、无效 48），4 个预测输入也在 128 例内 | ReverseAnalysis | plan + 只读内置工具 + `binary_inspect`（`ctags: false`，无 Host facts） |

  - 5 个 fixture 目录逐文件 SHA-256 与冻结计划的 `public_pins` 全部一致（S1、S2、R1 对照 `analysis-private/private-plan.json`，S3、H2 对照各自 stage 的 `manifest.json`）。S1、S2 的工作区副本 `analysis-live/S1|S2/*/project` 已被模型修改，因此以 seed 目录为 fixture 源。
  - `TaskKind` 是本规格的登记值，既有脚本不使用该枚举。依据是任务性质：S1、S2、S3 修改 C++ 工程（Coding），R1、H2 静态还原二进制（ReverseAnalysis）；`Reasoning` 不用于这 5 项。
  - 新的 Capability_Test_Host 不沿用 `enable_local_tools`（DD-03），登记时 `profile.tools` 按模式改写：Coding 任务用 `ws_read_file`、`ws_list_dir`、`ws_search_files`、`ws_write_file`、`ws_edit_file`、`run_public_checks`；ReverseAnalysis 任务只用 `ws_read_file`、`ws_list_dir`、`ws_search_files`、`binary_inspect`。
  - 污染限制：S1、S2、R1 的用例生成器和参考实现都在仓内 `verify_analysis_live.py`（固定种子），H2、S3 的 Oracle 与 public-template 是同级目录。模型可用绝对路径读取，因此这 5 项的 hidden 用例都不能算作模型不可读。
- 来源位置（均 @ `56d55d209ecea00e80dfefecdd3e9d47fb62c47a` 加工作区，脚本未改动）：
  - `crates/ara-agent-lite-core/tests/verify_analysis_live.py:27-28`（PRIVATE、TASKS），`:264`（`cpp_cases`），`:284`（`cpp_oracle`），`:315`（`binary_cases`），`:333-398`（`prepare` 写 seed、oracle 与 R1 fixture），`:474`（`reverse_oracle`），`:529`（预算），`:537-545`（new 变体的 ctags、objdump、nm），`:558`（mode 与 `enable_local_tools`），`:593-594`（R1 禁用工具检查）
  - `crates/ara-agent-lite-core/tests/verify_stream_coding_live.py:34-37`（ARMS），`:158`（`fresh_evaluate`），`:470-474`（execute、`analysis_config` 的 ctags）
  - `crates/ara-agent-lite-core/tests/verify_reverse_holdout_live.py:33-35`（ARMS），`:213-215`（plan、`ctags: false`），`:256`（`evaluate_submission`），`:417-419`（plan 模式、`analysis_config`）
  - `crates/ara-agent-lite-core/src/execution.rs:28-52`（`RunMode::Plan` 允许的工具），`src/analysis_tools.rs:269-272`、`:716`（`binary_inspect`、`source_symbols`，`allow_in_plan: true`），`crates/ara-agent-lite-harness/src/types.rs:14-18`（`TaskKind`）
  - `B/reverse-holdout-stage/manifest.json` 的 `oracle` 字段（cases 128、valid 80、invalid 48），`B/coding-stream-stage/manifest.json` 的 `public_tests` 10、`hidden_cases` 48
- 证据等级：fixture 路径、文件数、pins 一致性、用例数为运行确认（2026-10-29 本机重新计算 SHA-256 并读取 Oracle 文件）；Oracle 入口与工具族为源码支持；`TaskKind` 归类为推断。
- 日期：2026-10-29
- 后续动作：任务 9.3 按上表登记，fixture SHA 用 `lite-eval-tree-v1` 对上述目录重新计算；`oracles/{s1,s2,r1}.py` 调用 `cpp_oracle` / `reverse_oracle`，`s3.py` 调用 `fresh_evaluate`，`h2.py` 调用 `evaluate_submission`。污染限制写入 Run_Manifest 的 limits。

## DR-P0-2 现有 5 项任务的 Holdout_Status

- 约束：依据既有证据判断 S1、S2、S3、R1、H2 是否已被适应性使用。有任何证据即登记为 `not new holdout`，首次适应性使用日期取最早证据日期，并列出至少 1 条改动标识。阻塞 Task_Set 登记（任务 9.3；需求 1.6）。
- 核实结论：5 项全部登记为 `not new holdout`，首次适应性使用日期都是 2026-10-23。新登记的 `new holdout` 只有 SH1（任务 10）。

  | 任务 | 证据 | 改动标识 | 证据等级 |
  |---|---|---|---|
  | S1、S2 | 2026-10-23 07:40–07:45 完成 base/new 共 4 个真实 Run，fixed Oracle 全部通过；随后看过候选产物，追加了原 Oracle 之外的 3 个畸形 hex 补充用例（S1/base 0/3、S1/new 3/3、S2 两侧 0/3），并记录 “supplemental parser debt remains” | `coding-reverse-analysis-20261023.result.json` 的 `supplemental_contract`（result SHA-256 `57c65351…1d09`）：观察 Run 结果后补充评估用例 | 推断（补充用例依据 Run 结果设计有文件证据；是否据此改了 Lite 代码没有直接证据。按“有任何证据即登记”从严） |
  | S3 | 2026-10-23 进度记录写明原 48 例被适应性复用，“restored read windows and public-PASS submission instruction” | `core-workflow-20261023.progress.json` 的 `oracle_scope` “original48 reused adaptively”、`same_oracle_not_new_holdout: true`；改动：恢复读窗口，加入 public PASS 后提交的指令 | 源码支持（进度记录原文） |
  | R1 | 2026-10-23 07:46–07:47 两个 Run 都因 `assistant stopped with length` 失败（4096 输出被推理 token 占满）；08:04 起 `reverse-policy-live-v1` 在同一 specimen 与 Oracle 上做 A–E 五臂诊断（策略、Host facts、输出 8192），其 `limits` 自述 “same original specimen/oracle” | 生成策略 `thinking_type_disabled` 与 `max_output` 8192 的选定；H2 任务说明新增 “Check the actual return value of each validation branch…” 的提示 | 运行确认（诊断 Run 的 plan 与结果文件存在） |
  | H2 | 2026-10-23 08:27 起 H2-D、H2-G 各 1 个 Run；H2-G 的工具参数错误随后促成 nullable schema 修改与案例记录 | `coding-nullable-20261023.result.json`；`docs/knowledge/host-contracts.md` 的 “CASE-20261023：nullable schema 与原生工具实参” | 源码支持（知识库与结果文件原文） |

  - 预模型改动不计入适应性使用：S1、S2、R1 的 3 次 harness 修订（`private-plan.json` 的 `harness_amendments`）和 H2 的 1 次修订都记录 `model_calls_before_amendment: 0`，且声明 fixture 与 Oracle 字节未变。
- 来源位置：
  - `crates/ara-agent-lite-core/docs/evidence/coding-reverse-analysis-20261023.result.json:180-376`（`model_trials`、`r1_budget_observation`、`supplemental_contract`、`limits`）
  - `crates/ara-agent-lite-core/docs/evidence/core-workflow-20261023.progress.json:88`、`:371`、`:685-690`
  - `crates/ara-agent-lite-core/docs/evidence/coding-reverse-holdout-20261023.result.json:186-192`；`coding-nullable-20261023.result.json:212-216`；`crates/ara-agent-lite-core/docs/knowledge/host-contracts.md:121-125`
  - `B/analysis-live/{S1,S2,R1}/{base,new}/result.json`（修改时间 2026-10-23 07:40–07:47）；`B/reverse-policy-live-v1/plan.json`（arms A–E、limits）与 `A–E/result.json`（只有 B 臂提交，Oracle FAIL）；`B/analysis-private/private-plan.json` 的 `harness_amendments`
- 证据等级：逐项见上表；整体结论（5 项都不是新 holdout）为源码支持。
- 日期：2026-10-29
- 后续动作：任务 9.3 在 `holdout-log.jsonl` 为 5 项各追加一条 `marked_not_new`（日期 2026-10-23，changes 取上表改动标识）。之后的基线与对比只能写“已适应性使用任务上的结果”，泛化结论只依据 SH1。

## DR-P0-3 模型、协议、路由、授权与预算

- 约束：按 ARA Manager → OMP 管理 → CAS 路由的顺序核实当前 model ID、协议、路由与授权，OpenRouter 免费模型为备选；依据既有试验证据确定各任务预算（含 SH1、`LONG-COMPACT`、`LONG-TRIAL`、`consumer_task`），并确定价格是否已知；记录核实时间与证据。阻塞 Run_Policy 冻结（任务 8.3、12.3；需求 3.2、3.3）。每次真实模型试验前按同一方法复核并更新本条。没有可用路由时，为 M-P0-RUNNER 追加 open 阻断条目并发送 Blocker 通知。
- 核实结论：有可用路由，不需要追加阻断。采用 CAS 直连，与既有全部真实试验相同。
  - 路由顺序核查（2026-10-29 14:28 +08:00）：ARA Manager（`ara serve`，127.0.0.1:7337）在监听；OMP 管理端口 127.0.0.1:18799 未监听，Manager → OMP 链路当前不可用。因此退到既有试验使用的 CAS 直连。
  - 模型与协议：model ID `deepseek-v4.1-flash`，协议 `openai-completions`，endpoint `https://cas.ciqtek.com/v1`，取自既有路由记录 `B/live-coding/result.json`。
  - 授权：凭据环境变量 `CAS_API_KEY`（存在，值未读出）。design.md 示例中的 `ARA_LITE_EVAL_MODEL_KEY` 不存在，Run_Policy 的 `credential_env` 写 `CAS_API_KEY`。
  - Catalog：`GET /v1/models` 返回 200，耗时 182 ms，14 行，含 `deepseek-v4.1-flash`；响应 4392 字节，SHA-256 `d76c6907dadbd1b043e90c790af1d4dfd676c489f2f664e8aebdd06066375a6b`，与既有快照 `B/reverse-policy-live-v1/A/catalog.response.native.bin` 逐字节相同。该行只有 `supported_endpoint_types`、`reasoning_efforts`（off/low/high/max）、`reasoning_transport: deepseek` 等字段，没有上下文长度、输出上限和价格。
  - 输出上限探测：一次最小请求（`max_tokens: 32768`，`thinking: {type: disabled}`，提示 “Reply with OK only.”）返回 200，耗时 654 ms，`finish_reason: stop`，用量 prompt 9 / completion 1。这只说明 32768 作为请求值被接受，不说明模型能实际输出这么长。响应的 `model` 回显为 `deepseek-flash`，与请求 ID 不同；沿用 H2 的处理方式，保留原始回显，身份记为未验证。
  - 价格：catalog 没有价格字段，CAS 与 OMP 配置中也没有可引用的单价，价格未知。Run_Policy 的 `price` 三个字段写 `null`，成本按需求 4.4 记 unknown。
  - 备选：`OPENROUTER_API_KEY`、`RAY_GATEWAY_KEY` 存在，本次未探测；只有 CAS 不可用时再按同样方法核实。
  - 生成策略：Run_Policy 用 `thinking_type_disabled`。依据：R1 两个 Run 在 provider 默认策略、4096 输出下都因推理 token 占满上限而 `length` 失败；`reverse-policy-live-v1` 五臂中只有 B 臂（`thinking_type_disabled`，4096）交出提交；S3 与 H2-D 用该策略都完成了提交。S1、S2 的既有 Run 使用 provider 默认，与新基线不可直接比较，基线会重新运行。
  - `max_output`：Run_Policy 取 32768。基线代码在 Core 内仍收窄到 8192（S3、H2 曾以 8192 运行成功），P2-a 打开 Host 上限后才可能生效更大值，因此 Run_Policy 前后不变（design.md “Run_Policy” 节）。
  - 预算（任务 12.3 写入 `budgets`，单位：次、秒）：

    | 键 | max_runs | max_calls | deadline_seconds | run_max_calls | run_max_seconds | 依据 | 证据等级 |
    |---|---|---|---|---|---|---|---|
    | S1、S2 | 2 | 48 | 1800 | 24 | 600 | 既有单 Run 上限 24 次 / 600 秒，实际 12–16 次、43–92 秒；第 2 个 Run 留给 Host 审查续写 | 运行确认（单 Run 数据），Run 数为推断 |
    | R1、H2 | 2 | 56 | 1800 | 28 | 720 | 既有单 Run 上限 28 次 / 720 秒（H2 manifest `suggested_run_budget` 同值）；R1 3–4 次即因 length 停止，H2-D 用 11 次 | 运行确认（单 Run 数据），Run 数为推断 |
    | S3 | 3 | 120 | 3600 | 40 | 900 | 既有 Lite 单 Run 40 次 / 900 秒两臂均未通过 hidden；放宽到 3 个 Run 以覆盖 Host 审查续写 | 推断 |
    | SH1 | 3 | 120 | 3600 | 40 | 900 | 同为多文件 C++ 编码任务，规模按 S3 取 | 推断 |
    | LONG-COMPACT | 2 | 80 | 3600 | 60 | 1800 | 需要足够调用数积累上下文以触发压缩；未触发时按需求 25.6 记未覆盖，不改 Run_Policy | 推断 |
    | LONG-TRIAL | 6 | 160 | 5400 | 40 | 1800，另设 `max_output` 2048 | 需求 35 要求诱发 Length 并覆盖压缩、续写与 resume；R1 在 4096 下即出现 length | 推断 |

  - `consumer_task`：S1。理由：规模最小（13 文件），base 两次都通过，单 Run 不到 70 秒，适合验证只用 stable 接口的外部 Host。
- 来源位置：
  - 探测证据（仓外 `D:/WorkSpace/tmp/ara-lite-capability/`）：`dr-p0-3-catalog.response.bin`（SHA-256 `d76c6907…6a6b`）；`dr-p0-3-probe-32768.request.json`（SHA-256 `94c8ba29cb96543cee7a722ff6814350131ee5138529130ca247b30a72083690`）；`dr-p0-3-probe-32768.response.bin`（SHA-256 `e514db86be85079eb25f94cebf93bba8254b655c940ec5cd4221e76e835008c3`）
  - 既有路由与快照：`B/live-coding/result.json`；`B/reverse-policy-live-v1/A/catalog.response.native.bin`；`B/reverse-policy-live-v1/plan.json` 与 `A–E/result.json`
  - 既有预算：`crates/ara-agent-lite-core/tests/verify_analysis_live.py:529`、`:558`；`verify_stream_coding_live.py:396`、`:470-472`；`verify_reverse_holdout_live.py:213`、`:417`；`B/reverse-holdout-stage/manifest.json` 的 `suggested_run_budget`；`crates/ara-agent-lite-core/docs/evidence/coding-reverse-analysis-20261023.result.json`（各 Run 的调用数、耗时与 `r1_budget_observation`）
  - 生成策略实现：`crates/ara-agent-lite-core/src/model.rs:22-58`
- 证据等级：路由、catalog、授权变量存在、32768 被接受、价格缺失为运行确认（2026-10-29 本机探测）；预算逐项见上表。
- 日期：2026-10-29
- 后续动作：任务 12.3 按上述取值冻结 `run-policy.json`，`route.decision` 写 “OMP 18799 未监听，CAS 直连”。每次真实模型试验前重做同样的 catalog 探测（比较 SHA-256 与目标行），结果追加到本条；catalog 变化或 401/403 时先更新本条再试验。

## DR-P2a-1 输出上限 8192 的来源与 OMP 行为

- 约束：在修改 `agent.rs` 中 `request.max_output.min(8192)` 之前完成。写明 8192 的出现位置（文件与行号）、OMP 对单次输出上限的处理（OMP 版本与文件位置）、硬约束或历史保守值的结论，以及证据等级。结论决定常量 `BUILTIN_CEILING_IS_HARD` 的取值（任务 40.1；需求 15.1）。
- 核实结论：8192 是 Lite 的历史保守值，不是上游硬约束；`BUILTIN_CEILING_IS_HARD = false`。
  - Lite 中 8192 出现在 4 处（改动前）：`agent.rs:1043` `request.max_output.min(8192)`，`agent.rs:1057` 事件文本自称 “local 8192 ceiling”；`model_caps.rs:58` 目录行未声明 maxTokens 时 `unwrap_or(8192)`；`ara-agent-lite-jsonl/src/runtime.rs:59` `(1..=8192).contains(&run.max_output)`；`examples/business_host.rs:172` `clamp(256, 8192)`。
  - OMP v18.1.8（596f2da）：agent 请求取调用方的 `maxTokens`，否则取模型声明的 `maxTokens`（`packages/ai/src/stream.ts:2011`）。OpenAI 兼容路由再与 `OPENAI_MAX_OUTPUT_TOKENS = 64000` 取最小（`openai-shared.ts:617-632`，`types.ts:70`）；Anthropic 未声明时用 64000（`anthropic.ts:4058-4073`）。压缩摘要上限 `min(0.8 × 16384, 16384)`（`compaction.ts:191/203/857`）。全局 8192 上限不存在；唯一的模型级 8192 默认值只用于自动发现、未声明 maxTokens 的 Anthropic 接口模型，源码注释称其为保守值（`model-discovery.ts:142-146`）。
  - b91db66：逻辑相同，只有行号变化；Anthropic 未声明上限改用 `UNKNOWN_MODEL_MAX_OUTPUT_TOKENS = 64000`（`anthropic.ts:4686`）。
  - 本机 CAS 探测（DR-P0-3）：`max_tokens: 32768` 被接受。
- 来源位置：上游源码 `https://github.com/can1357/oh-my-pi` @ `596f2da7101178214aa27a753529d15e6b7ad91d` 与 `b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`（仓外部分克隆 `D:/WorkSpace/tmp/omp-src`）；Lite 位置 @ `56d55d209ecea00e80dfefecdd3e9d47fb62c47a` 加工作区。
- 证据等级：源码支持（读取两个固定提交的上游源码）；未做针对 8192 以上输出长度的运行确认。
- 日期：2026-10-29
- 后续动作：`output_limit.rs` 写入 `BUILTIN_CEILING_IS_HARD = false`：无 Host 上限时仍取 8192，Host 上限存在时可以超过 8192。JSONL 准入改用 `admission_bound`。`model_caps.rs:58` 的回退值不改（需求未要求）。

## DR-P2e-1 写入工具注册位置与 S3 实际写入次数

- 约束：在修改 Capability_Test_Host 工具集之前完成。写明 Host 注册写入工具的代码位置（文件与行号）、所依据的 S3 运行事件标识（事件文件路径与 SHA-256）、S3 实际调用的每个写入工具名称及调用次数，并逐项标注证据等级（运行确认或推断）。“S3 使用 `write` 3 次”在核对 S3 运行事件之前保持推断等级，不得记为运行确认（任务 53.1；需求 20.1）。
- 核实结论：待核实
- 来源位置：待核实（候选：`examples/business_host.rs:159`（`enable_local_tools` 透传），`local_tools.rs:30-31`（`write_file`、`edit_file` 定义）；S3 事件文件位于 `D:/WorkSpace/tmp/ara-core-capability-20261022` 下，具体路径待查）
- 证据等级：待核实
- 日期：待填
- 后续动作：待核实

## DR-P3-1 NUL 与 Unicode 字节被改写的环节（Diagnosis_Record）

- 约束：以 `header\u0000中文😀` 为输入，期望 UTF-8 字节为 `68 65 61 64 65 72 00 E4 B8 AD E6 96 87 F0 9F 98 80`（17 字节）。记录 Lite 工具 I/O、JSONL 传输、模型补丁三个环节入口与出口的十六进制字节，判定首次出现 `EF BF BD` 的环节。结论只取三个环节之一、“其他环节（注明具体位置）”或“未复现”；附可重新执行的复现步骤（Lite 提交标识、触发操作、输入内容），并注明原始输入是否含无效 UTF-8 字节序列，含则标注需另立显式编码要求。结论为“未复现”时列出已尝试的步骤与环境（操作系统、Lite 提交标识、客户端类型），状态记待验证。阻塞需求 26 的修复（任务 27.1、32.1；需求 26.1、26.6）。
- 核实结论：模型补丁。`EF BF BD` 首次出现在模型写出的候选代码运行时，Lite 工具 I/O 与 JSONL 传输都没有改写字节。原始输入不含无效 UTF-8（样例严格 UTF-8 解码成功），不需要另立显式编码要求。
  - 模型补丁：闭合预算快照 live Run 的 `rpc-19` 是模型发出的 `source_replace`（native_call_id `call_00_ywnxAk8yLp1e1nFMug4x7052`），`new` 参数新增 `decode_git_bytes`，末行为 `return text.replace("\x00", "\ufffd") if "\x00" in text else text`。这行在参数里是 ASCII 转义文本（`5C 78 30 30`、`5C 75 66 66 66 64`），帧、Journal 第 34 条记录和磁盘上的 `git_service.py` 都没有原始 `00` 或 `EF BF BD`，三处这一行的字节相同。按帧中 `args_sha256` 重算参数哈希一致（`afbf8912…fb34b`）。在新命名空间执行这段模型代码：入口 `68 65 61 64 65 72 00 E4 B8 AD E6 96 87 F0 9F 98 80`，出口 `68 65 61 64 65 72 EF BF BD E4 B8 AD E6 96 87 F0 9F 98 80`（str 和 bytes 两种输入相同）。快照报告的 `actual: "header�中文😀\nsecond�line\n"` 就是这段代码的运行结果。
  - Lite 工具 I/O：`local_tools::tools(root)` 的 `write_file` 参数 JSON 为 `{"path":"bytes.txt","content":"header\u0000中文😀"}`；磁盘 `68 65 61 64 65 72 00 E4 B8 AD E6 96 87 F0 9F 98 80`；`read_file` 出口相同。未复现。
  - JSONL 传输：只做了代码阅读和编解码层核对，没有用真实 sidecar 进程做样例往返（留给任务 32.3）。sidecar `transport.rs:14-32` 按 `\n` 分帧并严格校验 UTF-8，不做替换；Python 客户端 `encoded()`（`ara_agent_lite.py:93`）与 Node 客户端 `JSON.stringify` 编码 `tool_reply` 时把 U+0000 写成 `\u0000`，帧中无原始 `00`；两端解码（`json.loads`、`TextDecoder(fatal)` + `JSON.parse`）出口都是 17 字节样例。另有真实进程证据：`rpc-19` 帧由当时的 sidecar 写出，其参数字节与 Journal、磁盘一致。
- 复现步骤：
  1. Lite 提交 `f0a7570036aa9b57cb141057e1bdd1101b66fa08`（分支 `dev`，工作区含未提交改动），本机 Windows，客户端类型：模型补丁环节为闭合预算快照的 Harness（JSONL 帧证据），工具 I/O 环节为 Core 直接调用。
  2. 模型补丁环节：`python D:/WorkSpace/tmp/ara-dr-p3-1/model_patch_leg.py`（只读快照目录，打印上述哈希、计数与入口/出口十六进制）。仓内回放：`cargo test --test byte_roundtrip dr_p3_1_`，fake upstream 把 `rpc-19` 参数原文作为模型的 `edit_file` 调用回放（夹具 `tests/fixtures/dr-p3-1/jsonl-rpc-19-request.json`，SHA-256 `dc0ad1f1de11ec906e788de49265698dcafc1d77f92dde9423327a195b2cd18c`）。
  3. 工具 I/O 环节：`cargo test --test byte_roundtrip dr_p3_1_tool_io_leg -- --nocapture`。
  4. JSONL 编解码层：`python D:/WorkSpace/tmp/ara-dr-p3-1/jsonl_codec_leg.py`。
- 来源位置：
  - 快照（仓外 `D:/WorkSpace/tmp/ara-core-capability-20261022/closed-budget-snapshot-v1-20261025/acr-live-snapshot-v1/`）：`harness/jsonl-rpc-19-request.json`（SHA-256 `dc0ad1f1…d18c`）；`state/session.journal/segment-00000000.jsonl` 第 34 行（文件 SHA-256 `cf48224fac8a71e57f58ec2cb29dd7ef40ed870b8b4c51e95a60723733c87f24`）；`host/work/workspace/app/services/git_service.py`；`../root-semantic-preservation-probe.json`
  - 仓内：`crates/ara-agent-lite-jsonl/docs/evidence/closed-budget-snapshot-20261025.result.json:100-108`、`.content.json:133`；`crates/ara-agent-lite-core/src/local_tools.rs:96`（`read_text_handle` 严格 `String::from_utf8`）、`:666-673`（`write_file` 原样 `std::fs::write`）
- 证据等级：模型补丁结论为运行确认（执行了快照中的模型代码并比对三处字节）；工具 I/O 为运行确认；JSONL 传输为源码支持加编解码层运行确认，真实 sidecar 进程往返未做。
- 日期：2026-10-29
- 后续动作：任务 32.1 按“模型补丁”处理，Lite 代码不改；`tests/byte_roundtrip.rs` 的三个 `dr_p3_1_*` 测试回放补丁原文并断言磁盘字节与补丁目标一致、补丁内已有 `EF BF BD` 原样保留、样例补丁落盘为 17 字节。32.2 的 Property 35 在同一文件。32.3（真实 sidecar 与 Python、Node 进程往返）留给 JSONL 轮次。

## DR-P4-1 事件 `type` 清单与样本采集场景

- 约束：在 Event_Enum 改动之前，对源码与采样语料统计全部事件 `type` 值，并确定样本采集场景（普通完成、工具、压缩、Length 重规划、闭合预算、Goal，以及 P2 各功能开启）。任务 56.2 采集的样本文件 SHA-256 补记于本条，之后不重新生成。阻塞 Event_Enum 实施（任务 56.1、56.2；需求 28.1、28.4）。
- 核实结论：Core 发出 19 种 `type`，与 design.md 的候选清单一致，没有其他值。
  - 清单：`agent_start`、`agent_end`、`text_delta`、`text`、`tool_start`、`tool_end`、`model_capabilities`、`generation_policy`、`model_call`、`model_result`、`context_status`、`compact`、`length_recovery`、`goal_status`、`goal_update`、`task_update`、`completion_gate`、`stuck_detector`、`compaction_refresh`。源码中所有事件都用 `json!({"type":"<字面量>",…})` 构造，没有动态 `type`，也没有事后改写 `type` 的位置。
  - 采样场景 13 个，每个场景一个文件，见 `tests/event_scenarios/mod.rs`：`completion`（普通完成）、`tools`（成功、失败和 `effects_unknown` 三类工具结果，加 `task_update` 有无 `evidence` 两种）、`generation_policy`、`compaction`（两次 Run，第二次压缩）、`compaction_refresh`、`length_replan`（`replan_once`）、`length_stop_evidence`、`closed_budget`（`termination`）、`goal`（Goal Run 加 `/goal status` 命令 Run）、`completion_gate`（失败后通过，以及预算用尽时 `agent_end.completion_gate`）、`stuck_detector`（`rule_hit`、`correction`、`no_progress`）、`output_limit`（Host 上限 1024）、`summary_budget_stop`。样本共 221 行，19 种 `type` 各至少 1 行。
  - 采集方式：在加入 `events.rs` 之前，用当时的代码经 loopback provider 运行，`tests/event_samples.rs` 的 `capture_events_v1`（默认忽略，需 `LITE_EVENTS_V1_CAPTURE=write`，目录已存在时拒绝写入）写出样本与 `execute-golden.json`（每个场景每次 Run 的事件 `type` 与排序后的顶层字段名）。字符串中的临时目录替换为 `%ROOT%`，其他值原样保留；`.gitattributes` 的 `-text` 保证检出后字节不变。
  - 样本 SHA-256（`tests/event_compat.rs` 的 `SAMPLE_SHA256` 同步核对）：
    - `closed_budget.jsonl` `cac3a7a0643a4e30445e1ce30594076d975c748b2ce433c779630766b4857ce7`
    - `compaction.jsonl` `579caa14bb264bb751cfc2185a35ebd99866aa3ecfac70acad51479a9498feb3`
    - `compaction_refresh.jsonl` `5a24d1821c73d68529664c9dc4ae3cb377c779218d8fc942f2098c10b9f4805a`
    - `completion.jsonl` `0485de05539718ec993e9cba3bc9168d9d1c9e2182dc9c5616e3ea91e251f6cc`
    - `completion_gate.jsonl` `b7275859b63628e9166e852ca99c950564da434d2e4cc429e5e627558e56c521`
    - `execute-golden.json` `20c62425232fdc9387c0d16dc67c221f78614881e6a59f56bdbd087e2abcb7dd`
    - `generation_policy.jsonl` `1e49a1e42eeee12484b1756a2a0c5e84d1eac70e3da12bd25fd82481071a1f28`
    - `goal.jsonl` `c307c4ce80cbf0ea5fd4631b9ef15ad23e6496fdcf2b4ad2342916649b0ee978`
    - `length_replan.jsonl` `14c61b3a6d8428756d548048cb1e868ea420623f52a9d58c957ebaf4cd6896ca`
    - `length_stop_evidence.jsonl` `30f0fa70edc89a1703cd71662145525ac7f5ec54634fa8699fe8ead40ed30d50`
    - `output_limit.jsonl` `090b944343e087990ab8d1e66f6e29585d0334615d6fec27395cccacf86dd948`
    - `stuck_detector.jsonl` `8ac630fb7641e9788e1818fdbf1f69bdf4cd7794fcb62756da592cb5f8051b13`
    - `summary_budget_stop.jsonl` `4336320e30893a62caae0aebc0b3d703e30153abd85f26026f492b2a8c94544d`
    - `tools.jsonl` `cf950a8df5f21a168885bc217a9a353fd5a8fcb87fc7971a430f9b163739c379`
  - 字段规则（取自样本）：顶层字段全部有固定 JSON 类型，没有浮点数；`agent_end.code` 用 `i64`，其余整数用 `u64`。只在部分事件出现的字段（`agent_end` 的 `termination`、`stop_evidence`、`completion_gate`，`tool_end.effects_unknown`，`model_call.context_observation`，`model_result` 的 `elapsed_ms`、`title`、`content_types`、`provider_error_present`，`task_update.evidence`）用 `Option<T>` 加 `skip_serializing_if`，显式 `null` 判为类型错误。唯一“总是出现、可为 `null`”的字段是 `completion_gate.feedback_ref`，用必填的 `Option<String>`；样本中没有“缺省与 `null` 并存”的字段，因此没有用到 `Option<Option<T>>`。Goal、预算、终止证据、遥测等由其他模块定义的嵌套内容保持为 JSON 对象。
  - 已有的顺序不确定性：同一轮的 `model_result`（provider 任务在流结束时发送）与 `text_delta`（agent 消费流时发送）由两个任务各自发送，相对顺序在采集前的代码中就不固定（两次预览采集在 5 个场景中出现过对调）。黄金对照比较前把每个 `model_result` 移到与之竞争的相邻 `text_delta` 之前，其余顺序原样比较。Event_Enum 不改变这一行为。
- 来源位置：`src/agent.rs:700-725`（HostEmitter 的 `agent_start`、`text_delta`、`text`、`tool_start`、`tool_end`）、`:940`（`completion_gate`）、`:1171-1173`、`:1204-1205`、`:1304`、`:1310`、`:1347`、`:1488`、`:1499`、`:1615`、`:1629`、`:1815`、`:1853`、`:1946`、`:2487`、`:2490`；`src/model_telemetry.rs:44`、`:54`；`src/compaction_refresh.rs:141`；`src/execution.rs:217`、`:330` @ `3d12d49c7b02a98052a307d8349123c5a7a02cb1`（工作区含未提交改动；Core 源码在采集时无未提交改动）
- 证据等级：运行确认（样本由实际 `execute` 运行产生；`event_compat` 重新运行 13 个场景并与黄金对照一致）
- 日期：2026-10-29
- 后续动作：样本不再重新生成，提交待用户授权。`src/events.rs` 按上述字段规则实现；`tests/event_enum.rs`（Property 38）、`tests/event_enum_errors.rs`（Property 39）、`tests/event_compat.rs` 读取这些样本。新增事件 `type` 或字段时需把 `EVENT_SCHEMA_VERSION` 加 1，并另行补采样本（新文件，不覆盖本组）。

## DR-X-1 属性测试依赖 proptest 的精确版本

- 约束：Rust 属性测试统一使用 `proptest`（DD-12）。任务 3.1 要求核实 `proptest =1.11.0` 在 crates.io 存在、能用本机 rustc 1.97.1 编译、许可证在 `deny.toml` 的 `[licenses].allow` 内；不可用时改选可用的精确版本并记录。阻塞全部 Rust 属性测试（任务 3.3 起），以及任务 19.1、20.2 在 Core、Harness、JSONL 加入的 dev 依赖。
- 核实结论：可用，采用 `proptest = { version = "=1.11.0", default-features = false, features = ["std"] }`。
  - crates.io 上存在 1.11.0，`yanked=false`，发布于 2026-03-24，checksum `4b45fcc2344c680f5025fe57779faef368840d0bd1f42f216291f0dc4ace4744`。最新版本为 1.12.0，按设计仍锁 1.11.0。
  - 声明的 `rust-version = 1.85`，不高于本机 rustc 1.97.1。`tools/lite-eval` 在 rustc 1.97.1 下编译 proptest 1.11.0 成功，`cargo clippy --all-targets -- -D warnings` 与 `cargo test` 均通过。
  - 许可证为 `MIT OR Apache-2.0`，两项都在 allow 列表内。上述特性下 proptest 的非 dev 传递依赖共 22 个包，许可证表达式都可选 MIT 或 Apache-2.0；其中 `unicode-ident 1.0.27` 另含 `Unicode-3.0`，也在 allow 列表内。
  - `cargo deny --manifest-path tools/lite-eval/Cargo.toml --config deny.toml check licenses` 只有一个失败项：`webpki-roots 1.0.10`（CDLA-Permissive-2.0，经 ara-agent-lite-core 引入）。它是 `deny-baseline.json` 已记录的已知项，与 proptest 无关。
  - `tools/lite-eval/Cargo.lock` 以 Harness 的 Cargo.lock 为起点解析，因 proptest 新增 7 个包：proptest 1.11.0、rand 0.9.5、rand_chacha 0.9.0、rand_core 0.9.5、rand_xorshift 0.4.0、unarray 0.1.4、ppv-lite86 0.2.21；其余包版本与 Harness 锁定值相同。
- 来源位置：crates.io API `https://crates.io/api/v1/crates/proptest/1.11.0`；`cargo info proptest@1.11.0`；`deny.toml:11-29`（`[licenses]` 与 `allow`）@ `56d55d209ecea00e80dfefecdd3e9d47fb62c47a`；`tools/lite-eval/Cargo.toml`、`tools/lite-eval/Cargo.lock`（工作区新增，未提交）
- 证据等级：运行确认
- 日期：2026-10-26
- 后续动作：任务 19.1、20.2 在 Core、Harness、JSONL 的 `[dev-dependencies]` 加入同一行。这些 crate 的 lockfile 会新增上述包中尚未锁定的部分，任务 61.1 的 cargo-deny 比对把它们列为新增依赖。
