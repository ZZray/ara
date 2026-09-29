# Port map: Go `ara-lite` to Rust

Go source: old ARA repository, `tools/ara-lite/` (read-only reference, not part of this repository).
Rust crate: `tools/ara-lite/` (library `ara_lite`, binary `ara-lite`).
Rule from `AGENTS.md`: an item counts as ported only when a Rust test exercises the behavior. The last column of
every table names that test (or says why there is none).

Legend: **ported** = same behavior; **partial** = ported with the differences listed in "Differences";
**not ported** = deliberately left out.

## 1. Go file and function to Rust module and function

### `core.go` (state machine) -> `src/store/mod.rs`

| Go | Rust | Status |
| --- | --- | --- |
| `OpenStore` | `store::Store::open` | ported |
| `Store.Close` | `Store::close` (also on drop) | ported |
| `Store.Changes` (channel) | `Store::epoch` + `Store::wait_change` (Condvar) | partial (mechanism differs) |
| `Store.Tick` | `Store::tick` | ported |
| `randomID` | `util::random_hex` | ported |
| `cloneDisk` | `Draft::from_inner` | ported |
| `Store.save` | `Inner::save` (one SQLite transaction, then publish in memory) | ported |
| `addEvent` | `Draft::add_event` | ported |
| `masterID` | `master_id` | ported |
| `Store.expire` | `Inner::expire` | ported |
| `cleanState`, `Store.Snapshot`, `Store.visibleState` | `Inner::visible_clients/tasks/events/state`, `Store::snapshot` | ported |
| `Store.JoinKeys` | `Store::join_keys` | ported |
| `tokenEqual` | `util::token_equal` (constant time, rejects empty) | ported |
| `canonicalWorkspace`, `sameWorkspace`, `exactWorkspace` | `canonical_workspace`, `same_workspace`, `exact_workspace` | ported |
| `replayResult` | `Inner::replay_result` | ported |
| `Store.Execute` (join, create, claim, progress, heartbeat, submit, review, confirm-stop, clear-tasks, read ops) | `Store::execute` -> `Inner::execute` | partial (round 2: section 3 items 15, 16) |
| `lock_unix.go` / `lock_windows.go` `lockDirectory` | `lock::DirLock::acquire` (fs4: `flock` / `LockFileEx`) | ported (Unix untested) |
| `replaceStateFile` | not needed: state is SQLite; profile files use `profile::save_private` | n/a |

### `store_sqlite.go` -> `src/store/db.rs`, `src/store/mod.rs`

| Go | Rust | Status |
| --- | --- | --- |
| `openStateDB` | `db::open_state_db` (temp file + rename on first use; an unloadable file is never overwritten) | ported |
| `newState`, `validateState` | inside `open_state_db` / `load_state` (format, cursor, join keys, id checks) | partial |
| `openSQLite` | `db::open_sqlite` (`journal_mode=DELETE`, `synchronous=FULL`) | ported |
| `createSchema` | `db::create_schema` (new schema, see Differences) | partial |
| `persistState` | `Inner::save` | ported |
| `upsertJSON` | `store::upsert_json` | ported |
| `loadState` | `db::load_state` | ported |

### `message_store.go` -> `src/store/messages.rs`, `src/store/db.rs`

| Go | Rust | Status |
| --- | --- | --- |
| `ensureMessageSchema` | `db::ensure_message_schema` | ported |
| `insertMessageTx` | `messages::insert_message` | ported |
| `persistMessageDraftsTx` | inside `Inner::save` | ported |
| `sanitizeLegacyMessages` | none | not ported (no legacy data in a new database) |
| `pruneMessages` | `Inner::prune_messages` | ported |
| `authenticate` | `Inner::authenticate` | ported |
| `scanMessage` | `messages::scan_message` | ported |
| `SendChat`, `PollMessages`, `AckMessages`, `MessageHistory` | `Inner::send_chat/poll_messages/ack_messages/message_history` (public wrappers on `Store`) | ported |
| `equalBytes` | `util::equal_bytes` | ported |
| `recentMessages` | `Inner::recent_messages` | ported |

### `main.go` -> `src/cli.rs`, `serve.rs`, `server.rs`, `client.rs`, `profile.rs`, `snapshot.rs`, `help.rs`

| Go | Rust | Status |
| --- | --- | --- |
| `main`, `run` | `main.rs`, `cli::run` | ported |
| `configDir` | `profile::config_dir` | partial (own directory `ara-lite-rs`: section 3 item 4) |
| `apiHandler` | `server::route`, `handle_api`, `dispatch` (Origin, JSON, auth, timeout order kept) | ported |
| `serve` | `serve::serve`, `server::Server` (plain text status; no TUI) | partial |
| `shellQuote`, `joinCommands` | `serve::quote_posix/quote_powershell`, `join_commands` | ported |
| `writePrivate`, `savePrivate` | `profile::save_private` (exclusive create for `join`) | ported (Unix modes untested) |
| `readConnection` | `profile::read_connection`, `loopback_addr` | ported |
| `call`, `callWait`, `callWaitContext` | `client::call`, `call_wait` (plain `std::net`, no proxy, no redirects) | ported |
| `clientCommand` | `cli::client_command`, `build_request` | ported |
| `printResult` | `cli::print_result`, `util::indent_json` | ported |
| `gitSnapshot`, `gitSnapshotOnce` | `snapshot::git_snapshot`, `snapshot_once` | partial (hash input differs) |
| `helpText` | `help::HELP_TEXT` (English, rewritten) | partial |
| `promptHelpText` | `help::MASTER_PROMPT`, `WORKER_PROMPT` (rewritten, see README) | not ported by design |

### `mcp.go` -> `src/mcp.rs`

| Go | Rust | Status |
| --- | --- | --- |
| `newAraLiteMCPServer` (5 tools, Go MCP SDK) | `mcp::catalog` (14 tools) + `call_tool`, hand-written JSON-RPC | partial (extended) |
| `serveMCP` | `mcp::serve_mcp` | ported |

### `ui.go` -> none

The bubbletea console is **not ported** (deferred). `serve --plain` is accepted, and plain text is the only output.

## 2. Go tests to Rust tests

Unit tests of the store live in `src/store/tests.rs` (all names below are in module `store::tests`).
Real-process and HTTP tests live in `tests/`.

### `core_test.go`

| Go test | Rust test |
| --- | --- |
| `TestCoreMultiWorkerReviewAndFencing` | `multi_worker_review_and_fencing` |
| `TestCoreExpiryRestartAndConfirmedStop` | `expiry_restart_and_confirmed_stop` |
| `TestCorePermissionsIdempotenceAndSnapshots` | `permissions_idempotence_and_snapshots` |
| `TestCoreOverlappingWorkspacesAndReadMode` | `overlapping_workspaces_and_read_mode` |
| `TestCorePersistenceFailureAndNotifications` | `persistence_failure_and_notifications` |
| `TestCoreMessageHistoryOutlivesMonitorWindow` | `message_history_outlives_monitor_window` |
| `TestCoreLockAndCorruptState` | `lock_and_corrupt_state` |
| `TestCoreCrashReleasesOSLock` + `TestCoreLockHelper` | `tests/serve.rs::crash_releases_the_data_directory_lock` (real `serve` process killed, then the lock is free) |
| `TestCoreHistoryPersistsAcrossRestart` | `history_persists_across_restart` |
| `TestCoreReadContactIsVisibleButDoesNotRewriteDurableState` | `read_contact_is_visible_but_does_not_rewrite_durable_state` |
| `TestCoreNewSQLiteStateLeavesOldJSONUnused` | `new_state_db_leaves_unrelated_files_untouched` |
| `TestCoreTickDurabilityAndWake` | `tick_durability_and_wake` |

### `message_store_test.go`

| Go test | Rust test |
| --- | --- |
| `TestMessageDirectedBroadcastHistoryAndAck` | `directed_broadcast_history_and_ack` |
| `TestMessageTaskPrivacyProgressAndRestart` | `task_privacy_progress_and_restart` |
| `TestProgressResetsAtReworkAndRequeue` | `progress_resets_at_rework_and_requeue` |
| `TestMessageExpiryGapAndLegacyBodyCleanup` | `message_expiry_gap_and_receipt_cleanup` (the legacy-body half is skipped: not ported) |

### `clear_tasks_test.go`

| Go test | Rust test |
| --- | --- |
| `TestClearTasksArchivesExactWorkspaceAndAllowsNewWork` | `clear_tasks_archives_exact_workspace_and_allows_new_work` |
| `TestClearTasksRejectsActiveWorkAtomically` | `clear_tasks_rejects_active_work_atomically` |
| `TestClientClearTasksCommand` | `tests/cli.rs::cli_clear_tasks_command` |

### `main_test.go`

| Go test | Rust test |
| --- | --- |
| `TestHTTPPollWakesOnlyForDirectedDurableMessage` | `tests/http.rs::poll_wakes_only_for_a_directed_durable_message` |
| `TestHTTPPollIncludesTaskIDs` | `tests/http.rs::poll_includes_task_ids` |
| `TestHTTPRejectsBrowserAndExtraJSON` | `tests/http.rs::rejects_browser_origin_and_extra_json` |
| `TestGitSnapshotIncludesUntrackedIndexAndDeletion` | `tests/cli.rs::git_snapshot_includes_untracked_index_and_deletion` |
| `TestProfileAndJoinHelpDoNotExposeToken` | `tests/cli.rs::profile_and_join_help_do_not_expose_the_token`, `tests/serve.rs::plain_status_prints_join_commands_and_never_a_credential` |
| `TestJoinHelpChoosesUnusedProfiles` | `serve::tests::join_commands_pick_free_profile_names_and_quote` |
| `TestJoinHelpKeepsPersistedMasterNameAcrossRestart` | `tests/cli.rs::join_help_keeps_the_persisted_master_name_across_restart`, `serve::tests::join_commands_reuse_the_existing_master_name_and_escape_quotes` |
| `TestJoinHelpDoesNotRenderControlCharactersInMasterName` | `serve::tests::unprintable_master_names_get_no_command` |
| `TestCLIProcess` (helper) | `tests/common/mod.rs::cli`, `cli_ok`, `cli_err` |
| `TestCLIJoinSubmitReviewAndSnapshotGate` | `tests/cli.rs::cli_join_submit_review_and_snapshot_gate` |
| `TestProfilePublicationNeverOverwritesExisting` | `profile::tests::save_private_refuses_to_replace_when_exclusive` |

### `mcp_test.go`

| Go test | Rust test |
| --- | --- |
| `TestMCPToolCatalogAndPollProtocol` | `tests/mcp.rs::tool_catalog_covers_the_lifecycle_and_never_contains_the_credential`, `poll_forwards_defaults_and_passes_large_integers_through`, `initialize_negotiates_a_supported_protocol_version` |
| `TestMCPMutatingToolsForwardExactArguments` | `tests/mcp.rs::mutating_tools_forward_exact_arguments`, `mutation_without_a_request_id_gets_a_generated_one` |
| `TestMCPPollCancellationClosesHTTPRequest` | `tests/mcp.rs::poll_cancellation_closes_the_http_request_and_sends_no_response` |

### `ui_test.go`

All ten `TestConsole*` tests: **skipped** (TUI not ported).

### New Rust tests without a Go counterpart

| Rust test | What it pins |
| --- | --- |
| `store::tests::wire_notes_are_the_go_service_values` | the two Chinese `progress_note` wire values |
| `tests/http.rs::poll_without_messages_waits_then_returns_an_empty_page` | poll timeout returns an empty page |
| `tests/http.rs::request_shape_and_credential_checks` | check order: JSON shape, bearer token |
| `tests/http.rs::health_routes_methods_and_body_limits` | `/health`, 404/405, 1 MiB cap, chunked request refused |
| `tests/http.rs::a_join_with_the_wrong_role_key_is_refused` | join keys are role specific |
| `tests/cli.rs::cli_usage_errors_are_a_single_actionable_line`, `cli_body_file_and_retry_ids`, `help_lists_the_commands_and_both_prompts`, `git_snapshot_rejects_non_repositories` | CLI ergonomics |
| `tests/serve.rs::json_readiness_line_matches_the_join_files`, `listen_address_and_flag_errors_are_reported`, `state_survives_an_abrupt_kill` | `serve` process behavior |
| `tests/mcp.rs`: `invalid_input_is_a_tool_error_and_protocol_mistakes_are_rpc_errors`, `ending_stdin_stops_the_server_and_abandons_polls`, `an_unreachable_service_names_the_retry_key`, `profile_problems_fail_before_serving` | MCP protocol edge cases |
| `tests/e2e.rs::cli_lifecycle_against_a_real_service` | create -> claim -> progress -> heartbeat -> submit -> review(pass) through the CLI, plus stale-version rejection and an empty poll timeout |
| `tests/e2e.rs::mcp_lifecycle_against_a_real_service` | the same lifecycle through `ara-lite mcp` |
| `tests/e2e.rs::lease_expiry_makes_a_running_task_unconfirmed_and_only_confirm_stop_requeues_it` | lease expiry -> unconfirmed -> confirm-stop |
| `tests/e2e.rs::a_service_restart_turns_running_work_into_unconfirmed_and_keeps_it_recoverable` | killed and restarted real service |
| `src/util.rs`, `src/cli.rs`, `src/mcp.rs` unit tests | Go duration forms, query decoding, flag defaults, catalog shape, result pass-through |
| `tests/docs.rs` | README prompts equal `src/help.rs`; docs exist |
| `store::tests::owner_progress_renews_the_lease`, `store::tests::claim_may_omit_version_and_generation` | round 2 store behavior (section 3 items 15, 16) |
| `tests/e2e.rs::claim_without_version_or_generation_over_cli_and_mcp`, `tests/mcp.rs::poll_without_a_timeout_waits_five_seconds` | round 2 CLI and MCP behavior (section 3 items 16, 17) |
| `profile::tests::default_data_dir_is_not_the_go_tools_directory` | the default data directory is not Go's (section 3 item 4) |

## 3. Differences from the Go tool

1. **Wire strings "已领取" / "需返工".** Go writes these Chinese `progress_note` values on claim and on
   changes_requested, and its tests assert them (`message_store_test.go` lines 137, 259, 276). They are wire data, so the Rust
   port keeps them byte for byte. This is the only exception to the rule that strings in code are English. The source spells
   them as `\u{...}` escapes (`NOTE_CLAIMED`, `NOTE_REWORK` in `store/mod.rs`) and `wire_notes_are_the_go_service_values` pins them.
2. **Go test defect (clock).** `newMessageFixture` in `message_store_test.go` (line 27) pins the store clock to
   2026-09-24 12:00 UTC. A restarted store prunes messages with the real clock, so tests that restart it
   (`TestMessageTaskPrivacyProgressAndRestart`) fail once 24 hours have passed since that date. The Rust message tests start
   their test clock at `Utc::now()` (`message_clock` in `store/tests.rs`). Not fixed in the Go repository (read-only).
3. **TUI not ported.** No console; `--plain` is accepted and changes nothing. See README, deferred work.
4. **Separate default data directory.** The Rust default is `<user config dir>/ara-lite-rs` (fallback `.ara-lite-rs`),
   not Go's `<user config dir>/ara-lite`. A default Rust `serve` therefore never opens the live Go `state.db`; this matters
   because `open_state_db` writes its schema before `load_state` validates the file. Nothing is migrated. Whether the
   two `state.db` files are compatible is not tested and not claimed. `sanitizeLegacyMessages` has no counterpart.
   Test: `profile::tests::default_data_dir_is_not_the_go_tools_directory`.
5. **JSON keys are case-sensitive** (Go's decoder also accepted `Op` for `op`). Unknown fields are still rejected (400).
6. **HTTP server limits.** Thread per connection, 256 handlers at most; chunked request bodies get 411 (Go's `net/http` accepted them).
   The client has no redirect or proxy code at all, so a credential cannot leave the loopback address (Go sets `Proxy: nil` and
   `ErrUseLastResponse` explicitly in `callWaitContext`).
7. **Stopped-poll detection is polling based** (peek on the socket) instead of Go's request context.
8. **No signal handling.** Ctrl+C ends the process. State is crash safe (`journal_mode=DELETE`, `synchronous=FULL`, one transaction per
   operation), and the directory lock is released by the OS.
9. **Git snapshot.** Same format `<HEAD sha>:<branch>:<sha256 hex>` and same sensitivity (untracked files, index, deletions, file
   mode); the bytes hashed differ, so a Go snapshot and a Rust snapshot of the same tree are not equal.
10. **MCP.** 14 tools (Go: 5); `request_id` optional and auto-generated; read-only and idempotent hints; own JSON-RPC loop instead of the
    Go SDK; results are passed through as the service's raw JSON, so integers above 2^53 stay exact. `history` is CLI only.
11. **CLI flags.** Same names. Go single-dash flags (`-profile`) and `--flag=false` booleans are not accepted. Usage errors are one line.
    Help and prompts are English and rewritten (README lists the defects of the Go prompts).
12. **`join` help.** The message for an unsafe master name is worded differently; the behavior (no command is printed) is the same.
13. **`serve --json`** prints one readiness line with keys `status`, `url`, `data_dir`, `master_join_file`, `worker_join_file`.
14. **Unix-only code** (`#[cfg(unix)]` in `snapshot.rs` and `profile.rs`) is kept minimal and was never compiled here.
15. **Owner progress renews the lease** (round 2, simplification proposal 1). Go's `progress` never touches `lease_until`;
    the Rust `"progress"` arm sets it to now + 120 s, as `heartbeat` does. `progress` on an `unconfirmed` task still
    fails. Tests: `store::tests::owner_progress_renews_the_lease`; `tests/e2e.rs::cli_lifecycle_against_a_real_service`
    (its old "progress does not renew the lease" assertion now asserts the renewal).
16. **Claim fencing is optional** (round 2, simplification proposal 3). `Request.version` and `generation` are
    `Option<i64>`, absent when not sent (generation 0 is a real value, so 0 cannot mean "absent"). Only `claim` lets an
    absent value pass; a present value is checked with the same errors as Go, and every other operation rejects an
    absent value exactly like a wrong one. CLI `claim` flags and MCP `claim_task` fields are optional. Tests:
    `store::tests::claim_may_omit_version_and_generation`, `tests/e2e.rs::claim_without_version_or_generation_over_cli_and_mcp`.
17. **MCP poll default 5 s** (round 2, simplification proposal 10; Go MCP: 25 s). The HTTP API (30 s without `timeout`)
    and CLI (25 s) defaults are unchanged. Tests: `tests/mcp.rs::poll_without_a_timeout_waits_five_seconds`,
    `tests/e2e.rs::mcp_lifecycle_against_a_real_service`.
