//! The mission orchestrator: planning, wake-driven scheduling, leased agent
//! turn loops, delegation, budgets, loop detection, and provider fallbacks.
//!
//! The deterministic runtime owns every state transition. Models propose,
//! use granted tools, and return evidence — nothing else. See SPEC.md
//! §Orchestrator for the behavior contract; `integration.rs` holds the
//! merge-queue and finalization half.

use crate::context::{compact_transcript, load_project_instructions, TranscriptEntry};
use crate::policy::PolicyEngine;
use crate::prompts;
use crate::tools::{registry_for_role, PolicyDecider, ToolCtx, ToolExec, ToolOutcome};
use crate::worktrees::{TaskWorkspace, WorktreeManager};
use crate::{config::effective_rules, git, scopes};
use maestro_providers::ProviderRegistry;
use maestro_store::{ApprovalRequest, Store};
use maestro_types::config::{MaestroConfig, ModelRef, RoleAssignment};
use maestro_types::domain::{
    ApprovalStatus, Invocation, InvocationStatus, Mission, MissionStatus, RiskLevel, Role, Task,
    TaskStatus,
};
use maestro_types::event::kind;
use maestro_types::plan::{Plan, PlanTask};
use maestro_types::protocol::{parse_agent_turn, AgentTurn, FinalReport, FinalStatus};
use maestro_types::provider::{ModelMessage, ModelRequest, ModelResponse};
use maestro_types::{new_id, now_ms, MaestroError, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Result of [`Orchestrator::finalize_mission`].
#[derive(Debug, Clone)]
pub struct FinalizeOutcome {
    /// The commit the user branch now points at.
    pub commit: String,
    /// The mission integration branch that was merged.
    pub branch: String,
}

pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) config: MaestroConfig,
    pub(crate) registry: Arc<ProviderRegistry>,
    pub(crate) root: PathBuf,
    pub(crate) generation: String,
    pub(crate) worktrees: WorktreeManager,
    /// Wakes the scheduler on task completion / delegation / steering.
    pub(crate) notify: Notify,
    /// Restores exclude repository tools, finalization and new requests.
    pub(crate) workspace_gate: tokio::sync::RwLock<()>,
    pub(crate) history_lock: tokio::sync::Mutex<()>,
}

/// Drives missions end to end. Cheap to clone (all state behind an `Arc`).
#[derive(Clone)]
pub struct Orchestrator {
    pub(crate) inner: Arc<Inner>,
}

/// Heartbeat/ticker task owned by a scope; aborted on drop so no task leaks.
struct OwnedTask(tokio::task::JoinHandle<()>);

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What one spawned worker was doing (for batch conflict checks).
struct ActiveInfo {
    task_id: String,
    /// Workspace a reader shares with its coder dependency, if any.
    reader_workspace: Option<PathBuf>,
    /// Declared write scopes when the task is a coder.
    coder_scopes: Vec<String>,
}

/// Parse an architect tool call: a reply that begins with a JSON object
/// (optionally ```json-fenced) carrying a string `tool` and object `args`.
/// Models sometimes keep writing past the call — simulating tool results and
/// further turns — so only the leading object counts and the rest is dropped;
/// the loop feeds the real result back. A reply with no leading JSON object
/// is prose — the final answer.
fn architect_tool_call(text: &str) -> Option<(String, serde_json::Value)> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|rest| rest.trim_start_matches('\n').trim_end_matches('`').trim())
        .unwrap_or(trimmed);
    if !body.starts_with('{') {
        return None;
    }
    let value = serde_json::Deserializer::from_str(body)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?;
    let tool = value.get("tool")?.as_str()?.to_string();
    let args = value.get("args").cloned().unwrap_or(serde_json::json!({}));
    Some((tool, args))
}

/// True when `s` begins with `word` followed by a word boundary.
fn starts_with_word(s: &str, word: &str) -> bool {
    s.strip_prefix(word)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
}

/// Heuristic used by the TUI/CLI to route plain chat text. Work is the
/// default: plain messages launch missions unless they are informational
/// questions or small talk, which go to the architect (who can read the
/// repository itself).
pub fn looks_like_implementation_request(text: &str) -> bool {
    let lowered = text.trim().to_lowercase();
    let mut s = lowered.trim_end_matches(['?', '!', '.', ' ']);
    // Peel conversational wrappers repeatedly, including wrappers after a
    // modal: "now, can you please explain" and "can you now explain".
    loop {
        let opener = [
            "hey",
            "hi",
            "ok",
            "okay",
            "so",
            "please",
            "maestro",
            "now",
            "also",
            "next",
            "then",
            "first",
            "firstly",
            "for now",
            "can you",
            "could you",
            "would you",
            "will you",
            "i want you to",
            "i would like you to",
        ]
        .into_iter()
        .find(|opener| starts_with_word(s, opener));
        let Some(opener) = opener else {
            break;
        };
        s = s[opener.len()..].trim_start_matches([',', ':', ' ', '\n']);
    }
    if s.is_empty() {
        return false;
    }
    // Acknowledgments and greetings never start work.
    const SMALL_TALK: [&str; 18] = [
        "thanks",
        "thank you",
        "thx",
        "ty",
        "cool",
        "nice",
        "great",
        "awesome",
        "hello",
        "yo",
        "sup",
        "lol",
        "wow",
        "good job",
        "well done",
        "yes",
        "yep",
        "no",
    ];
    if SMALL_TALK.contains(&s) {
        return false;
    }
    // Saving a code checkpoint is a conversational tool action. Compound
    // requests still go through planning so requested edits are not dropped.
    let backup = ["backup", "back up", "checkpoint", "snapshot", "version"]
        .iter()
        .any(|w| s.contains(w));
    let save_action = [
        "save",
        "back up",
        "backup",
        "make a backup",
        "create a backup",
        "take a snapshot",
        "create a snapshot",
        "create a checkpoint",
    ]
    .iter()
    .any(|w| starts_with_word(s, w));
    let build_backup_feature = ["create", "make", "build", "implement", "add"]
        .iter()
        .any(|w| starts_with_word(s, w))
        && ["tool", "feature", "system", "script", "endpoint", "test"]
            .iter()
            .any(|w| s.split(|c: char| !c.is_alphanumeric()).any(|t| t == *w));
    let compound_change =
        [" and ", " then ", "; ", " before ", " after "]
            .iter()
            .any(|separator| {
                s.split(separator).skip(1).any(|clause| {
                    let clause = clause
                        .trim()
                        .strip_prefix("please ")
                        .unwrap_or(clause.trim());
                    [
                        "fix",
                        "change",
                        "edit",
                        "add",
                        "implement",
                        "refactor",
                        "build",
                        "update",
                        "remove",
                        "delete",
                        "write",
                        "create",
                        "run",
                        "test",
                        "redesign",
                        "redesigning",
                        "changing",
                        "editing",
                        "refactoring",
                        "updating",
                        "modifying",
                        "implementing",
                        "removing",
                        "deleting",
                    ]
                    .iter()
                    .any(|w| starts_with_word(clause, w))
                })
            });
    if backup && save_action && !build_backup_feature && !compound_change {
        return false;
    }
    // Informational asks go to the architect. Bare "do" stays out of this
    // list so imperatives like "do the migration" still launch missions.
    const QUESTION_STARTS: [&str; 24] = [
        "what",
        "why",
        "how",
        "where",
        "when",
        "who",
        "which",
        "whose",
        "is",
        "are",
        "was",
        "were",
        "does",
        "did",
        "do you",
        "do we",
        "should",
        "explain",
        "describe",
        "summarize",
        "summarise",
        "tell me",
        "show me",
        "walk me through",
    ];
    const INSPECTION_STARTS: &[&str] = &[
        "look into",
        "look at",
        "look through",
        "go through",
        "read",
        "inspect",
        "review",
        "analyze",
        "analyse",
        "explore",
        "understand",
        "help me understand",
        "give me an overview",
    ];
    let informational = QUESTION_STARTS
        .iter()
        .chain(INSPECTION_STARTS.iter())
        .any(|prefix| starts_with_word(s, prefix));
    if !informational {
        return true;
    }
    // An inspection followed by an explicit change still needs a mission.
    // Match clause starts, not words inside an explanation such as "how to add".
    const ACTIONS: &[&str] = &[
        "fix",
        "change",
        "edit",
        "add",
        "implement",
        "refactor",
        "build",
        "update",
        "remove",
        "delete",
        "write",
        "create",
        "run",
        "test",
    ];
    [" and ", " then ", "; "].iter().any(|separator| {
        s.split(separator).skip(1).any(|clause| {
            let clause = clause
                .trim()
                .strip_prefix("please ")
                .unwrap_or(clause.trim());
            ACTIONS
                .iter()
                .any(|action| starts_with_word(clause, action))
        })
    })
}

impl Orchestrator {
    pub fn new(
        store: Store,
        config: MaestroConfig,
        registry: Arc<ProviderRegistry>,
        root: PathBuf,
        generation: String,
    ) -> Self {
        let worktrees = WorktreeManager::new(root.clone(), config.safety.worktrees);
        Orchestrator {
            inner: Arc::new(Inner {
                store,
                config,
                registry,
                root,
                generation,
                worktrees,
                notify: Notify::new(),
                workspace_gate: tokio::sync::RwLock::new(()),
                history_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn config(&self) -> &MaestroConfig {
        &self.inner.config
    }

    /// Wake the scheduler (steering, external approvals, retries).
    ///
    /// Uses `notify_waiters` so every parked mission scheduler wakes:
    /// `notify_one` would hand the single permit to an arbitrary mission and
    /// leave the intended one parked. `notify_waiters` stores no permit for
    /// future waiters, so a poke landing between a scheduler's state check
    /// and its `notified().await` is lost — that is safe here because the
    /// execute loop re-lists tasks on every iteration and always has the
    /// bounded fallback park (≤1s) to pick up missed wakeups.
    pub fn poke(&self) {
        self.inner.notify.notify_waiters();
    }

    /// Create the mission row (status `planning`) and its creation event.
    pub async fn create_mission(&self, objective: &str) -> Result<Mission> {
        let _workspace = self.inner.workspace_gate.read().await;
        self.ensure_history_recovered().await?;
        // Missions need a repository with a commit for worktrees, baselines,
        // and integration; refuse up front instead of creating a doomed
        // mission that fails on the first git call.
        if git::head(&self.inner.root).await.is_err() {
            return Err(MaestroError::Config(format!(
                "{} is not a git repository with a commit, which missions require. \
                 Launch maestro inside a repository (this folder's sub-repos count), \
                 or run `git init` and make an initial commit here first.",
                self.inner.root.display()
            )));
        }
        let mission = self.inner.store.create_mission(objective)?;
        self.capture_version(
            &self.inner.root,
            Some(&mission.id),
            None,
            "before",
            &format!("Before: {objective}"),
            true,
        )
        .await?;
        self.inner.store.append_event(
            &mission.id,
            None,
            kind::MISSION_CREATED,
            serde_json::json!({ "objective": objective }),
        )?;
        Ok(mission)
    }

    fn set_mission_status(
        &self,
        mission_id: &str,
        status: MissionStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        self.inner
            .store
            .set_mission_status(mission_id, status, reason)?;
        self.inner.store.append_event(
            mission_id,
            None,
            kind::MISSION_STATUS,
            serde_json::json!({ "status": status.as_str(), "reason": reason }),
        )?;
        Ok(())
    }

    pub(crate) fn assignment(&self, role: Role) -> Result<&RoleAssignment> {
        self.inner
            .config
            .roles
            .get(role.as_str())
            .ok_or_else(|| MaestroError::Config(format!("no model assigned to role {role}")))
    }

    /// Candidate models: an explicit per-task override first, then the role's
    /// primary and fallbacks.
    pub(crate) fn candidates(
        &self,
        role: Role,
        override_ref: Option<&str>,
    ) -> Result<Vec<ModelRef>> {
        let assignment = self.assignment(role)?;
        let mut out = Vec::with_capacity(1 + assignment.fallbacks.len() + 1);
        if let Some(raw) = override_ref {
            let parsed = ModelRef::parse(raw)
                .ok_or_else(|| MaestroError::Config(format!("invalid model override {raw:?}")))?;
            out.push(parsed);
        }
        out.push(assignment.model.clone());
        out.extend(assignment.fallbacks.iter().cloned());
        Ok(out)
    }

    pub(crate) fn check_budget(&self, mission_id: &str) -> Result<()> {
        let runtime = &self.inner.config.runtime;
        if runtime.max_total_tokens <= 0 && runtime.max_cost_micro_usd <= 0 {
            return Ok(());
        }
        let stats = self.inner.store.mission_stats(mission_id)?;
        let over_tokens =
            runtime.max_total_tokens > 0 && stats.total_tokens() >= runtime.max_total_tokens;
        let over_cost =
            runtime.max_cost_micro_usd > 0 && stats.cost_micro_usd >= runtime.max_cost_micro_usd;
        if over_tokens || over_cost {
            let what = if over_tokens { "token" } else { "cost" };
            self.inner.store.append_event(
                mission_id,
                None,
                kind::BUDGET_EXHAUSTED,
                serde_json::json!({
                    "budget": what,
                    "totalTokens": stats.total_tokens(),
                    "costMicroUsd": stats.cost_micro_usd,
                }),
            )?;
            return Err(MaestroError::BudgetExhausted(format!(
                "mission {what} budget exhausted ({} tokens, {} µ$ spent)",
                stats.total_tokens(),
                stats.cost_micro_usd
            )));
        }
        Ok(())
    }

    /// Budgeted, fallback-chained, retry-bounded provider call. Every attempt
    /// is recorded as an invocation row with tokens, cost, and latency.
    async fn invoke(
        &self,
        mission_id: &str,
        task: Option<&Task>,
        role: Role,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<ModelResponse> {
        self.check_budget(mission_id)?;
        let override_ref = task.and_then(|t| t.model_override.as_deref());
        let candidates = self.candidates(role, override_ref)?;
        let retries = self.inner.config.runtime.provider_retries;
        let mut last_err = MaestroError::Config("no model candidates".into());

        for (ci, model) in candidates.iter().enumerate() {
            for attempt in 0..=retries {
                if cancel.is_cancelled() {
                    return Err(MaestroError::Cancelled);
                }
                let started = now_ms();
                let outcome = self.inner.registry.generate(model, req, cancel).await;
                match outcome {
                    Ok((resp, cost)) => {
                        self.record_invocation(
                            mission_id,
                            task,
                            role,
                            model,
                            InvocationStatus::Succeeded,
                            resp.usage.input_tokens,
                            resp.usage.output_tokens,
                            cost,
                            resp.latency_ms,
                            None,
                        )?;
                        return Ok(resp);
                    }
                    Err(MaestroError::Cancelled) => {
                        self.record_invocation(
                            mission_id,
                            task,
                            role,
                            model,
                            InvocationStatus::Cancelled,
                            0,
                            0,
                            0,
                            now_ms() - started,
                            None,
                        )?;
                        return Err(MaestroError::Cancelled);
                    }
                    Err(err) => {
                        let (retryable, retry_after) = match &err {
                            MaestroError::Provider {
                                retryable,
                                retry_after_ms,
                                ..
                            } => (*retryable, *retry_after_ms),
                            _ => (false, None),
                        };
                        self.record_invocation(
                            mission_id,
                            task,
                            role,
                            model,
                            InvocationStatus::Failed,
                            0,
                            0,
                            0,
                            now_ms() - started,
                            Some(&err.to_string()),
                        )?;
                        last_err = err;
                        if retryable && attempt < retries {
                            // Jittered exponential backoff capped at 1s,
                            // honoring provider retry hints.
                            let base = 100u64.saturating_mul(1 << attempt.min(4));
                            let jitter = (now_ms() as u64) % 50;
                            let wait = retry_after
                                .map(|ms| ms.max(0) as u64)
                                .unwrap_or(base + jitter)
                                .min(1_000);
                            self.inner.store.append_event(
                                mission_id,
                                task.map(|t| t.id.as_str()),
                                kind::PROVIDER_RETRY,
                                serde_json::json!({
                                    "model": model.to_string(),
                                    "attempt": attempt + 1,
                                    "waitMs": wait,
                                }),
                            )?;
                            tokio::select! {
                                _ = cancel.cancelled() => return Err(MaestroError::Cancelled),
                                _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
            if ci + 1 < candidates.len() {
                self.inner.store.append_event(
                    mission_id,
                    task.map(|t| t.id.as_str()),
                    kind::PROVIDER_FALLBACK,
                    serde_json::json!({
                        "from": model.to_string(),
                        "to": candidates[ci + 1].to_string(),
                    }),
                )?;
            }
        }
        Err(last_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_invocation(
        &self,
        mission_id: &str,
        task: Option<&Task>,
        role: Role,
        model: &ModelRef,
        status: InvocationStatus,
        input_tokens: i64,
        output_tokens: i64,
        cost_micro_usd: i64,
        latency_ms: i64,
        error: Option<&str>,
    ) -> Result<()> {
        self.inner.store.record_invocation(&Invocation {
            id: new_id(),
            mission_id: mission_id.to_string(),
            task_id: task.map(|t| t.id.clone()),
            role,
            provider_id: model.provider.clone(),
            model: model.model.clone(),
            status,
            input_tokens,
            output_tokens,
            cost_micro_usd,
            latency_ms,
            error: error.map(str::to_string),
            created_at: now_ms(),
        })
    }

    pub(crate) fn directive_texts(
        &self,
        mission_id: &str,
        task_id: Option<&str>,
    ) -> Result<Vec<String>> {
        Ok(self
            .inner
            .store
            .active_directives(mission_id, task_id)?
            .into_iter()
            .map(|d| d.text)
            .collect())
    }

    /// Plan the mission (idempotent: returns the stored plan when tasks
    /// already exist). Leaves the mission at `awaiting_plan_approval` or
    /// `planned` and records the git baseline + mission branch.
    pub async fn plan_mission(&self, mission_id: &str, cancel: &CancellationToken) -> Result<Plan> {
        let mission = self.inner.store.get_mission(mission_id)?;
        if !self.inner.store.list_tasks(mission_id)?.is_empty() {
            if let Some(plan) = self.inner.store.get_mission_plan(mission_id)? {
                return Ok(plan);
            }
        }
        self.set_mission_status(mission_id, MissionStatus::Planning, None)?;

        if mission.base_head.is_none() {
            let head = git::head(&self.inner.root).await?;
            let branch = self.inner.worktrees.ensure_mission_branch(&mission).await?;
            self.inner
                .store
                .set_mission_git_base(mission_id, &head, &branch)?;
        }

        let repo = git::repo_summary(&self.inner.root, 200)
            .await
            .unwrap_or_default();
        let instructions = load_project_instructions(&self.inner.root);
        let directives = self.directive_texts(mission_id, None)?;
        let mut user = format!(
            "# Objective\n{}\n\n# Repository\n{}\n",
            mission.objective, repo
        );
        if !instructions.is_empty() {
            user.push_str("\n# Project instructions\n");
            user.push_str(&instructions);
        }
        if !directives.is_empty() {
            user.push_str("\n# Steering directives\n");
            for d in &directives {
                user.push_str("- ");
                user.push_str(d);
                user.push('\n');
            }
        }
        let req = ModelRequest {
            model: String::new(),
            messages: vec![
                ModelMessage::system(if self.inner.config.runtime.guild_roles {
                    prompts::guild_planner_system()
                } else {
                    prompts::planner_system(true)
                }),
                crate::media::message(&self.inner.root, user)?,
            ],
            max_tokens: self.assignment(Role::Planner)?.max_output_tokens,
            temperature: None,
            json: true,
        };
        let resp = self
            .invoke(mission_id, None, Role::Planner, &req, cancel)
            .await?;
        let plan = crate::plan::validate_plan(&resp.text, &self.inner.config.runtime)?;
        let _workspace = self.inner.workspace_gate.read().await;
        self.inner
            .store
            .assert_history_mission_current(mission_id)?;
        self.inner.store.set_mission_plan(mission_id, &plan)?;
        self.inner.store.insert_tasks(mission_id, &plan.tasks)?;
        self.inner.store.refresh_ready(mission_id)?;
        self.inner.store.append_event(
            mission_id,
            None,
            kind::PLAN_PROPOSED,
            serde_json::json!({ "summary": plan.summary, "tasks": plan.tasks.len() }),
        )?;
        let next = if self.inner.config.runtime.plan_approval {
            MissionStatus::AwaitingPlanApproval
        } else {
            MissionStatus::Planned
        };
        self.set_mission_status(mission_id, next, None)?;
        Ok(plan)
    }

    /// Approve, reject, or replace the stored plan (before execution only).
    pub async fn decide_plan(
        &self,
        mission_id: &str,
        approve: bool,
        edited_plan: Option<Plan>,
    ) -> Result<Mission> {
        let mission = self.inner.store.get_mission(mission_id)?;
        if !matches!(
            mission.status,
            MissionStatus::AwaitingPlanApproval | MissionStatus::Planned
        ) {
            return Err(MaestroError::Conflict(format!(
                "mission is {}; plans can only be decided before execution",
                mission.status.as_str()
            )));
        }
        if !approve {
            self.inner.store.append_event(
                mission_id,
                None,
                kind::PLAN_REJECTED,
                serde_json::json!({}),
            )?;
            self.set_mission_status(mission_id, MissionStatus::Cancelled, Some("plan rejected"))?;
            return self.inner.store.get_mission(mission_id);
        }
        if let Some(plan) = edited_plan {
            let raw = serde_json::to_string(&plan)?;
            let validated = crate::plan::validate_plan(&raw, &self.inner.config.runtime)?;
            self.inner
                .store
                .replace_mission_tasks(mission_id, &validated.tasks)?;
            self.inner.store.set_mission_plan(mission_id, &validated)?;
        }
        self.inner.store.refresh_ready(mission_id)?;
        self.inner.store.append_event(
            mission_id,
            None,
            kind::PLAN_APPROVED,
            serde_json::json!({}),
        )?;
        self.set_mission_status(mission_id, MissionStatus::Planned, None)?;
        self.poke();
        self.inner.store.get_mission(mission_id)
    }

    /// Drive the mission until a terminal or awaiting state.
    pub async fn run_mission(
        &self,
        mission_id: &str,
        cancel: &CancellationToken,
    ) -> Result<MissionStatus> {
        self.inner
            .store
            .assert_history_mission_current(mission_id)?;
        let mission = self.inner.store.get_mission(mission_id)?;
        if mission.status == MissionStatus::Planning
            || self.inner.store.list_tasks(mission_id)?.is_empty()
        {
            self.plan_mission(mission_id, cancel).await?;
        }
        let mission = self.inner.store.get_mission(mission_id)?;
        match mission.status {
            MissionStatus::AwaitingPlanApproval
            | MissionStatus::AwaitingApproval
            | MissionStatus::Completed
            | MissionStatus::Failed
            | MissionStatus::Cancelled
            // A deliberately paused mission must not be silently resumed by
            // a retry/approval ensure; resuming is an explicit user action.
            | MissionStatus::Paused => return Ok(mission.status),
            _ => {}
        }
        {
            let _guard = self.inner.workspace_gate.read().await;
            self.ensure_shadow_tasks(mission_id)?;
        }
        self.prepare_workspace_snapshot(&mission).await?;
        // Tolerate a concurrent cancel/pause between the check above and the
        // transition to running: the transition is a guarded CAS UPDATE
        // (only planning/planned/running/blocked may enter execution), so a
        // cancel landing in the window is never overwritten. A plain re-read
        // + unconditional write left exactly that race open.
        if !self.inner.store.begin_mission_running(mission_id)? {
            return Ok(self.inner.store.get_mission(mission_id)?.status);
        }
        self.inner.store.append_event(
            mission_id,
            None,
            kind::MISSION_STATUS,
            serde_json::json!({
                "status": MissionStatus::Running.as_str(),
                "reason": Option::<&str>::None,
            }),
        )?;
        self.execute(mission_id, cancel).await
    }

    fn ensure_shadow_tasks(&self, mission_id: &str) -> Result<()> {
        if !self.inner.config.runtime.guild_roles {
            return Ok(());
        }
        self.inner
            .store
            .assert_history_mission_current(mission_id)?;
        let tasks = self.inner.store.list_tasks(mission_id)?;
        let mut count = tasks.len();
        for coder in tasks.iter().filter(|t| t.role == Role::Coder) {
            if tasks
                .iter()
                .any(|t| t.role == Role::Security && t.dependencies.contains(&coder.task_key))
            {
                continue;
            }
            if count >= self.inner.config.runtime.max_tasks_per_mission {
                return Err(MaestroError::Config(
                    "Cannot add required Shadow review: mission task limit reached".into(),
                ));
            }
            let mut dependencies = vec![coder.task_key.clone()];
            if let Some(verifier) = tasks
                .iter()
                .find(|t| t.role == Role::Reviewer && t.dependencies.contains(&coder.task_key))
            {
                dependencies.push(verifier.task_key.clone());
            }
            let spec=maestro_types::plan::PlanTask{key:format!("shadow-{}",coder.id),role:Role::Security,title:format!("Shadow review: {}",coder.title),description:"Independently review the implementation, Sentinel evidence, correctness, regressions, readability and security. Return a factual patch-bound verdict; unresolved issues must not be approved.".into(),dependencies,acceptance:vec!["Independent review and security verdict with concrete evidence".into()],risk:coder.risk,write_scopes:vec![]};
            let added = self.inner.store.insert_tasks(mission_id, &[spec])?;
            count += 1;
            self.inner.store.append_event(mission_id,None,"guild.review_added",serde_json::json!({"task":added[0],"reason":"PS-001 requires independent Shadow review"}))?;
        }
        self.inner.store.refresh_ready(mission_id)?;
        Ok(())
    }

    /// The wake-driven scheduler loop.
    async fn execute(&self, mission_id: &str, cancel: &CancellationToken) -> Result<MissionStatus> {
        let child = cancel.child_token();
        let mut workers: JoinSet<String> = JoinSet::new();
        let mut active: Vec<ActiveInfo> = Vec::new();
        let ttl = self.inner.config.runtime.lease_ttl_ms.max(1_000);
        // Capped at 1s: poke() uses notify_waiters (no stored permit), so the
        // fallback park bounds the staleness of any lost wakeup.
        let park = Duration::from_millis((ttl as u64 / 3).clamp(200, 1_000));

        loop {
            // External pause/cancel flips status first; honor it at the
            // safe boundary.
            let mission = self.inner.store.get_mission(mission_id)?;
            let interrupted = match mission.status {
                MissionStatus::Paused | MissionStatus::Cancelled => true,
                _ => cancel.is_cancelled(),
            };
            if interrupted {
                child.cancel();
                while workers.join_next().await.is_some() {}
                let mission = self.inner.store.get_mission(mission_id)?;
                if mission.status == MissionStatus::Running {
                    // Guarded fallback write: a cancel landing between the
                    // re-read above and this write must win — a user's
                    // explicit cancel must never be downgraded to a
                    // resumable pause.
                    if self.inner.store.set_mission_status_if(
                        mission_id,
                        MissionStatus::Running,
                        MissionStatus::Paused,
                        Some("interrupted"),
                    )? {
                        self.inner.store.append_event(
                            mission_id,
                            None,
                            kind::MISSION_STATUS,
                            serde_json::json!({
                                "status": MissionStatus::Paused.as_str(),
                                "reason": "interrupted",
                            }),
                        )?;
                        return Ok(MissionStatus::Paused);
                    }
                    return Ok(self.inner.store.get_mission(mission_id)?.status);
                }
                return Ok(mission.status);
            }

            self.inner.store.recover_expired_leases(now_ms())?;
            self.inner.store.refresh_ready(mission_id)?;
            let mut tasks = self.inner.store.list_tasks(mission_id)?;

            // Merge fully gated patches before spawning the next wave so a
            // ready coder that depends on them forks from a base that
            // already contains their files.
            if self.inner.config.integration.strategy
                == maestro_types::config::IntegrationStrategy::MissionBranch
                && tasks.iter().any(|t| t.status == TaskStatus::Ready)
                && tasks.iter().any(|t| {
                    t.role == Role::Coder && t.integrated_at.is_none() && t.patch_hash.is_some()
                })
            {
                match self.merge_queue_pass(mission_id, &tasks, true).await? {
                    None => tasks = self.inner.store.list_tasks(mission_id)?,
                    Some(crate::integration::Completion::Continue) => continue,
                    Some(crate::integration::Completion::Done(status)) => return Ok(status),
                }
            }

            let spawned = self.spawn_batch(&tasks, &mut workers, &mut active, &child)?;

            if active.is_empty() && spawned == 0 {
                if tasks.iter().any(|t| t.status == TaskStatus::Ready) {
                    // Ready work exists but is blocked on stale locks;
                    // retry after a short park.
                    tokio::select! {
                        _ = self.inner.notify.notified() => {}
                        _ = tokio::time::sleep(park) => {}
                        _ = child.cancelled() => {}
                    }
                    continue;
                }
                match self.try_complete(mission_id).await? {
                    crate::integration::Completion::Done(status) => return Ok(status),
                    crate::integration::Completion::Continue => continue,
                }
            }

            tokio::select! {
                joined = workers.join_next(), if !workers.is_empty() => {
                    match joined {
                        Some(Ok(task_id)) => active.retain(|a| a.task_id != task_id),
                        Some(Err(join_err)) => {
                            tracing::error!(error = %join_err, "task worker panicked");
                            // Drop meta for tasks no longer running; the
                            // lease recovery loop reclaims the task itself.
                            let running: std::collections::HashSet<String> = self
                                .inner.store.list_tasks(mission_id)?
                                .into_iter()
                                .filter(|t| t.status == TaskStatus::Running)
                                .map(|t| t.id)
                                .collect();
                            active.retain(|a| running.contains(&a.task_id));
                        }
                        None => {}
                    }
                }
                _ = self.inner.notify.notified() => {}
                _ = tokio::time::sleep(park) => {}
                _ = child.cancelled() => {}
            }
        }
    }

    /// Select and spawn runnable ready tasks, respecting concurrency, shared
    /// reader workspaces, and coder write-scope overlap.
    fn spawn_batch(
        &self,
        tasks: &[Task],
        workers: &mut JoinSet<String>,
        active: &mut Vec<ActiveInfo>,
        cancel: &CancellationToken,
    ) -> Result<usize> {
        let max = self.inner.config.runtime.max_concurrent_tasks.max(1);
        let by_key: BTreeMap<&str, &Task> =
            tasks.iter().map(|t| (t.task_key.as_str(), t)).collect();
        let mut spawned = 0usize;

        for task in tasks.iter().filter(|t| t.status == TaskStatus::Ready) {
            if active.len() >= max {
                break;
            }
            if active.iter().any(|a| a.task_id == task.id) {
                continue;
            }
            let mut info = ActiveInfo {
                task_id: task.id.clone(),
                reader_workspace: None,
                coder_scopes: Vec::new(),
            };
            match task.role {
                Role::Coder => {
                    let overlap = active.iter().any(|a| {
                        !a.coder_scopes.is_empty()
                            && scopes::scopes_overlap(&a.coder_scopes, &task.write_scopes)
                    });
                    if overlap {
                        continue;
                    }
                    info.coder_scopes = task.write_scopes.clone();
                }
                Role::Reviewer | Role::Security => {
                    let dep_ws = task
                        .dependencies
                        .iter()
                        .filter_map(|k| by_key.get(k.as_str()))
                        .find_map(|d| d.workspace_path.clone().map(PathBuf::from));
                    if let Some(ws) = &dep_ws {
                        let in_use = active
                            .iter()
                            .any(|a| a.reader_workspace.as_deref() == Some(ws.as_path()));
                        if in_use {
                            continue;
                        }
                    }
                    info.reader_workspace = dep_ws;
                }
                _ => {}
            }
            let orch = self.clone();
            let task_id = task.id.clone();
            let token = cancel.clone();
            workers.spawn(async move {
                if let Err(e) = orch.run_task_attempts(&task_id, &token).await {
                    tracing::error!(task_id = %task_id, error = %e, "task worker failed");
                }
                orch.poke();
                task_id
            });
            active.push(info);
            spawned += 1;
        }
        Ok(spawned)
    }

    /// Lease → heartbeat → agent loop, with bounded retries.
    async fn run_task_attempts(&self, task_id: &str, cancel: &CancellationToken) -> Result<()> {
        let store = &self.inner.store;
        let runtime = &self.inner.config.runtime;
        let ttl = runtime.lease_ttl_ms.max(1_000);

        loop {
            let task = store.get_task(task_id)?;
            if task.status != TaskStatus::Ready {
                return Ok(());
            }
            // Lease FIRST, scope locks second. Locks are keyed by task_id, so
            // releasing them after a lost lease CAS would delete a concurrent
            // winner's locks (two-process scenario: daemon + `run --local`).
            let worker = format!("{}:{}", self.inner.generation, &new_id()[..8]);
            if !store.acquire_lease(task_id, &worker, &self.inner.generation, ttl)? {
                return Ok(());
            }
            if task.role == Role::Coder
                && !store.acquire_scope_locks(&task.mission_id, task_id, &task.write_scopes)?
            {
                // Another task holds overlapping scopes; return to ready
                // (clears the lease) and let the scheduler retry later.
                // Owner-checked so a concurrently recovered/reassigned lease
                // is never clobbered.
                match store.set_task_status_owned(
                    task_id,
                    &worker,
                    TaskStatus::Ready,
                    Some("waiting for overlapping write scopes"),
                    None,
                ) {
                    Ok(_) | Err(MaestroError::LeaseLost { .. }) => {}
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // Attempts are only consumed by real attempt failures (see the
            // error arm below): approval parks, pauses, and lost leases must
            // not burn the retry budget. The event still reports which
            // attempt this lease would be.
            let attempt = task.attempts + 1;
            store.append_event(
                &task.mission_id,
                Some(task_id),
                kind::TASK_LEASED,
                serde_json::json!({ "worker": worker, "attempt": attempt }),
            )?;

            let _heartbeat = {
                let store = store.clone();
                let task_id = task_id.to_string();
                let worker = worker.clone();
                OwnedTask(tokio::spawn(async move {
                    let period = Duration::from_millis((ttl as u64 / 3).max(200));
                    loop {
                        tokio::time::sleep(period).await;
                        match store.renew_lease(&task_id, &worker, ttl) {
                            Ok(true) => {}
                            // Definitive: the lease is gone (reassigned or
                            // the task left `running`). Stop renewing.
                            Ok(false) => break,
                            // Transient storage error: keep the heartbeat
                            // alive and retry next period — a single hiccup
                            // must not silently let the lease expire
                            // mid-attempt.
                            Err(err) => {
                                tracing::warn!(
                                    task_id = %task_id,
                                    error = %err,
                                    "lease renewal failed; retrying next heartbeat"
                                );
                            }
                        }
                    }
                }))
            };

            let task = store.get_task(task_id)?;
            let result = self.run_agent(&task, &worker, cancel).await;
            // Every attempt-outcome write below is an owner-checked CAS
            // (`WHERE status = 'running' AND lease_owner = ?`): if this
            // worker's lease expired mid-attempt and the task was reassigned,
            // the stale write must not clobber the new owner's running row
            // (or clear its lease and scope locks). `LeaseLost` from the CAS
            // means exactly that — walk away silently.
            match result {
                Ok(_) => return Ok(()),
                Err(MaestroError::ApprovalRequired { approval_id }) => {
                    match store.set_task_status_owned(
                        task_id,
                        &worker,
                        TaskStatus::Blocked,
                        Some(&format!("Waiting for approval {approval_id}")),
                        None,
                    ) {
                        Ok(_) | Err(MaestroError::LeaseLost { .. }) => {}
                        Err(e) => return Err(e),
                    }
                    return Ok(());
                }
                Err(MaestroError::Cancelled) => {
                    match store.set_task_status_owned(
                        task_id,
                        &worker,
                        TaskStatus::Ready,
                        Some("interrupted at a safe boundary"),
                        None,
                    ) {
                        Ok(_) | Err(MaestroError::LeaseLost { .. }) => {}
                        Err(e) => return Err(e),
                    }
                    return Ok(());
                }
                Err(MaestroError::LeaseLost { .. }) => return Ok(()),
                Err(MaestroError::BudgetExhausted(msg)) => {
                    // Budgets are operator levers, not task defects: park the
                    // task blocked (attempt NOT consumed) and stop the
                    // mission. Raising the cap in maestro.toml and resuming
                    // reopens it exactly where it stopped.
                    let reason = format!("budget exhausted: {msg}");
                    match store.set_task_status_owned(
                        task_id,
                        &worker,
                        TaskStatus::Blocked,
                        Some(&reason),
                        None,
                    ) {
                        Ok(_) | Err(MaestroError::LeaseLost { .. }) => {}
                        Err(e) => return Err(e),
                    }
                    return Ok(());
                }
                Err(err) => {
                    // Only a real attempt failure (model/protocol/max-turns)
                    // consumes the retry budget — and only while this worker
                    // still owns the lease.
                    let attempts = match store.increment_attempts_owned(task_id, &worker) {
                        Ok(n) => n,
                        Err(MaestroError::LeaseLost { .. }) => return Ok(()),
                        Err(e) => return Err(e),
                    };
                    let msg = err.to_string();
                    let next_status = if attempts < runtime.max_attempts_per_task as i64 {
                        TaskStatus::Ready
                    } else {
                        TaskStatus::Failed
                    };
                    match store.set_task_status_owned(
                        task_id,
                        &worker,
                        next_status,
                        Some(&msg),
                        None,
                    ) {
                        Ok(_) => {}
                        Err(MaestroError::LeaseLost { .. }) => return Ok(()),
                        Err(e) => return Err(e),
                    }
                    if next_status == TaskStatus::Ready {
                        store.append_event(
                            &task.mission_id,
                            Some(task_id),
                            kind::TASK_RETRY,
                            serde_json::json!({ "attempt": attempts, "error": msg }),
                        )?;
                        continue;
                    }
                    store.append_event(
                        &task.mission_id,
                        Some(task_id),
                        kind::TASK_STATUS,
                        serde_json::json!({ "status": "failed", "reason": msg }),
                    )?;
                    return Ok(());
                }
            }
        }
    }

    /// All agents start from the user's current files, including uncommitted
    /// application files. Private snapshot refs never move the user's branch.
    async fn prepare_workspace_snapshot(&self, mission: &Mission) -> Result<()> {
        let _workspace = self.inner.workspace_gate.read().await;
        self.inner
            .store
            .assert_history_mission_current(&mission.id)?;
        let store = &self.inner.store;
        if store
            .artifacts_for_mission(&mission.id)?
            .iter()
            .any(|a| a.kind == "workspace_snapshot")
        {
            return Ok(());
        }
        let tasks = store.list_tasks(&mission.id)?;
        if !tasks.iter().any(|t| t.role == Role::Coder)
            || mission.integration_head.is_some()
            || tasks.iter().any(|t| t.patch_hash.is_some())
        {
            return Ok(());
        }
        // Never replace a baseline underneath partial agent edits.
        for t in tasks.iter().filter(|t| t.role == Role::Coder) {
            if let Some(path) = t
                .workspace_path
                .as_deref()
                .map(std::path::Path::new)
                .filter(|p| p.exists())
            {
                if !git::is_clean(path).await? {
                    return Err(MaestroError::Conflict("Cannot refresh the project snapshot while a coder worktree has uncommitted edits".into()));
                }
            }
        }
        let root_head = git::head(&self.inner.root).await?;
        if mission.base_head.as_deref().is_some_and(|b| b != root_head) {
            return Err(MaestroError::Conflict(
                "Repository HEAD changed since planning; start a new mission for the new baseline"
                    .into(),
            ));
        }
        let (base, snapshot) = git::snapshot_working_tree(&self.inner.root).await?;
        let branch = self.inner.worktrees.ensure_mission_branch(mission).await?;
        let old = git::rev_parse(&self.inner.root, &branch).await?;
        if old != base {
            return Err(MaestroError::Conflict(
                "Mission branch already has work; refusing to replace its baseline".into(),
            ));
        }
        git::git(
            &self.inner.root,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                &snapshot,
                &old,
            ],
        )
        .await?;
        store.set_mission_git_base(&mission.id, &base, &branch)?;
        let path = self
            .inner
            .root
            .join(".maestro/worktrees")
            .join(format!("source-{}", mission.id));
        if !path.exists() {
            git::git(
                &self.inner.root,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    &path.to_string_lossy(),
                    &snapshot,
                ],
            )
            .await?;
        }
        store.add_artifact(
            &mission.id,
            None,
            "workspace_snapshot",
            serde_json::json!({
                "sourceHead":base, "snapshotHead":snapshot, "workspacePath":path.to_string_lossy(),
                "includesUncommittedFiles":base != snapshot,
            }),
        )?;
        Ok(())
    }

    /// Resolve where this task works: coders get isolated worktrees, readers
    /// ride their coder dependency's workspace, everyone else uses the root.
    async fn workspace_for(&self, mission: &Mission, task: &Task) -> Result<TaskWorkspace> {
        let _workspace = self.inner.workspace_gate.read().await;
        self.inner
            .store
            .assert_history_mission_current(&mission.id)?;
        match task.role {
            Role::Coder => self.inner.worktrees.for_coder(mission, task).await,
            Role::Reviewer | Role::Security if !task.dependencies.is_empty() => {
                let tasks = self.inner.store.list_tasks(&mission.id)?;
                let mut map = BTreeMap::new();
                for t in &tasks {
                    if let (Some(path), Some(branch), Some(base)) =
                        (&t.workspace_path, &t.branch_name, &t.base_ref)
                    {
                        map.insert(
                            t.task_key.clone(),
                            TaskWorkspace {
                                path: PathBuf::from(path),
                                branch: branch.clone(),
                                base_ref: base.clone(),
                            },
                        );
                    }
                }
                self.inner.worktrees.for_reader(task, &map)
            }
            _ => {
                if let Some(snapshot) = self
                    .inner
                    .store
                    .artifacts_for_mission(&mission.id)?
                    .iter()
                    .find(|a| a.kind == "workspace_snapshot")
                {
                    if let (Some(path), Some(base)) = (
                        snapshot.content["workspacePath"].as_str(),
                        snapshot.content["snapshotHead"].as_str(),
                    ) {
                        return Ok(TaskWorkspace {
                            path: PathBuf::from(path),
                            branch: "snapshot".into(),
                            base_ref: base.into(),
                        });
                    }
                }
                let branch = git::current_branch(&self.inner.root)
                    .await
                    .unwrap_or_default();
                Ok(TaskWorkspace {
                    path: self.inner.root.clone(),
                    branch,
                    base_ref: "HEAD".to_string(),
                })
            }
        }
    }

    fn make_tool_ctx(&self, task: &Task, workspace: &TaskWorkspace) -> Result<ToolCtx> {
        let safety = &self.inner.config.safety;
        let policy = PolicyEngine::new(task.role, &effective_rules(&self.inner.config))?;
        let compiled = scopes::compile_scopes(&task.write_scopes)?;
        let mut sandbox = crate::sandbox::SandboxSpec::new(workspace.path.clone());
        sandbox.mode = safety.sandbox;
        sandbox.allow_network = safety.sandbox_network;
        sandbox.timeout_ms = safety.check_timeout_ms;
        sandbox.max_output_chars = safety.max_check_output_chars;

        let decider: PolicyDecider = {
            let store = self.inner.store.clone();
            let mission_id = task.mission_id.clone();
            let task_id = task.id.clone();
            Box::new(move |action, resource| {
                let action_str = action.as_str();
                if let Some(existing) =
                    store.find_approval(&mission_id, Some(&task_id), action_str, resource)?
                {
                    match existing.status {
                        ApprovalStatus::Approved => return Ok(()),
                        ApprovalStatus::Rejected => {
                            return Err(MaestroError::PolicyDenied {
                                action: action_str.to_string(),
                                resource: resource.to_string(),
                                reason: "rejected by user".into(),
                            })
                        }
                        ApprovalStatus::Pending => {
                            return Err(MaestroError::ApprovalRequired {
                                approval_id: existing.id,
                            })
                        }
                        ApprovalStatus::Expired => {}
                    }
                }
                let approval = store.create_approval(ApprovalRequest {
                    mission_id: &mission_id,
                    task_id: Some(&task_id),
                    action: action_str,
                    resource,
                    patch_hash: None,
                    policy_snapshot: serde_json::json!({
                        "action": action_str,
                        "resource": resource,
                    }),
                    expires_at: None,
                })?;
                store.append_event(
                    &mission_id,
                    Some(&task_id),
                    kind::APPROVAL_REQUESTED,
                    serde_json::json!({
                        "approvalId": approval.id,
                        "action": action_str,
                        "resource": resource,
                    }),
                )?;
                Err(MaestroError::ApprovalRequired {
                    approval_id: approval.id,
                })
            })
        };

        Ok(ToolCtx {
            root: self.inner.root.clone(),
            workspace: workspace.path.clone(),
            base_ref: Some(workspace.base_ref.clone()),
            role: task.role,
            scopes: compiled,
            policy,
            checks: self.inner.config.checks.clone(),
            sandbox,
            max_read_chars: 20_000,
            decider,
        })
    }

    /// The per-attempt agent turn loop.
    async fn run_agent(
        &self,
        task: &Task,
        worker: &str,
        cancel: &CancellationToken,
    ) -> Result<FinalStatus> {
        let store = &self.inner.store;
        let runtime = &self.inner.config.runtime;
        let mission = store.get_mission(&task.mission_id)?;

        let workspace = self.workspace_for(&mission, task).await?;
        store.set_task_worktree(
            &task.id,
            &workspace.path.to_string_lossy(),
            &workspace.branch,
            &workspace.base_ref,
        )?;
        if task.role == Role::Coder && workspace.path != self.inner.root {
            store.append_event(
                &mission.id,
                Some(&task.id),
                kind::WORKTREE_CREATED,
                serde_json::json!({
                    "path": workspace.path.to_string_lossy(),
                    "branch": workspace.branch,
                    "base": workspace.base_ref,
                }),
            )?;
        }

        let tools = registry_for_role(task.role);
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        let ctx = self.make_tool_ctx(task, &workspace)?;

        let all_tasks = store.list_tasks(&mission.id)?;
        // Snapshot the coder dependencies this attempt reviews AT ATTEMPT
        // START: a reviewer/security verdict applies to the patches as they
        // existed when the review began. Binding gates to a fresh read at
        // final time would let a coder retried mid-review re-stamp the gate
        // with a hash the reviewer never saw (and silently merge unreviewed
        // code).
        let gate_targets: Vec<Task> = coder_dependencies(task, &all_tasks)
            .into_iter()
            .cloned()
            .collect();
        let handoffs: Vec<(String, String)> = task
            .dependencies
            .iter()
            .filter_map(|key| all_tasks.iter().find(|t| &t.task_key == key))
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| dependency_handoff(store, t).map(|text| (t.task_key.clone(), text)))
            .collect::<Result<Vec<_>>>()?;
        let instructions = load_project_instructions(&self.inner.root);
        let repo = git::repo_summary(&workspace.path, 200)
            .await
            .unwrap_or_default();
        let max_context = self
            .assignment(task.role)
            .map(|a| a.max_context_chars)
            .unwrap_or(24_000);
        let max_output = self
            .assignment(task.role)
            .ok()
            .and_then(|a| a.max_output_tokens);
        if runtime.agent_runtime == maestro_types::config::AgentRuntime::Langgraph {
            let prompt = prompts::task_prompt(
                &mission.objective,
                task,
                &handoffs,
                &repo,
                &instructions,
                &[],
                &[],
                0,
            );
            return self
                .run_langgraph(
                    task,
                    worker,
                    &workspace,
                    &gate_targets,
                    &tools,
                    &ctx,
                    prompt,
                    cancel,
                )
                .await;
        }
        let system = prompts::agent_system(task.role, &tool_names);

        let mut transcript: Vec<TranscriptEntry> = Vec::new();
        let mut last_tool_hash: Option<String> = None;
        let mut repeat_count: u32 = 0;
        let mut block_clarified = false;
        let mut protocol_repairs = 0;

        // The turn budget is soft: once spent, the agent gets a few grace
        // turns in which tool calls are refused and it is told to emit its
        // final report — killing a nearly-finished attempt wastes all of it.
        const GRACE_TURNS: u32 = 3;
        let max_turns = runtime.max_turns_per_attempt.max(1);
        for turn in 0..max_turns + GRACE_TURNS {
            let in_grace = turn >= max_turns;
            if turn == max_turns {
                transcript.push(TranscriptEntry::tool_result(
                    "[orchestrator] TURN BUDGET EXHAUSTED. Further tool calls will \
                     not be executed. Respond NOW with your final report (kind: \
                     \"final\") summarizing what is done and what remains."
                        .to_string(),
                ));
            }
            if !store.renew_lease(&task.id, worker, runtime.lease_ttl_ms.max(1_000))? {
                return Err(MaestroError::LeaseLost {
                    task_id: task.id.clone(),
                });
            }
            let directives = self.directive_texts(&mission.id, Some(&task.id))?;
            let (tail, omitted) = compact_transcript(&transcript, max_context);
            let prompt = prompts::task_prompt(
                &mission.objective,
                task,
                &handoffs,
                &repo,
                &instructions,
                &directives,
                &tail,
                omitted,
            );
            let req = ModelRequest {
                model: String::new(),
                messages: vec![
                    ModelMessage::system(system.clone()),
                    crate::media::message(&self.inner.root, prompt)?,
                ],
                max_tokens: max_output,
                temperature: None,
                json: true,
            };
            let resp = self
                .invoke(&mission.id, Some(task), task.role, &req, cancel)
                .await?;
            store.append_event(
                &mission.id,
                Some(&task.id),
                kind::AGENT_TURN,
                serde_json::json!({ "raw": clip(&resp.text, 2_000) }),
            )?;

            let turn = match parse_agent_turn(&resp.text) {
                Ok(turn) => turn,
                Err(error) if protocol_repairs < 2 && !in_grace => {
                    // Keep the gathered evidence on a malformed response rather
                    // than restarting the whole inspection from scratch.
                    protocol_repairs += 1;
                    transcript.push(TranscriptEntry::tool_result(format!(
                        "[orchestrator] Invalid response format: {error}. No action was executed. Keep using the evidence already gathered. Reply with exactly one JSON object containing kind=tool and tool/arguments, or kind=final and status/summary/evidence/risks (plus verdict/reasons for review roles)."
                    )));
                    continue;
                }
                Err(error) => return Err(MaestroError::Protocol(error)),
            };
            // The parser accepts one action. Never feed ignored trailing calls or
            // simulated results back as if the orchestrator executed them.
            transcript.push(TranscriptEntry::turn(
                serde_json::to_string(&turn).map_err(|e| MaestroError::Protocol(e.to_string()))?,
            ));

            match turn {
                AgentTurn::Tool(call) if in_grace => {
                    store.append_event(
                        &mission.id,
                        Some(&task.id),
                        kind::TOOL_DENIED,
                        serde_json::json!({ "tool": call.tool, "error": "turn budget exhausted" }),
                    )?;
                    transcript.push(TranscriptEntry::tool_result(
                        "[orchestrator] refused: the turn budget is exhausted — \
                         respond with your final report."
                            .to_string(),
                    ));
                }
                AgentTurn::Tool(call) => {
                    let canonical = call.arguments.to_string();
                    let hash = hex::encode(Sha256::digest(
                        format!("{}\n{}", call.tool, canonical).as_bytes(),
                    ));
                    if last_tool_hash.as_deref() == Some(hash.as_str()) {
                        repeat_count += 1;
                    } else {
                        last_tool_hash = Some(hash);
                        repeat_count = 1;
                    }
                    if repeat_count >= runtime.loop_detection_window.max(2) {
                        store.append_event(
                            &mission.id,
                            Some(&task.id),
                            kind::LOOP_DETECTED,
                            serde_json::json!({ "tool": call.tool, "repeats": repeat_count }),
                        )?;
                        return Err(MaestroError::Other(format!(
                            "loop detected: {} identical `{}` calls",
                            repeat_count, call.tool
                        )));
                    }

                    store.append_event(
                        &mission.id,
                        Some(&task.id),
                        kind::TOOL_CALL,
                        serde_json::json!({
                            "tool": call.tool,
                            "arguments": clip(&canonical, 1_000),
                        }),
                    )?;
                    let result_json = self
                        .execute_tool(&tools, &ctx, task, &call.tool, &call.arguments)
                        .await?;
                    if matches!(call.tool.as_str(), "run_check" | "run_project_task") {
                        let index = call.arguments["index"].as_u64().map(|n| n as usize);
                        let command = if call.tool == "run_project_task" {
                            Some(vec![
                                "npm".into(),
                                "run".into(),
                                call.arguments["name"].as_str().unwrap_or("").into(),
                            ])
                        } else {
                            index
                                .and_then(|i| ctx.checks.get(i))
                                .map(|c| c.command.clone())
                        };
                        store.add_artifact(
                            &mission.id,
                            Some(&task.id),
                            "check_result",
                            serde_json::json!({
                                "command": command,
                                "workspace": ctx.workspace.to_string_lossy(),
                                "result": result_json,
                            }),
                        )?;
                    }
                    store.append_event(
                        &mission.id,
                        Some(&task.id),
                        kind::TOOL_RESULT,
                        serde_json::json!({
                            "tool": call.tool,
                            "output": clip(&result_json.to_string(), 2_000),
                        }),
                    )?;
                    transcript.push(TranscriptEntry::tool_result(format!(
                        "[{}] {}",
                        call.tool, result_json
                    )));
                }
                AgentTurn::Final(report)
                    if report.status == FinalStatus::Blocked && !block_clarified && !in_grace =>
                {
                    // One bounded clarification, never an automatic approval or an
                    // unbounded retry. Genuine blockers remain the model's decision.
                    block_clarified = true;
                    transcript.push(TranscriptEntry::tool_result(
                        "[orchestrator] Before stopping, reassess only YOUR assigned task and acceptance criteria. The mission objective includes work assigned to other roles. Expected role restrictions (for example, Designer cannot edit or launch a server) do not block a design handoff. Continue permitted inspection if it can resolve missing evidence. If your own task is genuinely blocked, return blocked again with the precise obstacle and required next action; do not fabricate success or bypass restrictions.".to_string(),
                    ));
                }
                AgentTurn::Final(report) => {
                    return self
                        .handle_final(task, &workspace, worker, &gate_targets, report)
                        .await;
                }
            }
        }
        Err(MaestroError::Other(format!(
            "exceeded {} turns (+{GRACE_TURNS} grace) without a final report",
            runtime.max_turns_per_attempt
        )))
    }

    /// Execute one tool call; policy/argument failures are fed back to the
    /// model as `{error}` results. `ApprovalRequired` propagates and parks
    /// the task.
    pub(crate) async fn execute_tool(
        &self,
        tools: &[Arc<dyn ToolExec>],
        ctx: &ToolCtx,
        task: &Task,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        // A named backup must not race another task's file writes, including
        // direct-workspace mode. Acquire exclusively before any read guard.
        let _backup = if name == "save_version" {
            Some(self.inner.workspace_gate.write().await)
        } else {
            None
        };
        let _workspace = if _backup.is_none() {
            Some(self.inner.workspace_gate.read().await)
        } else {
            None
        };
        self.inner
            .store
            .assert_history_mission_current(&task.mission_id)?;
        self.ensure_history_recovered().await?;
        let Some(tool) = tools.iter().find(|t| t.name() == name) else {
            return Ok(serde_json::json!({
                "error": format!("unknown or unpermitted tool `{name}`"),
            }));
        };
        let editing = matches!(name, "write_file" | "replace_text");
        if editing {
            self.capture_version(
                &ctx.workspace,
                Some(&task.mission_id),
                Some(&task.id),
                "draft",
                &format!("Before {}", task.title),
                false,
            )
            .await?;
        }
        match tool.execute(ctx, args).await {
            Ok(ToolOutcome::SaveVersion { label }) => {
                let project = ctx.workspace == self.inner.root;
                let version = self
                    .capture_version(
                        &ctx.workspace,
                        Some(&task.mission_id),
                        Some(&task.id),
                        "backup",
                        &label,
                        project,
                    )
                    .await?;
                Ok(
                    serde_json::json!({"versionId":version.id,"label":version.label,"kind":version.kind,"scope":if project{"project"}else{"task workspace"},"restoreCommand":format!("/restore {}",version.id)}),
                )
            }
            Ok(ToolOutcome::Result(v)) => {
                if editing {
                    self.capture_version(
                        &ctx.workspace,
                        Some(&task.mission_id),
                        Some(&task.id),
                        "draft",
                        &format!(
                            "{}: {}",
                            task.title,
                            args["path"].as_str().unwrap_or("files")
                        ),
                        ctx.workspace == self.inner.root,
                    )
                    .await?;
                }
                Ok(v)
            }
            Ok(ToolOutcome::Delegate(req)) => {
                if self.inner.config.runtime.guild_roles && req.role == Role::Designer {
                    return Ok(
                        serde_json::json!({"error":"Shogun owns design; use Samurai, Sentinel or Shadow execution roles"}),
                    );
                }
                let spec = PlanTask {
                    key: req.key.clone(),
                    role: req.role,
                    title: req.title,
                    description: req.description,
                    dependencies: vec![task.task_key.clone()],
                    acceptance: vec![],
                    risk: RiskLevel::Low,
                    write_scopes: req.write_scopes,
                };
                match self.inner.store.add_delegated_task(
                    &task.mission_id,
                    &task.id,
                    &spec,
                    self.inner.config.runtime.max_delegation_depth as i64,
                    self.inner.config.runtime.max_tasks_per_mission as i64,
                ) {
                    Ok(t) => {
                        // Every coder patch needs a reviewer gate to merge;
                        // a delegated coder without one would block the
                        // mission at integration, so pair it automatically.
                        let mut warning = None;
                        if t.role == Role::Coder {
                            let key: String =
                                format!("review-{}", t.task_key).chars().take(48).collect();
                            let review_spec = PlanTask {
                                key,
                                role: Role::Reviewer,
                                title: format!("Review the '{}' patch", t.task_key),
                                description: format!(
                                    "Review the diff produced by the delegated task \
                                     '{}' against its description and record a verdict.",
                                    t.task_key
                                ),
                                dependencies: vec![t.task_key.clone()],
                                acceptance: vec![],
                                risk: RiskLevel::Low,
                                write_scopes: vec![],
                            };
                            if let Err(e) = self.inner.store.add_delegated_task(
                                &task.mission_id,
                                &task.id,
                                &review_spec,
                                self.inner.config.runtime.max_delegation_depth as i64,
                                self.inner.config.runtime.max_tasks_per_mission as i64,
                            ) {
                                tracing::warn!(
                                    task = %t.task_key,
                                    error = %e,
                                    "could not add auto-reviewer for delegated coder"
                                );
                                warning = Some(format!(
                                    "no reviewer could be added ({e}); the patch \
                                     will not merge without a reviewer gate"
                                ));
                            }
                        }
                        self.ensure_shadow_tasks(&task.mission_id)?;
                        self.inner.store.refresh_ready(&task.mission_id)?;
                        self.poke();
                        let mut result = serde_json::json!({
                            "delegated": t.task_key,
                            "taskId": t.id,
                            "note": "the delegated task runs after this one completes",
                        });
                        if let Some(w) = warning {
                            result["warning"] = serde_json::json!(w);
                        }
                        Ok(result)
                    }
                    Err(e) => Ok(serde_json::json!({ "error": e.to_string() })),
                }
            }
            Err(MaestroError::ApprovalRequired { approval_id }) => {
                Err(MaestroError::ApprovalRequired { approval_id })
            }
            Err(e) => {
                self.inner.store.append_event(
                    &task.mission_id,
                    Some(&task.id),
                    kind::TOOL_DENIED,
                    serde_json::json!({ "tool": name, "error": e.to_string() }),
                )?;
                Ok(serde_json::json!({ "error": e.to_string() }))
            }
        }
    }

    /// Validate and persist a final report: verdict rules, coder scope
    /// re-verification + patch hashing, gate recording, artifacts, status.
    ///
    /// `gate_targets` is the coder-dependency snapshot captured when the
    /// attempt started; verdict gates bind those patch hashes — the ones the
    /// reviewer actually reviewed — never a fresh read at final time. The
    /// terminal status write is an owner CAS (`worker`), so a stale worker
    /// cannot mark a reassigned task completed/failed.
    pub(crate) async fn handle_final(
        &self,
        task: &Task,
        workspace: &TaskWorkspace,
        worker: &str,
        gate_targets: &[Task],
        report: FinalReport,
    ) -> Result<FinalStatus> {
        let _workspace = self.inner.workspace_gate.read().await;
        self.inner
            .store
            .assert_history_mission_current(&task.mission_id)?;
        let store = &self.inner.store;
        let needs_verdict = task.role.is_read_only();
        if needs_verdict && report.verdict.is_none() {
            return Err(MaestroError::Protocol(format!(
                "{} final must include a verdict",
                task.role
            )));
        }
        if !needs_verdict && report.verdict.is_some() {
            return Err(MaestroError::Protocol(format!(
                "{} final must not include a verdict",
                task.role
            )));
        }

        if task.role == Role::Coder && report.status == FinalStatus::Completed {
            // Commit worktree changes so the branch carries the patch, then
            // re-verify scopes against the real diff and bind its hash.
            if !git::is_clean(&workspace.path).await? {
                git::git(&workspace.path, &["add", "-A"]).await?;
                git::git(
                    &workspace.path,
                    &[
                        "-c",
                        "user.name=Maestro",
                        "-c",
                        "user.email=maestro@localhost",
                        "commit",
                        "-m",
                        &format!("maestro: {}", task.task_key),
                    ],
                )
                .await?;
            }
            if let Err(e) = self
                .inner
                .worktrees
                .assert_task_scopes(workspace, &task.write_scopes)
                .await
            {
                store.append_event(
                    &task.mission_id,
                    Some(&task.id),
                    kind::SCOPE_VIOLATION,
                    serde_json::json!({ "error": e.to_string() }),
                )?;
                return Err(e);
            }
            let hash = git::patch_hash(&workspace.path, &workspace.base_ref).await?;
            let diff = git::diff_binary(&workspace.path, &workspace.base_ref).await?;
            if !diff.trim().is_empty() {
                store.set_task_patch_hash(&task.id, &hash)?;
                store.add_artifact(
                    &task.mission_id,
                    Some(&task.id),
                    "worktree",
                    serde_json::json!({
                        "path": workspace.path.to_string_lossy(),
                        "branch": workspace.branch,
                        "patchHash": hash,
                        "baseRef": workspace.base_ref,
                        "changedPaths": git::diff_name_only(&workspace.path, &workspace.base_ref).await?,
                        "scopeVerified": true,
                    }),
                )?;
            } else if task.patch_hash.is_some() {
                // A retried attempt converged on an empty diff (e.g. the base
                // now includes what the first attempt wrote): the stale hash
                // would demand a reviewer gate for a patch that no longer
                // exists.
                store.clear_task_patch_hash(&task.id)?;
            }
        }

        if let Some(verdict) = report.verdict {
            // Bind the verdict to EVERY reviewed coder dependency's patch AS
            // SNAPSHOTTED AT ATTEMPT START (`gate_targets`): a reviewer
            // covering N coders records N gates, each bound to the patch
            // hash that coder had when the review began — the code the
            // reviewer actually read. Re-reading the hash here would stamp a
            // mid-review retried patch as "reviewed". A stale hash simply
            // fails `gates_satisfied` freshness and forces a re-review.
            // `source_task_id` differs from `task_id` by construction: a
            // model can never approve its own patch.
            for coder in gate_targets {
                let gate = maestro_types::domain::Gate {
                    id: new_id(),
                    mission_id: task.mission_id.clone(),
                    task_id: coder.id.clone(),
                    role: task.role,
                    source_task_id: task.id.clone(),
                    verdict,
                    reasons: report.reasons.clone(),
                    patch_hash: coder.patch_hash.clone(),
                    created_at: now_ms(),
                };
                store.record_gate(&gate)?;
                store.append_event(
                    &task.mission_id,
                    Some(&task.id),
                    kind::GATE_RECORDED,
                    serde_json::json!({
                        "coderTask": coder.task_key,
                        "role": task.role.as_str(),
                        "verdict": verdict.as_str(),
                    }),
                )?;
            }
        }

        store.add_artifact(
            &task.mission_id,
            Some(&task.id),
            "final",
            serde_json::json!({
                "summary": report.summary,
                "evidence": report.evidence,
                "risks": report.risks,
                "verdict": report.verdict,
                "reasons": report.reasons,
            }),
        )?;

        let status = match report.status {
            FinalStatus::Completed => TaskStatus::Completed,
            FinalStatus::Failed => TaskStatus::Failed,
            FinalStatus::Blocked => TaskStatus::Blocked,
        };
        // Owner CAS: errors with LeaseLost (swallowed by the attempt loop)
        // instead of clobbering a task that was reassigned mid-attempt.
        let reason = if matches!(status, TaskStatus::Blocked | TaskStatus::Failed) {
            Some(if report.reasons.is_empty() {
                report.summary.clone()
            } else {
                report.reasons.join("; ")
            })
        } else {
            None
        };
        store.set_task_status_owned(
            &task.id,
            worker,
            status,
            reason.as_deref(),
            Some(&report.summary),
        )?;
        store.append_event(
            &task.mission_id,
            Some(&task.id),
            kind::TASK_STATUS,
            serde_json::json!({ "status": status.as_str(), "summary": report.summary, "reason": reason }),
        )?;
        self.inner.store.refresh_ready(&task.mission_id)?;
        self.poke();
        Ok(report.status)
    }

    /// Conversational architect answer via the planner's model chain.
    /// The architect drives the read-only repository tools (read / list /
    /// search over the project root) for up to [`ARCHITECT_MAX_TURNS`]
    /// before answering. Not persisted.
    pub async fn answer_question(
        &self,
        question: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        const ARCHITECT_MAX_TURNS: usize = 8;
        /// Per-tool-result clip: keeps 8 turns comfortably inside the
        /// planner's context budget.
        const RESULT_CLIP: usize = 12_000;

        let candidates = self.candidates(Role::Planner, None)?;
        // Reviewer role: the hard read-only guard applies even though the
        // registry below only contains read tools anyway (belt + suspenders).
        let mut sandbox = crate::sandbox::SandboxSpec::new(self.inner.root.clone());
        sandbox.mode = self.inner.config.safety.sandbox;
        let ctx = ToolCtx {
            root: self.inner.root.clone(),
            workspace: self.inner.root.clone(),
            base_ref: None,
            role: Role::Reviewer,
            scopes: scopes::compile_scopes(&[])?,
            policy: PolicyEngine::new(Role::Reviewer, &effective_rules(&self.inner.config))?,
            checks: Vec::new(),
            sandbox,
            max_read_chars: 20_000,
            decider: crate::tools::deny_ask_decider(),
        };
        let tools: Vec<std::sync::Arc<dyn ToolExec>> = registry_for_role(Role::Reviewer)
            .into_iter()
            .filter(|t| {
                matches!(
                    t.name(),
                    "read_file"
                        | "list_files"
                        | "search"
                        | "save_version"
                        | "view_image"
                        | "browser_inspect"
                        | "git_diff"
                )
            })
            .collect();

        let mut messages = vec![
            ModelMessage::system(prompts::architect_system()),
            crate::media::message(&self.inner.root, question.to_string())?,
        ];
        for turn in 0..ARCHITECT_MAX_TURNS {
            let mut left = 4;
            for message in messages.iter_mut().rev() {
                if message.images.len() > left {
                    message.images.drain(..message.images.len() - left);
                }
                left -= message.images.len();
            }
            let req = ModelRequest {
                model: String::new(),
                messages: messages.clone(),
                max_tokens: self.assignment(Role::Planner)?.max_output_tokens,
                temperature: None,
                json: false,
            };
            let mut last_err = MaestroError::Config("no model candidates".into());
            let mut text: Option<String> = None;
            for model in &candidates {
                match self.inner.registry.generate(model, &req, cancel).await {
                    Ok((resp, _)) => {
                        text = Some(resp.text);
                        break;
                    }
                    Err(MaestroError::Cancelled) => return Err(MaestroError::Cancelled),
                    Err(e) => last_err = e,
                }
            }
            let Some(text) = text else {
                return Err(last_err);
            };

            let Some((tool_name, args)) = architect_tool_call(&text) else {
                return Ok(text); // plain prose = the final answer
            };
            let result = match tools.iter().find(|t| t.name() == tool_name) {
                Some(tool) => match tool.execute(&ctx, &args).await {
                    Ok(ToolOutcome::Result(v)) => v.to_string(),
                    Ok(ToolOutcome::SaveVersion{label}) => match self.save_code_version(&label).await {
                        Ok(version)=>serde_json::json!({"versionId":version.id,"label":version.label,"scope":"project","restoreCommand":format!("/restore {}",version.id)}).to_string(),
                        Err(e)=>serde_json::json!({"error":e.to_string()}).to_string(),
                    },
                    Ok(_) => serde_json::json!({"error": "unsupported tool outcome"}).to_string(),
                    Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                },
                None => serde_json::json!({
                    "error": format!("unknown tool '{tool_name}' — read_file, list_files, search, save_version")
                })
                .to_string(),
            };
            let clipped: String = result.chars().take(RESULT_CLIP).collect();
            messages.push(ModelMessage::assistant(
                serde_json::json!({"tool": tool_name, "args": args}).to_string(),
            ));
            let followup = if turn + 1 == ARCHITECT_MAX_TURNS - 1 {
                format!("tool result:\n{clipped}\n\nTool budget exhausted — answer now in prose.")
            } else {
                format!("tool result:\n{clipped}")
            };
            messages.push(crate::media::message(&self.inner.root, followup)?);
        }
        Err(MaestroError::Protocol(
            "architect kept calling tools past the turn budget".into(),
        ))
    }

    /// Unified diff of the mission's integration state (with its hash), or
    /// the concatenated coder worktree diffs before integration.
    pub async fn mission_diff(&self, mission_id: &str) -> Result<(String, Option<String>)> {
        let mission = self.inner.store.get_mission(mission_id)?;
        if let (Some(base), Some(head)) = (&mission.base_head, &mission.integration_head) {
            let diff = git::unified_diff(&self.inner.root, base, head).await?;
            let hash = hex::encode(Sha256::digest(diff.as_bytes()));
            return Ok((diff, Some(hash)));
        }
        let mut out = String::new();
        for task in self.inner.store.list_tasks(mission_id)? {
            if task.role != Role::Coder {
                continue;
            }
            if let (Some(path), Some(base)) = (&task.workspace_path, &task.base_ref) {
                let dir = PathBuf::from(path);
                if dir.exists() {
                    if let Ok(diff) = git::git(&dir, &["diff", base]).await {
                        if !diff.trim().is_empty() {
                            out.push_str(&format!("# task {}\n{}\n", task.task_key, diff));
                        }
                    }
                }
            }
        }
        Ok((out, None))
    }
}

/// Bounded handoff of claims AND runtime receipts. Summaries alone discarded
/// the evidence reviewers need to verify an upstream task's work.
fn dependency_handoff(store: &maestro_store::Store, task: &Task) -> Result<String> {
    let mut out = task.summary.clone().unwrap_or_else(|| task.title.clone());
    out.push_str(&format!(
        "\nRecorded task: status={}, patchHash={:?}, scopes={:?}\n",
        task.status.as_str(),
        task.patch_hash,
        task.write_scopes
    ));
    let artifacts = store.artifacts_for_task(&task.id)?;
    if let Some(report) = artifacts.iter().rev().find(|a| a.kind == "final") {
        out.push_str(&format!(
            "Model final report (claims, not independent proof; artifact {}):\n{}\n",
            report.id,
            clip(&report.content.to_string(), 4000)
        ));
    }
    if let Some(patch) = artifacts.iter().rev().find(|a| a.kind == "worktree") {
        out.push_str(&format!(
            "Runtime patch receipt (artifact {}):\n{}\n",
            patch.id,
            clip(&patch.content.to_string(), 2000)
        ));
    }
    let checks: Vec<_> = artifacts
        .iter()
        .filter(|a| a.kind == "check_result")
        .collect();
    // Preserve the initial observation and the latest verification, even if
    // intermediate check output must be omitted to keep context bounded.
    for (i, receipt) in checks.iter().enumerate() {
        if i == 0 || i + 1 == checks.len() {
            out.push_str(&format!(
                "Runtime check receipt (artifact {}, recordedAt={}):\n{}\n",
                receipt.id,
                receipt.created_at,
                clip(&receipt.content.to_string(), 3000)
            ));
        }
    }
    Ok(out)
}

/// The coder tasks a reviewer/security verdict applies to: every direct
/// dependency of `task` (by task key, within `all`) whose role is coder.
pub(crate) fn coder_dependencies<'a>(task: &Task, all: &'a [Task]) -> Vec<&'a Task> {
    task.dependencies
        .iter()
        .filter_map(|key| all.iter().find(|t| &t.task_key == key))
        .filter(|t| t.role == Role::Coder)
        .collect()
}

pub(crate) fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}… [truncated]")
}

#[cfg(test)]
mod tests {
    use super::looks_like_implementation_request as is_impl;
    use super::*;
    use maestro_types::plan::PlanTask;

    fn task(key: &str, role: Role, deps: &[&str]) -> Task {
        Task {
            id: new_id(),
            mission_id: "m".into(),
            task_key: key.into(),
            role,
            title: key.into(),
            description: "t".into(),
            status: TaskStatus::Completed,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            acceptance: vec![],
            risk: RiskLevel::Low,
            attempts: 0,
            status_reason: None,
            summary: None,
            lease_owner: None,
            lease_expires_at: None,
            lease_generation: None,
            workspace_path: None,
            branch_name: None,
            base_ref: None,
            patch_hash: None,
            integrated_at: None,
            integration_commit: None,
            parent_task_id: None,
            delegation_depth: 0,
            write_scopes: vec![],
            model_override: None,
            created_at: now_ms(),
            updated_at: now_ms(),
        }
    }

    #[test]
    fn coder_dependencies_returns_every_coder_dep() {
        let coder_a = task("coder-a", Role::Coder, &[]);
        let coder_b = task("coder-b", Role::Coder, &[]);
        let designer = task("design", Role::Designer, &[]);
        let reviewer = task(
            "review-all",
            Role::Reviewer,
            &["coder-a", "design", "coder-b"],
        );
        let all = vec![coder_a, coder_b, designer, reviewer.clone()];

        let deps = coder_dependencies(&reviewer, &all);
        let keys: Vec<&str> = deps.iter().map(|t| t.task_key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["coder-a", "coder-b"],
            "every coder dependency must be covered, non-coders skipped"
        );

        // No coder deps → empty (no gate is recorded).
        let lone = task("solo-review", Role::Reviewer, &["design"]);
        assert!(coder_dependencies(&lone, &all).is_empty());
    }

    #[tokio::test]
    async fn scope_lock_failure_returns_task_to_ready_and_spares_winner_locks() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(".maestro")).unwrap();
        let config = MaestroConfig::default();
        let registry = Arc::new(ProviderRegistry::new(&config));
        let orch = Orchestrator::new(
            store.clone(),
            config,
            registry,
            dir.path().to_path_buf(),
            "gen-test".to_string(),
        );
        let mission = store.create_mission("locks").unwrap();
        let tasks = store
            .insert_tasks(
                &mission.id,
                &[PlanTask {
                    key: "c".into(),
                    role: Role::Coder,
                    title: "c".into(),
                    description: "coder".into(),
                    dependencies: vec![],
                    acceptance: vec![],
                    risk: RiskLevel::Low,
                    write_scopes: vec!["src/**".into()],
                }],
            )
            .unwrap();
        store.refresh_ready(&mission.id).unwrap();
        // A concurrent winner (e.g. another process) holds the scope.
        assert!(store
            .acquire_scope_locks(&mission.id, "winner-task", &["src/**".to_string()])
            .unwrap());

        let cancel = CancellationToken::new();
        orch.run_task_attempts(&tasks[0].id, &cancel).await.unwrap();

        let t = store.get_task(&tasks[0].id).unwrap();
        assert_eq!(t.status, TaskStatus::Ready, "task must return to ready");
        assert!(t.lease_owner.is_none(), "lease must be cleared");
        assert_eq!(t.attempts, 0, "a lock conflict is not a model failure");
        // Regression: the winner's locks must survive the loser's exit.
        assert!(!store
            .acquire_scope_locks(&mission.id, "third-task", &["src/**".to_string()])
            .unwrap());
        assert!(store
            .acquire_scope_locks(&mission.id, "winner-task", &["src/**".to_string()])
            .unwrap());
    }

    #[test]
    fn routes_implementation_requests() {
        assert!(is_impl("add a login endpoint"));
        assert!(is_impl("Implement dark mode across the app"));
        assert!(is_impl("fix the flaky websocket reconnect"));
        assert!(is_impl("please refactor the parser module"));
        assert!(is_impl("Build a CLI for this"));
        assert!(!is_impl("Can you go through the whole project for me??"));
        assert!(!is_impl(
            "Hey, Can you look into the repo and explain it to me nicelt"
        ));
        for question in [
            "Now explain me the tech stack of this project",
            "Now, can you please explain the project?",
            "Can you now explain the stack?",
            "I want you to explain the architecture",
        ] {
            assert!(!is_impl(question), "misrouted: {question}");
        }
        assert!(is_impl("Now, can you please implement authentication"));
        assert!(is_impl("Now inspect the project and fix the broken tests"));
        assert!(!is_impl(
            "Please inspect the project and explain how to add a page"
        ));
        assert!(!is_impl("Review the authentication code for bugs"));
        assert!(is_impl("Look into the repository and fix the login bug"));
        assert!(is_impl("Explain the parser then refactor it"));
        assert!(is_impl("hey, could you please wire up the webhooks?"));
        assert!(is_impl("run the tests and fix whatever breaks"));
        assert!(is_impl("do the database migration"));
    }

    #[test]
    fn architect_tool_calls_parse_and_prose_does_not() {
        let (tool, args) =
            architect_tool_call("{\"tool\":\"read_file\",\"args\":{\"path\":\"a\"}}").unwrap();
        assert_eq!(tool, "read_file");
        assert_eq!(args["path"], "a");
        let fenced = "```json\n{\"tool\":\"search\",\"args\":{\"pattern\":\"x\"}}\n```";
        assert_eq!(architect_tool_call(fenced).unwrap().0, "search");
        assert!(architect_tool_call("plain prose answer").is_none());
        assert!(architect_tool_call("{\"not\":\"a tool\"}").is_none());
        // A reply that keeps roleplaying past the call still yields the call.
        let roleplay = "{\"tool\":\"list_files\",\"args\":{\"path\":\"backend/app\"}}\n\n\
                        user tool result:\n{\"entries\":[\"main.py\"]}\n\n\
                        {\"tool\":\"read_file\",\"args\":{\"path\":\"backend/app/main.py\"}}";
        let (tool, args) = architect_tool_call(roleplay).unwrap();
        assert_eq!(tool, "list_files");
        assert_eq!(args["path"], "backend/app");
        let fenced_roleplay =
            "```json\n{\"tool\":\"search\",\"args\":{\"pattern\":\"x\"}}\n```\nuser tool result: {}";
        assert_eq!(architect_tool_call(fenced_roleplay).unwrap().0, "search");
    }

    #[tokio::test]
    async fn architect_reads_the_repo_through_tools() {
        let dir = tempfile::tempdir().unwrap();
        crate::git::test_support::init_repo(dir.path()).await;
        std::fs::write(dir.path().join("probe.txt"), "the secret is needle-4242\n").unwrap();
        let store = Store::open(&dir.path().join(".maestro")).unwrap();
        let config = MaestroConfig::default();
        let registry = Arc::new(ProviderRegistry::new(&config));
        let orch = Orchestrator::new(
            store,
            config,
            registry,
            dir.path().to_path_buf(),
            "gen-test".to_string(),
        );
        let cancel = CancellationToken::new();
        // The mock architect answers ARCHITECT-TOOL-PROBE by first calling
        // read_file(probe.txt), then echoing the tool result — so real file
        // content must flow through the loop.
        let answer = orch
            .answer_question("ARCHITECT-TOOL-PROBE what does probe.txt say?", &cancel)
            .await
            .unwrap();
        assert!(
            answer.contains("needle-4242"),
            "architect must read the file itself: {answer}"
        );
        // A plain question stays a one-shot prose answer.
        let plain = orch
            .answer_question("what should we build?", &cancel)
            .await
            .unwrap();
        assert!(plain.contains("mock architect"));
    }

    #[test]
    fn routes_questions_to_the_architect() {
        assert!(!is_impl("how is authentication structured?"));
        assert!(!is_impl("what does the merge queue do?"));
        assert!(!is_impl(""));
        assert!(!is_impl("thanks!"));
        assert!(!is_impl("can you explain the valuation service?"));
        assert!(!is_impl("could you walk me through the webhook flow"));
        assert!(!is_impl("tell me about the transaction model"));
        assert!(!is_impl("is the database async?"));
        assert!(!is_impl("do you support CSV import?"));
        assert!(!is_impl("ok cool"));
    }
}
