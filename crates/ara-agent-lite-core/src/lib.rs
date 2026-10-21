//! Compact embeddable Agent runtime. Hosts own identity, authorization, paths and scheduling.
pub mod agent;
pub mod context_memory;
pub mod environment_library;
pub mod environment_tools;
pub mod execution;
pub mod experience;
pub mod file_discovery;
pub mod goal_runtime;
pub mod local_tools;
pub mod managed_jobs;
pub mod model;
pub mod model_caps;
pub mod model_telemetry;
pub mod project_rules;
pub mod script_tools;
pub mod session_journal;
pub mod skill_library;
pub mod steering;
pub mod system_capabilities;
pub mod terminal_emulator;
pub mod terminal_process;
pub mod web_tools;

pub use agent::{execute, RunRequest, RunResult, RuntimePaths, ToolRegistration};
pub use execution::{GoalState, RunMode};
pub use rpi_agent;
pub use rpi_ai;

pub fn fresh_id() -> String {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("OS random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
