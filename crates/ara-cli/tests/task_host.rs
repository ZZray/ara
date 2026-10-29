//! G3 ordinary executable + Native Task + actual ChildHost/Git/files/journals.
//! Controlled HTTP model responses are not real-model acceptance.
#![cfg(feature = "test-fixture")]
#[path = "support/task_host_cases.rs"]
mod cases;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g3_native_task_isolation_park_warm_release_bank() {
    cases::isolated_lifecycle().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g3_native_async_batch_and_actual_wait_bank() {
    cases::async_batch().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g3_native_task_print_line_rpc_acp_entry_bank() {
    cases::ordinary_entries().await;
}
