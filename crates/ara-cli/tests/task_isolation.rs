//! Central G1/G2/G4/G5 private actual-Git/resource banks; no user repository is mutated.
//! Authored assertions are not executed evidence. G3 ordinary Native/ChildHost bank lives with binary tests.
use ara_cli::host_peers;
#[path = "support/task_isolation_cases.rs"]
mod cases;
#[path = "../src/task_isolation/mod.rs"]
mod task_isolation;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g1_backend_policy_baseline_budget_and_linked_detachment() {
    cases::primitives().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g2_real_git_binary_nested_history_and_parent_wip() {
    cases::git_capture().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g4_exact_resource_admission_release_and_lost_observer() {
    cases::ownership().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g5_artifact_ref_and_unknown_close_retention() {
    cases::recovery().await;
}
