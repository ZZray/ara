# Full/Lite 支持矩阵（初稿）

本文是需求 30 的 Support_Matrix，机器可读副本为同目录的 [support-matrix.json](support-matrix.json)，两份内容一致，`lite-eval gate` 读取 JSON。

- 本阶段不合并 Full 与 Lite。两边的支持状态分开列出，一方的源码位置或测试不作为另一方已支持的依据。
- Full 指同仓库的 `crates/ara-agent` 与 `crates/ara-workflow`；`ara-tools`、`ara-session`、`ara-mcp` 等其他完整版 crate 不在本矩阵的 Full 范围内，因此相关能力在 Full 一列记为“不支持”，并在说明中注明。
- Lite 指 `crates/ara-agent-lite-core`、`crates/ara-agent-lite-harness`、`crates/ara-agent-lite-jsonl`、`crates/ara-agent-lite-mcp`。
- 依据版本：分支 `dev` @ `c061062`，工作区含未提交改动，2026-10-29 核对。
- 状态只取“已支持”“部分支持”“不支持”。Lite 的已支持与部分支持项都附测试名；部分支持项写明缺口。Full 的“不支持”是在上述两个 crate 中检索后的结论，检索词写在说明里，没有做穷尽审计。
- 测试名写法：Rust 单元测试为 `crate:模块路径::测试名`（`cargo test --manifest-path <crate>/Cargo.toml --lib <名称> -- --exact`），集成测试为 `crate:tests/文件.rs::测试名`；Python 为 `文件::类.方法`，这些 Python 测试需要设置 `ARA_JSONL_TEST_BINARY` 等环境变量。
- 引用的测试在本文编写时只核对了存在性，没有在本次运行；交付门禁按需求 30.4、30.5 在同一提交上运行它们（任务 36.2）。

## 工具

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `tools.host-registered` | 宿主经 `AgentTool` 注册工具 | 已支持 | `crates/ara-agent/src/lib.rs` `tool::AgentTool`；`ara-agent:tests/live_tools.rs::standalone_tool_execution_reads_fresh_snapshot_once_without_provider_call` | 已支持 | `core:tests/host_boundary.rs::invalid_custom_names_are_rejected_before_model_state_or_tool_effects`；`harness:tests/tasks.rs::actual_file_feedback_repair_reuses_session_and_original_call_budget` | 无 |
| `tools.builtin-files` | 内置文件读取、搜索、写入与版本化编辑 | 不支持 | 两个 crate 中没有内置文件工具；`ara-tools` 不在范围内 | 已支持 | `core:local_tools::tests::versioned_snapshot_and_multi_edit_preserve_legacy_and_source_bytes`；`core:file_edits::tests::rejections_name_the_current_hash_and_the_failing_block` | 无 |
| `tools.shell-script` | Shell 与脚本执行及取消 | 不支持 | 不在两个 crate 中 | 已支持 | `core:local_tools::tests::real_shell_captures_output_and_exit`；`core:script_tools::tests::real_python_utf8_quoting_exit_cancel_timeout_cleanup` | 无 |
| `tools.web` | 联网搜索与 URL 抓取 | 不支持 | 不在两个 crate 中 | 部分支持 | `core:web_tools::tests::tool_contract_has_only_two_read_only_tools` | 只有 `web_search`、`fetch_url` 两个只读工具，没有浏览器自动化与 computer use |
| `tools.jobs-terminal` | 后台 Job 与交互终端 | 不支持 | 不在两个 crate 中 | 已支持 | `core:managed_jobs::tests::repeated_intent_never_replays_and_input_conflict_is_rejected`；`core:terminal_process::tests::terminal_is_tty_input_environment_and_resize` | 无 |
| `tools.analysis` | 源码符号与静态二进制分析 | 不支持 | 不在两个 crate 中 | 部分支持 | `core:analysis_tools::tests::parses_definitions_and_rejects_malformed_records` | 依赖本机 ctags、objdump、nm 后端；没有反编译与动态调试 |
| `tools.call-hooks` | 每次工具调用前后的宿主钩子 | 已支持 | `crates/ara-agent/src/lib.rs` `agent_loop::LoopHooks`；`ara-agent:tests/core_hooks.rs::group_g_request_boundary_one_preparation_provenance_stop_errors_and_abort` | 不支持 | 无 | Core 没有逐次调用的宿主钩子，只能注册 Run 级的 Completion_Gate |
| `tools.mcp` | MCP server 作为工具来源（Claude Code `mcpServers` 格式） | 不支持 | `ara-mcp` 不在范围内 | 部分支持 | `mcp:tests/stdio.rs::handshake_paginates_and_names_tools`；`mcp:tests/http.rs::session_headers_json_and_sse_replies`；`mcp:tests/harness_mcp.rs::mcp_tool_succeeds_inside_a_task` | 只支持 stdio 与 Streamable HTTP；`sse`、`ws`、OAuth、`headersHelper` 明确拒绝，prompts、sampling、`listChanged` 不在范围；Unix 进程路径只编译未运行 |
| `tools.unknown-effects` | 工具结果中的结构化未知副作用 | 部分支持 | `ara-agent:tests/compaction.rs::unknown_tool_effect_is_explicit_even_when_receipt_text_is_empty`。缺口：该证据只覆盖摘要输入中的未知效果标注，工具结果上的结构化字段本次未确认 | 已支持 | `core:tests/run_policies.rs::structured_unknown_details_mark_tool_end`；`core:effect_status::tests::builtin_phrases_on_the_first_line_of_effect_tool_errors_are_typed`；`harness:tests/unknown_effects.rs::unknown_iff_structured_details` | 无 |

## Session

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `session.persist-reopen` | 原生 Session 持久化与重启后重开 | 不支持 | 两个 crate 中没有 Session 持久化；`ara-session` 不在范围内 | 已支持 | `core:agent::tests::actual_tool_and_text_history_survives_checkpoint_and_restart`；`core:session_journal::tests::torn_or_unconfirmed_tail_is_preserved_and_fenced` | 无 |
| `session.fence-settlement` | 中断 Run 留下 fence，Host 结算后才能继续 | 已支持 | `crates/ara-workflow/src/driver.rs` `ReconciliationFence`；`ara-workflow:tests/independent_faults.rs::unknown_fence_is_recoverable_from_clone_and_release_restores_same_agent` | 已支持 | `core:agent::interrupted_run_property_tests::reconciliation_succeeds_only_for_the_exact_open_calls_with_evidence`；`harness:tests/resume.rs::crashed_owner_resumes_only_after_host_settlement_without_replay_or_budget_refund`；`jsonl:tests/test_resume_process.py::ResumeProcessTest.test_killed_during_a_tool_call_resumes_once_with_the_host_receipt` | 无 |
| `session.fork` | 在已提交前缀处分叉 Session | 不支持 | 检索 `fork`：0 处 | 部分支持 | `core:tests/session_fork.rs::property_14_fork_keeps_prefix_and_source`；`core:tests/session_fork.rs::run_on_one_fork_isolates_other_fork_and_source` | 有摘要归档前缀（`active_base > 0`）的 Session 拒绝分叉 |
| `session.replay-provenance` | NotExecuted 结算后重新发出的调用带重放来源且只执行一次 | 不支持 | 无 | 已支持 | `core:tests/replay_preconditions.rs::same_call_after_not_executed_runs_once_under_the_new_native_id`；`core:tests/replay_provenance.rs::replay_of_names_the_latest_matching_not_executed_call` | 无 |
| `session.steering` | 运行中由 Host 插入消息（steering） | 已支持 | `crates/ara-agent/src/lib.rs` `agent_loop::SteeringSource`；`ara-agent:tests/agent_loop.rs::owned_idle_continue_skips_the_second_steer_until_the_next_model_turn` | 部分支持 | `core:steering::tests::object_receipts_preserve_metadata_for_complete_and_finish` | 引用的测试只覆盖插话缓冲区与回执，没有引用经 `execute` 端到端注入的测试 |

## 压缩

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `compaction.summary` | 摘要压缩，保留工具回执与当前轮 | 已支持 | `crates/ara-agent/src/compaction.rs` `summarize_compaction_cut`；`ara-agent:tests/compaction_cut.rs::never_cuts_between_tool_call_receipt_and_final_assistant` | 已支持 | `core:context_memory::tests::one_large_old_turn_can_compact_and_current_stays_complete`；`jsonl:tests/test_compaction_process.py::CompactionProcessTest.test_two_summaries_three_runs_and_new_process_reopens_released_session` | 无 |
| `compaction.budget` | 摘要调用占用同一调用预算，无余量时类型化停止 | 不支持 | 未找到摘要调用的预算预留（检索 `reserv` 只命中 `preserve`） | 已支持 | `core:tests/summary_reservation.rs::summary_needed_with_no_call_left_is_a_summary_budget_stop`；`harness:tests/summary_budget.rs::task_budget_exhausted_before_summary_is_a_typed_budget_stop` | 无 |
| `compaction.refresh` | 压缩后重新读取最近使用的文件（Compaction_Refresh，需显式开启） | 不支持 | 检索 `refresh`：0 处 | 已支持 | `core:tests/compaction_refresh.rs::refresh_follows_the_summary_with_current_disk_and_session_plan_state`；`core:tests/compaction_refresh.rs::without_the_policy_no_refresh_message_or_event_appears` | 无 |
| `compaction.remote` | provider 侧远程压缩 | 已支持 | `crates/ara-agent/src/compaction.rs` `select_native_remote_compaction_cut` | 不支持 | 无 | Lite 只用自己的摘要调用压缩 |

## Goal

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `goal.mode` | Goal 模式：有界步骤与显式审查 | 不支持 | 检索 `goal`：0 处 | 已支持 | `core:execution::tests::goal_review_is_explicit_and_bounded` | 无 |
| `goal.completion-gate` | 普通停止时的宿主 Completion_Gate（需显式开启） | 部分支持 | `crates/ara-agent/src/lib.rs` `final_provider_gate::FinalProviderGate`。缺口：有最终 provider 门，同一 Run 内带来源反馈的逐次续写本次未确认 | 已支持 | `core:tests/run_policies.rs::failed_gate_continues_the_same_run_with_sourced_feedback_then_finishes`；`core:tests/completion_gate.rs::gate_runs_once_per_ordinary_stop_and_continues_only_within_budget` | 无 |
| `goal.stuck` | Stuck_Detector：重复调用或错误时先纠正，再以 NoProgress 停止（需显式开启） | 不支持 | 检索 `stuck`、`no_progress`：0 处 | 已支持 | `core:tests/stuck.rs::repeated_errors_hit_correct_then_stop_with_no_progress`；`core:tests/stuck.rs::unconfigured_detector_emits_nothing_and_injects_nothing` | 无 |

## Harness

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `harness.bounded-review` | 有界多 Run Task：Host 复核后在同一 Session 续写 | 已支持 | `crates/ara-workflow/src/types.rs` `WorkflowHost`；`ara-workflow:tests/workflow.rs::prepared_request_budget_accumulates_across_runs` | 已支持 | `harness:tests/tasks.rs::cumulative_budget_is_not_refilled_after_needs_changes`；`harness:tests/tasks.rs::wrong_uncommitted_and_unknown_receipts_stop_without_second_run` | 无 |
| `harness.length` | Length 停止：重规划一次与 Host 停止复核（需显式开启） | 已支持 | `crates/ara-workflow/src/types.rs` `LengthRecoveryPolicy`；`ara-workflow:tests/length_faults.rs::ordinary_start_never_calls_opt_in_length_settlement` | 已支持 | `core:agent::length_recovery::tests::replan_requires_explicit_execute_policy_and_original_budget`；`harness:tests/stop_review.rs::closed_length_stop_is_reviewed_and_verified_without_a_new_run` | 无 |
| `harness.budget-snapshot` | 闭合预算停止的快照复核（需显式开启） | 不支持 | `ara-workflow` 只有 `workflow prepared-request budget exhausted` 一种预算停止（`driver.rs`），没有快照复核 | 已支持 | `harness:tests/budget_snapshots.rs::actual_file_can_be_verified_without_synthesizing_a_model_stop`；`harness:tests/budget_snapshots.rs::default_start_keeps_closed_budget_stop_out_of_ordinary_review` | 无 |
| `harness.resume` | 恢复被 fence 的 Task，拒绝原因类型化 | 不支持 | `ara-workflow` 在进程内从 clone 恢复 fence；在新进程恢复被 fence 的 Task 本次未确认 | 已支持 | `harness:tests/resume.rs::resume_checked_refuses_with_typed_reasons_before_any_callback`；`harness:tests/resume.rs::out_of_order_or_partial_settlements_are_refused_and_settled_resume_executes_nothing` | 无 |
| `harness.unknown-effect-records` | `TaskOutcome` 列出未知副作用调用及回执引用 | 不支持 | 无 | 已支持 | `harness:tests/unknown_effect_list.rs::records_are_unique_ordered_and_locatable`；`harness:tests/unknown_effects_examples.rs::builtin_exec_timeout_is_unknown_through_structured_status_only` | 无 |

## JSONL

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `jsonl.sidecar` | 跨语言 sidecar 及 Python、Node 客户端 | 不支持 | 两个 crate 中没有 sidecar | 已支持 | `jsonl:tests/test_process.py::NativeProcessTest.test_python_real_file_feedback_and_same_task_budget`；`jsonl:tests/test_process.py::NativeProcessTest.test_node_real_public_host_feedback_uses_original_session_and_budget` | 无 |
| `jsonl.resume` | start 帧 `resume` 字段与 `reconcile_request`／`reconcile_reply` 帧 | 不支持 | 无 | 部分支持 | `jsonl:tests/test_jsonl_resume.py::JsonlResumeTest.test_valid_resume_settles_once_and_continues_in_the_original_task`；`jsonl:tests/test_protocol_doc_examples.py::ProtocolDocExamplesTest.test_successful_resume_sequence` | 授权改变、预算或单 Run 上限变大时只报 `start_config_rejected`／`task_admission_rejected` 且零帧，Host 无法得知命中哪一条（任务 25.2 未完成） |
| `jsonl.stop-review` | Length 停止复核帧（`verify_stop_request`、`prepare_stop_request`） | 不支持 | 无 | 已支持 | `jsonl:tests/test_length_stop_review.py::LengthStopReviewTest.test_needs_changes_continues_in_the_same_session_then_verifies`；`jsonl:tests/test_length_stop_review.py::LengthStopReviewTest.test_default_policy_sends_no_stop_frames` | 无 |
| `jsonl.unknown-effects` | task 结果中的未知副作用记录与 `receipt_ref` | 不支持 | 无 | 已支持 | `jsonl:tests/test_jsonl_unknown.py::JsonlUnknownTest.test_n_unknown_calls_give_n_records_bound_to_existing_evidence` | 无 |
| `jsonl.mcp` | start 帧 `mcp_servers` 字段与 `mcp_status` 事件 | 不支持 | 无 | 已支持 | `jsonl:tests/test_mcp_servers.py::McpServersTest.test_connected_tools_join_the_task_and_status_is_reported`；`jsonl:tests/test_mcp_servers.py::McpServersTest.test_without_the_field_frames_and_tools_are_unchanged` | 无 |
| `jsonl.bytes` | NUL 与非 BMP 文本经客户端逐字节往返 | 不支持 | 无 | 已支持 | `jsonl:tests/test_byte_roundtrip.py::ByteRoundTripTest.test_sample_round_trips_through_real_python_and_node_client_processes`；`core:tests/byte_roundtrip.rs::dr_p3_1_tool_io_leg_preserves_sample` | 无 |

## 事件

| ID | 能力 | Full | Full 依据或说明 | Lite | Lite 测试 | Lite 缺口 |
|---|---|---|---|---|---|---|
| `events.typed` | 类型化事件模型 | 已支持 | `crates/ara-agent/src/event.rs` `AgentEvent` | 已支持 | `core:tests/event_enum.rs::every_sample_line_round_trips_through_its_named_variant`；`core:tests/event_enum_errors.rs::missing_required_field_is_rejected` | 无 |
| `events.versioned-json` | 版本化 JSON 事件：`EVENT_SCHEMA_VERSION`，保留未知类型与字段 | 不支持 | `AgentEvent` 没有 `Deserialize` 实现（检索 0 处），也未找到 schema 版本常量 | 已支持 | `core:tests/event_enum.rs::an_unknown_type_keeps_the_complete_original_json`；`core:tests/event_compat.rs::value_execute_matches_the_golden_event_shapes` | 无 |
| `events.policy-events` | 输出上限、Completion_Gate、Stuck_Detector、Compaction_Refresh 的事件 | 不支持 | 无 | 已支持 | `core:tests/event_compat.rs::every_event_type_has_samples_and_a_named_variant`；`core:tests/output_limit_wire.rs::provider_receives_the_recorded_effective_limit` | 无 |

## 未覆盖

- 本稿已列入截至 `c061062` 的 P1–P4 新增能力（resume、停止复核、未知副作用、Session 分叉、Completion_Gate、Stuck_Detector、Compaction_Refresh、输出上限、事件枚举、MCP）。任务 57.2 定稿时复核并补入之后新增的能力，同步 JSON。
- 门禁按矩阵运行引用测试（任务 36.2）与门禁结论属性测试（任务 36.3）在 `tools/lite-eval` 中实现，尚未接入。
