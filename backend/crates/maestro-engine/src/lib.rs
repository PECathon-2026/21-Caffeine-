//! Maestro engine: plan validation, policy, scopes, sandbox (bubblewrap),
//! git worktrees, context compilation, prompts, and the repository tool bus.
//!
//! Everything in this crate is the deterministic "static half" of the engine:
//! pure functions and side-effect-scoped helpers the orchestrator drives.
//! The orchestrator itself lives in `orchestrator.rs` (added separately).

pub mod browser;
pub mod config;
pub mod context;
pub mod credentials;
pub mod git;
pub mod history;
pub mod integration;
mod langgraph;
pub mod media;
pub mod orchestrator;
pub mod plan;
pub mod policy;
pub mod prompts;
pub mod sandbox;
pub mod scopes;
pub mod tools;
pub mod worktrees;

pub use config::{
    effective_rules, init_config, init_state_only, load_config, profile_rules, save_config,
    state_dir, validate_config,
};
pub use context::{compact_transcript, load_project_instructions, TranscriptEntry, TranscriptKind};
pub use credentials::{
    inject_env, load_credentials, save_credential, save_global_credential, CredentialsFile,
};
pub use git::MergeOutcome;
pub use orchestrator::{looks_like_implementation_request, FinalizeOutcome, Orchestrator};
pub use plan::validate_plan;
pub use policy::PolicyEngine;
pub use sandbox::{run_check, CheckResult, SandboxSpec};
pub use scopes::{compile_scopes, path_in_scopes, scopes_overlap};
pub use tools::{
    deny_ask_decider, registry_for_role, safe_path, DelegateRequest, PolicyDecider, ToolCtx,
    ToolExec, ToolOutcome,
};
pub use worktrees::{TaskWorkspace, WorktreeManager};

#[cfg(test)]
pub(crate) mod test_env {
    use std::sync::{Mutex, MutexGuard};
    static LOCK: Mutex<()> = Mutex::new(());
    /// Serializes tests that mutate process-global env (MAESTRO_CONFIG_DIR).
    pub fn lock() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}
