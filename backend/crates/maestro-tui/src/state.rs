//! Normalized client-side store. Pure — no I/O — updated only from SSE
//! events ([`ClientState::apply_event`]) and explicit fetch results, so the
//! whole reducer is unit-testable.

use maestro_types::api::{MissionDetail, MissionSummary, TaskCounts};
use maestro_types::domain::{
    Approval, ApprovalStatus, MissionStatus, ProviderHealth, Task, TaskStatus,
};
use maestro_types::event::{kind, EventRecord};
use maestro_types::plan::Plan;
use maestro_types::provider::ModelDiscovery;
use std::collections::{BTreeMap, HashMap, VecDeque};

pub const EVENT_RING_CAP: usize = 500;
pub const STREAM_TAIL_CAP: usize = 8 * 1024;
pub const TRANSCRIPT_CAP: usize = 2000;
/// Max cached unified diffs (tiny LRU keyed by mission id).
pub const DIFF_CACHE_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    Connected,
    Connecting,
    StartingDaemon,
    Offline,
}

/// Transcript line kinds; the gutter glyph is derived from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    User,
    Assistant,
    Tool,
    ToolResult,
    System,
    Error,
    /// Inline approval card; renders live from `approvals` by id.
    ApprovalCard {
        approval_id: String,
    },
    /// Inline plan-approval card; renders live from `plans` by mission id.
    PlanCard {
        mission_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    /// Stable id — render-cache key together with width.
    pub id: String,
    pub kind: EntryKind,
    pub text: String,
}

/// A cached unified diff plus render state derived once when the diff is
/// stored. Settled content is never re-derived per frame (SPEC Goal 3): the
/// draw path slices the visible window out of `line_spans` in O(height)
/// without cloning the text or re-scanning it for newlines.
#[derive(Debug)]
pub struct CachedDiff {
    unified: String,
    /// Byte range of every line (terminators excluded), matching
    /// `str::lines()` semantics. Computed once in [`CachedDiff::new`].
    line_spans: Vec<(usize, usize)>,
}

impl CachedDiff {
    pub fn new(unified: String) -> Self {
        let bytes = unified.as_bytes();
        let mut line_spans = Vec::new();
        let mut start = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                // `str::lines()` strips one '\r' before the '\n', if present.
                let end = if i > start && bytes[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                line_spans.push((start, end));
                start = i + 1;
            }
        }
        if start < bytes.len() {
            line_spans.push((start, bytes.len()));
        }
        CachedDiff {
            unified,
            line_spans,
        }
    }

    /// The full unified diff text (borrowed — never cloned by the UI). The
    /// render path only ever uses the line window; this exists for tests.
    #[cfg(test)]
    pub fn unified(&self) -> &str {
        &self.unified
    }

    pub fn is_empty(&self) -> bool {
        self.unified.is_empty()
    }

    /// Total line count, precomputed (used for scroll clamping).
    pub fn line_count(&self) -> usize {
        self.line_spans.len()
    }

    /// Up to `count` lines starting at line `start` — O(count) slicing into
    /// the stored text; no allocation, no full-text scan.
    pub fn lines(&self, start: usize, count: usize) -> impl Iterator<Item = &str> {
        self.line_spans
            .iter()
            .skip(start)
            .take(count)
            .map(move |&(s, e)| &self.unified[s..e])
    }
}

/// What the app must do after applying an event (fetches are I/O, so they
/// happen outside this pure module).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub fetch_mission: Option<String>,
    pub fetch_approvals: bool,
}

#[derive(Debug)]
pub struct ClientState {
    pub connectivity: Connectivity,
    /// Roles running on mock providers (from /health); welcome + chat warn.
    pub mock_roles: Vec<String>,
    /// Newest-first mission summaries (fetch order preserved).
    pub missions: Vec<MissionSummary>,
    /// mission id → tasks (from mission_detail fetches, patched by events).
    pub tasks: HashMap<String, Vec<Task>>,
    /// mission id → proposed plan.
    pub plans: HashMap<String, Plan>,
    /// Pending approvals only.
    pub approvals: Vec<Approval>,
    pub transcript: VecDeque<TranscriptEntry>,
    /// Monotonic count of entries popped off the transcript front (cap
    /// enforcement). Lets render caches drop the same prefix incrementally
    /// instead of rebuilding.
    pub transcript_dropped: u64,
    /// task id → streaming text tail (bounded).
    pub streaming: BTreeMap<String, String>,
    pub provider_health: Vec<ProviderHealth>,
    /// Bounded ring of raw events for the Events tab.
    pub events: VecDeque<EventRecord>,
    pub last_seq: i64,
    /// Mission this chat session is currently driving.
    pub active_mission: Option<String>,
    /// Mission whose plan awaits approval (plan card shown).
    pub pending_plan: Option<String>,
    pub models: Option<ModelDiscovery>,
    pub role_models: Vec<maestro_types::api::RoleModelInfo>,
    /// mission id → unified diff (lazy-fetched, capped at [`DIFF_CACHE_CAP`]).
    pub diffs: HashMap<String, CachedDiff>,
    /// Insertion order for `diffs` (front = oldest, evicted first).
    diff_order: VecDeque<String>,
    /// A chat send is in flight (spinner in transcript tail).
    pub waiting_chat: bool,
    pub session_id: String,
    next_entry: u64,
}

impl ClientState {
    pub fn new(session_id: String) -> Self {
        ClientState {
            connectivity: Connectivity::Connecting,
            mock_roles: Vec::new(),
            missions: Vec::new(),
            tasks: HashMap::new(),
            plans: HashMap::new(),
            approvals: Vec::new(),
            transcript: VecDeque::new(),
            transcript_dropped: 0,
            streaming: BTreeMap::new(),
            provider_health: Vec::new(),
            events: VecDeque::new(),
            last_seq: 0,
            active_mission: None,
            pending_plan: None,
            models: None,
            role_models: Vec::new(),
            diffs: HashMap::new(),
            diff_order: VecDeque::new(),
            waiting_chat: false,
            session_id,
            next_entry: 0,
        }
    }

    // ---- explicit fetch results -------------------------------------------

    pub fn set_missions(&mut self, missions: Vec<MissionSummary>) {
        self.missions = missions;
    }

    pub fn set_mission_detail(&mut self, detail: MissionDetail) {
        let id = detail.mission.id.clone();
        if let Some(plan) = detail.plan {
            self.plans.insert(id.clone(), plan);
        }
        self.tasks.insert(id.clone(), detail.tasks);
        let counts = self.count_tasks(&id);
        match self.missions.iter_mut().find(|m| m.mission.id == id) {
            Some(s) => {
                s.mission = detail.mission;
                s.stats = detail.stats;
                s.task_counts = counts;
            }
            None => self.missions.insert(
                0,
                MissionSummary {
                    mission: detail.mission,
                    stats: detail.stats,
                    task_counts: counts,
                    pending_approvals: 0,
                },
            ),
        }
        if self.active_mission.as_deref() == Some(id.as_str())
            && self
                .mission(&id)
                .is_some_and(|m| m.mission.status == MissionStatus::Blocked)
        {
            let reason = self.blocking_reason().unwrap_or("a task needs attention");
            let message = format!("mission blocked: {reason}");
            if !self.transcript.iter().any(|e| e.text == message) {
                self.system_line(message);
            }
        }
        if let Some(m) = self.mission(&id) {
            if m.mission.status == MissionStatus::AwaitingPlanApproval {
                self.pending_plan = Some(id);
            }
        }
    }

    pub fn set_approvals(&mut self, approvals: Vec<Approval>) {
        self.approvals = approvals
            .into_iter()
            .filter(|a| a.status == ApprovalStatus::Pending)
            .collect();
    }

    pub fn set_models(&mut self, discovery: ModelDiscovery) {
        self.models = Some(discovery);
    }

    pub fn set_diff(&mut self, mission_id: String, unified: String) {
        if self
            .diffs
            .insert(mission_id.clone(), CachedDiff::new(unified))
            .is_some()
        {
            // Refresh recency on overwrite.
            self.diff_order.retain(|m| *m != mission_id);
        }
        self.diff_order.push_back(mission_id);
        while self.diffs.len() > DIFF_CACHE_CAP {
            match self.diff_order.pop_front() {
                Some(oldest) => {
                    self.diffs.remove(&oldest);
                }
                None => break,
            }
        }
    }

    /// Drop a cached diff (it just went stale — e.g. a merge landed).
    pub fn invalidate_diff(&mut self, mission_id: &str) {
        if self.diffs.remove(mission_id).is_some() {
            self.diff_order.retain(|m| m != mission_id);
        }
    }

    // ---- lookups -----------------------------------------------------------

    pub fn mission(&self, id: &str) -> Option<&MissionSummary> {
        self.missions.iter().find(|m| m.mission.id == id)
    }

    pub fn mission_mut(&mut self, id: &str) -> Option<&mut MissionSummary> {
        self.missions.iter_mut().find(|m| m.mission.id == id)
    }

    pub fn active_summary(&self) -> Option<&MissionSummary> {
        self.active_mission
            .as_deref()
            .and_then(|id| self.mission(id))
    }

    /// Prefer the originating agent report to downstream dependency blocks.
    pub fn blocking_reason(&self) -> Option<&str> {
        let blocked: Vec<_> = self
            .active_tasks()
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Blocked))
            .collect();
        blocked
            .iter()
            .find(|t| t.summary.is_some())
            .or_else(|| blocked.first())
            .and_then(|t| t.status_reason.as_deref().or(t.summary.as_deref()))
    }

    pub fn active_tasks(&self) -> &[Task] {
        self.active_mission
            .as_deref()
            .and_then(|id| self.tasks.get(id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn running_tasks(&self) -> Vec<&Task> {
        self.active_tasks()
            .iter()
            .filter(|t| t.status == TaskStatus::Running)
            .collect()
    }

    pub fn pending_approvals(&self) -> usize {
        self.approvals.len()
    }

    /// True while anything animates: drives the ticker task's lifetime.
    /// Awaiting-approval states are deliberately *not* animating — nothing
    /// spins while a mission waits on a human, so the ticker can stop.
    pub fn animating(&self) -> bool {
        self.waiting_chat
            || !self.streaming.is_empty()
            || self.connectivity == Connectivity::StartingDaemon
            || self.connectivity == Connectivity::Connecting
            || self
                .active_summary()
                .map(|m| m.mission.status.is_active())
                .unwrap_or(false)
    }

    fn count_tasks(&self, mission_id: &str) -> TaskCounts {
        let mut c = TaskCounts::default();
        if let Some(tasks) = self.tasks.get(mission_id) {
            c.total = tasks.len() as i64;
            for t in tasks {
                match t.status {
                    TaskStatus::Completed => c.completed += 1,
                    TaskStatus::Running => c.running += 1,
                    TaskStatus::Failed => c.failed += 1,
                    TaskStatus::Blocked => c.blocked += 1,
                    _ => {}
                }
            }
        }
        c
    }

    // ---- transcript --------------------------------------------------------

    pub fn push_entry(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.next_entry += 1;
        self.transcript.push_back(TranscriptEntry {
            id: format!("e{}", self.next_entry),
            kind,
            text: text.into(),
        });
        while self.transcript.len() > TRANSCRIPT_CAP {
            self.transcript.pop_front();
            self.transcript_dropped += 1;
        }
    }

    pub fn system_line(&mut self, text: impl Into<String>) {
        self.push_entry(EntryKind::System, text);
    }

    // ---- the reducer -------------------------------------------------------

    pub fn apply_event(&mut self, ev: &EventRecord) -> Applied {
        let mut out = Applied::default();
        if ev.seq > self.last_seq {
            self.last_seq = ev.seq;
        }
        self.events.push_back(ev.clone());
        while self.events.len() > EVENT_RING_CAP {
            self.events.pop_front();
        }

        let payload = &ev.payload;
        let is_active = self.active_mission.as_deref() == Some(ev.mission_id.as_str());
        match ev.kind.as_str() {
            kind::MISSION_CREATED => {
                out.fetch_mission = Some(ev.mission_id.clone());
            }
            kind::MISSION_STATUS => {
                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(MissionStatus::parse);
                match (status, self.mission_mut(&ev.mission_id)) {
                    (Some(st), Some(m)) => {
                        m.mission.status = st;
                        m.mission.updated_at = ev.created_at;
                    }
                    _ => out.fetch_mission = Some(ev.mission_id.clone()),
                }
                if let Some(st) = status {
                    match st {
                        MissionStatus::AwaitingPlanApproval => {
                            self.pending_plan = Some(ev.mission_id.clone());
                            if !self.plans.contains_key(&ev.mission_id) {
                                out.fetch_mission = Some(ev.mission_id.clone());
                            }
                        }
                        st if (st.is_terminal()
                            || matches!(st, MissionStatus::Blocked | MissionStatus::Paused))
                            && is_active =>
                        {
                            match payload.get("reason").and_then(|v| v.as_str()) {
                                Some(reason) if !reason.is_empty() => {
                                    self.system_line(format!("mission {}: {reason}", st.as_str()))
                                }
                                _ => self.system_line(format!("mission {}", st.as_str())),
                            }
                        }
                        _ => {}
                    }
                }
            }
            kind::PLAN_PROPOSED => {
                if let Some(plan) = payload
                    .get("plan")
                    .and_then(|p| serde_json::from_value::<Plan>(p.clone()).ok())
                {
                    self.plans.insert(ev.mission_id.clone(), plan);
                } else if !self.plans.contains_key(&ev.mission_id) {
                    out.fetch_mission = Some(ev.mission_id.clone());
                }
                if is_active {
                    self.push_entry(
                        EntryKind::PlanCard {
                            mission_id: ev.mission_id.clone(),
                        },
                        String::new(),
                    );
                }
            }
            kind::PLAN_APPROVED => {
                if self.pending_plan.as_deref() == Some(ev.mission_id.as_str()) {
                    self.pending_plan = None;
                }
                if is_active {
                    self.system_line("plan approved");
                }
            }
            kind::PLAN_REJECTED => {
                if self.pending_plan.as_deref() == Some(ev.mission_id.as_str()) {
                    self.pending_plan = None;
                }
                if is_active {
                    self.system_line("plan rejected");
                }
            }
            kind::TASK_STATUS => {
                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(TaskStatus::parse);
                let reason = payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let task_id = ev.task_id.clone().or_else(|| {
                    payload
                        .get("taskId")
                        .and_then(|v| v.as_str())
                        .map(Into::into)
                });
                let mut patched = false;
                if let (Some(tid), Some(st)) = (task_id.as_deref(), status) {
                    if let Some(tasks) = self.tasks.get_mut(&ev.mission_id) {
                        if let Some(t) = tasks.iter_mut().find(|t| t.id == tid) {
                            t.status = st;
                            t.status_reason = reason.clone();
                            t.updated_at = ev.created_at;
                            patched = true;
                        }
                    }
                    if !matches!(st, TaskStatus::Running) {
                        self.streaming.remove(tid);
                    }
                }
                if is_active && matches!(status, Some(TaskStatus::Failed | TaskStatus::Blocked)) {
                    if let Some(reason) = reason.as_deref().filter(|r| !r.is_empty()) {
                        let title = task_id
                            .as_deref()
                            .and_then(|id| {
                                self.tasks.get(&ev.mission_id)?.iter().find(|t| t.id == id)
                            })
                            .map(|t| t.title.as_str())
                            .unwrap_or("task");
                        self.push_entry(EntryKind::Error, format!("{title}: {reason}"));
                    }
                }
                if patched {
                    let counts = self.count_tasks(&ev.mission_id);
                    if let Some(m) = self.mission_mut(&ev.mission_id) {
                        m.task_counts = counts;
                    }
                } else {
                    out.fetch_mission = Some(ev.mission_id.clone());
                }
            }
            kind::TASK_LEASED | kind::TASK_RETRY => {
                out.fetch_mission = Some(ev.mission_id.clone());
            }
            kind::AGENT_STREAM => {
                let task_id = ev.task_id.clone().or_else(|| {
                    payload
                        .get("taskId")
                        .and_then(|v| v.as_str())
                        .map(Into::into)
                });
                let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(tid) = task_id {
                    let buf = self.streaming.entry(tid).or_default();
                    buf.push_str(text);
                    if buf.len() > STREAM_TAIL_CAP {
                        // Keep the tail; trim at a char boundary.
                        let cut = buf.len() - STREAM_TAIL_CAP;
                        let cut = (cut..buf.len())
                            .find(|i| buf.is_char_boundary(*i))
                            .unwrap_or(0);
                        buf.drain(..cut);
                    }
                }
            }
            kind::AGENT_TURN => {
                if let Some(tid) = ev.task_id.as_deref() {
                    self.streaming.remove(tid);
                }
            }
            kind::TOOL_CALL if is_active => {
                let tool = payload.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                let args = payload
                    .get("arguments")
                    .map(compact_json)
                    .unwrap_or_default();
                self.push_entry(EntryKind::Tool, format!("{tool} {args}"));
            }
            kind::TOOL_RESULT if is_active => {
                let text = payload
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| compact_json(payload));
                self.push_entry(EntryKind::ToolResult, text);
            }
            kind::TOOL_DENIED if is_active => {
                let tool = payload.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                self.push_entry(EntryKind::Error, format!("tool denied: {tool}"));
            }
            kind::APPROVAL_REQUESTED => {
                let parsed = serde_json::from_value::<Approval>(payload.clone())
                    .ok()
                    .or_else(|| {
                        payload
                            .get("approval")
                            .and_then(|a| serde_json::from_value(a.clone()).ok())
                    });
                match parsed {
                    Some(a) if a.status == ApprovalStatus::Pending => {
                        let id = a.id.clone();
                        if !self.approvals.iter().any(|x| x.id == id) {
                            self.approvals.push(a);
                        }
                        self.push_entry(EntryKind::ApprovalCard { approval_id: id }, String::new());
                    }
                    _ => out.fetch_approvals = true,
                }
            }
            kind::APPROVAL_DECIDED => {
                let id = payload
                    .get("approvalId")
                    .or_else(|| payload.get("id"))
                    .and_then(|v| v.as_str());
                match id {
                    Some(id) => self.approvals.retain(|a| a.id != id),
                    None => out.fetch_approvals = true,
                }
            }
            kind::MERGE_COMPLETED | kind::INTEGRATION_FINALIZED => {
                // The mission's tree just changed: any cached diff is stale.
                self.invalidate_diff(&ev.mission_id);
            }
            kind::BUDGET_EXHAUSTED if is_active => {
                self.push_entry(EntryKind::Error, "budget exhausted".to_string());
            }
            _ => {}
        }
        out
    }
}

/// Compact single-line JSON, clipped for transcript display.
fn compact_json(v: &serde_json::Value) -> String {
    let mut s = v.to_string();
    if s.len() > 160 {
        let cut = (0..=160)
            .rev()
            .find(|i| s.is_char_boundary(*i))
            .unwrap_or(0);
        s.truncate(cut);
        s.push('…');
    }
    s
}

/// Build a fresh `Mission` for tests / optimistic inserts.
#[cfg(test)]
pub fn test_mission(id: &str, status: MissionStatus) -> maestro_types::domain::Mission {
    maestro_types::domain::Mission {
        id: id.into(),
        objective: "obj".into(),
        status,
        plan_summary: None,
        base_head: None,
        mission_branch: None,
        integration_head: None,
        final_approval_id: None,
        created_at: 1,
        updated_at: 1,
    }
}

#[cfg(test)]
pub fn test_summary(id: &str, status: MissionStatus) -> MissionSummary {
    MissionSummary {
        mission: test_mission(id, status),
        stats: maestro_types::domain::MissionStats::default(),
        task_counts: TaskCounts::default(),
        pending_approvals: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::domain::{RiskLevel, Role};

    fn ev(
        seq: i64,
        mission: &str,
        task: Option<&str>,
        kind: &str,
        payload: serde_json::Value,
    ) -> EventRecord {
        EventRecord {
            seq,
            mission_id: mission.into(),
            task_id: task.map(Into::into),
            kind: kind.into(),
            payload,
            created_at: 100,
        }
    }

    fn test_task(id: &str, mission: &str, status: TaskStatus) -> Task {
        Task {
            id: id.into(),
            mission_id: mission.into(),
            task_key: "do-it".into(),
            role: Role::Coder,
            title: "Do it".into(),
            description: String::new(),
            status,
            dependencies: vec![],
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
            write_scopes: vec!["src/**".into()],
            model_override: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn task_status_event_updates_task_and_counts() {
        let mut st = ClientState::new("s".into());
        st.missions.push(test_summary("m1", MissionStatus::Running));
        st.tasks.insert(
            "m1".into(),
            vec![test_task("t1", "m1", TaskStatus::Running)],
        );
        let applied = st.apply_event(&ev(
            1,
            "m1",
            Some("t1"),
            kind::TASK_STATUS,
            serde_json::json!({"status": "completed"}),
        ));
        assert_eq!(applied, Applied::default());
        assert_eq!(st.tasks["m1"][0].status, TaskStatus::Completed);
        assert_eq!(st.mission("m1").unwrap().task_counts.completed, 1);
        assert_eq!(st.last_seq, 1);
    }

    #[test]
    fn unknown_task_status_requests_refetch() {
        let mut st = ClientState::new("s".into());
        st.missions.push(test_summary("m1", MissionStatus::Running));
        let applied = st.apply_event(&ev(
            2,
            "m1",
            Some("ghost"),
            kind::TASK_STATUS,
            serde_json::json!({"status": "running"}),
        ));
        assert_eq!(applied.fetch_mission.as_deref(), Some("m1"));
    }

    #[test]
    fn approval_requested_adds_pending_and_card() {
        let mut st = ClientState::new("s".into());
        let approval = serde_json::json!({
            "id": "ap1", "missionId": "m1", "action": "edit",
            "resource": "src/main.rs", "policySnapshot": {},
            "status": "pending", "createdAt": 5
        });
        let applied = st.apply_event(&ev(3, "m1", None, kind::APPROVAL_REQUESTED, approval));
        assert!(!applied.fetch_approvals);
        assert_eq!(st.pending_approvals(), 1);
        assert!(matches!(
            st.transcript.back().unwrap().kind,
            EntryKind::ApprovalCard { .. }
        ));
        // Malformed payload → refetch flag instead.
        let applied = st.apply_event(&ev(
            4,
            "m1",
            None,
            kind::APPROVAL_REQUESTED,
            serde_json::json!({"weird": true}),
        ));
        assert!(applied.fetch_approvals);
    }

    #[test]
    fn approval_decided_removes_pending() {
        let mut st = ClientState::new("s".into());
        st.apply_event(&ev(
            1,
            "m1",
            None,
            kind::APPROVAL_REQUESTED,
            serde_json::json!({
                "id": "ap1", "missionId": "m1", "action": "edit",
                "resource": "x", "policySnapshot": {}, "status": "pending", "createdAt": 5
            }),
        ));
        assert_eq!(st.pending_approvals(), 1);
        st.apply_event(&ev(
            2,
            "m1",
            None,
            kind::APPROVAL_DECIDED,
            serde_json::json!({"approvalId": "ap1", "approved": true}),
        ));
        assert_eq!(st.pending_approvals(), 0);
    }

    #[test]
    fn agent_stream_appends_bounded_buffer() {
        let mut st = ClientState::new("s".into());
        st.apply_event(&ev(
            1,
            "m1",
            Some("t1"),
            kind::AGENT_STREAM,
            serde_json::json!({"text": "hello "}),
        ));
        st.apply_event(&ev(
            2,
            "m1",
            Some("t1"),
            kind::AGENT_STREAM,
            serde_json::json!({"text": "world"}),
        ));
        assert_eq!(st.streaming["t1"], "hello world");
        // Overflow keeps only the tail.
        let big = "x".repeat(STREAM_TAIL_CAP + 100);
        st.apply_event(&ev(
            3,
            "m1",
            Some("t1"),
            kind::AGENT_STREAM,
            serde_json::json!({ "text": big }),
        ));
        assert!(st.streaming["t1"].len() <= STREAM_TAIL_CAP);
        // Terminal task status clears the buffer.
        st.tasks.insert(
            "m1".into(),
            vec![test_task("t1", "m1", TaskStatus::Running)],
        );
        st.apply_event(&ev(
            4,
            "m1",
            Some("t1"),
            kind::TASK_STATUS,
            serde_json::json!({"status": "completed"}),
        ));
        assert!(!st.streaming.contains_key("t1"));
    }

    #[test]
    fn blocked_mission_announces_reason_without_being_terminal() {
        let mut state = ClientState::new("s".into());
        state.active_mission = Some("m1".into());
        state
            .missions
            .push(test_summary("m1", MissionStatus::Running));
        state.apply_event(&ev(1, "m1", None, kind::MISSION_STATUS,
            serde_json::json!({"status":"blocked","reason":"inspection could not open its workspace"})));
        assert!(!MissionStatus::Blocked.is_terminal());
        assert!(state.transcript.iter().any(|e| e
            .text
            .contains("mission blocked: inspection could not open its workspace")));
    }

    #[test]
    fn mission_status_event_patches_and_flags_plan() {
        let mut st = ClientState::new("s".into());
        st.missions
            .push(test_summary("m1", MissionStatus::Planning));
        st.apply_event(&ev(
            1,
            "m1",
            None,
            kind::MISSION_STATUS,
            serde_json::json!({"status": "awaiting_plan_approval"}),
        ));
        assert_eq!(
            st.mission("m1").unwrap().mission.status,
            MissionStatus::AwaitingPlanApproval
        );
        assert_eq!(st.pending_plan.as_deref(), Some("m1"));
    }

    #[test]
    fn events_ring_is_bounded() {
        let mut st = ClientState::new("s".into());
        for i in 0..(EVENT_RING_CAP as i64 + 50) {
            st.apply_event(&ev(
                i + 1,
                "m1",
                None,
                "task.heartbeat",
                serde_json::json!({}),
            ));
        }
        assert_eq!(st.events.len(), EVENT_RING_CAP);
        assert_eq!(st.last_seq, EVENT_RING_CAP as i64 + 50);
    }

    #[test]
    fn transcript_is_bounded_and_counts_drops() {
        let mut st = ClientState::new("s".into());
        for i in 0..(TRANSCRIPT_CAP + 10) {
            st.system_line(format!("line {i}"));
        }
        assert_eq!(st.transcript.len(), TRANSCRIPT_CAP);
        assert_eq!(st.transcript_dropped, 10);
        assert_eq!(st.transcript.front().unwrap().text, "line 10");
    }

    #[test]
    fn diff_cache_is_capped_lru() {
        let mut st = ClientState::new("s".into());
        for i in 0..(DIFF_CACHE_CAP + 1) {
            st.set_diff(format!("m{i}"), format!("diff {i}"));
        }
        assert_eq!(st.diffs.len(), DIFF_CACHE_CAP);
        assert!(!st.diffs.contains_key("m0"), "oldest entry evicted");
        assert!(st.diffs.contains_key(&format!("m{DIFF_CACHE_CAP}")));
        // Overwriting refreshes recency instead of double-counting.
        st.set_diff("m1".into(), "diff 1 v2".into());
        st.set_diff("m5".into(), "diff 5".into());
        st.set_diff("m6".into(), "diff 6".into());
        assert_eq!(st.diffs.len(), DIFF_CACHE_CAP);
        assert_eq!(st.diffs.get("m1").map(|d| d.unified()), Some("diff 1 v2"));
    }

    #[test]
    fn cached_diff_line_index_matches_str_lines() {
        // Line spans are derived once in the constructor and must reproduce
        // `str::lines()` exactly (trailing newline, CRLF, blank lines).
        for text in [
            "@@ -1,2 +1,2 @@\n-old\n+new\n context",
            "one line no newline",
            "trailing newline\n",
            "crlf line\r\nsecond\r\n",
            "\n\nblank heavy\n\n",
            "",
        ] {
            let d = CachedDiff::new(text.to_string());
            let expected: Vec<&str> = text.lines().collect();
            assert_eq!(d.line_count(), expected.len(), "count for {text:?}");
            let got: Vec<&str> = d.lines(0, usize::MAX).collect();
            assert_eq!(got, expected, "lines for {text:?}");
            // Windowing slices the same content.
            let win: Vec<&str> = d.lines(1, 2).collect();
            let exp_win: Vec<&str> = text.lines().skip(1).take(2).collect();
            assert_eq!(win, exp_win, "window for {text:?}");
        }
    }

    #[test]
    fn cached_diff_render_window_is_borrowed_not_recloned() {
        // The draw path must reuse the settled representation: every line the
        // window yields is a slice of the stored String (pointer inside its
        // buffer), never a fresh allocation or copy of the diff.
        let mut st = ClientState::new("s".into());
        st.set_diff("m1".into(), "@@ -1 +1 @@\n-a\n+b\n ctx\n".into());
        let d = st.diffs.get("m1").expect("cached");
        let buf = d.unified().as_bytes().as_ptr_range();
        assert!(d.line_count() > 0);
        for line in d.lines(0, d.line_count()) {
            let p = line.as_ptr();
            assert!(
                buf.contains(&p) || (line.is_empty() && p as usize <= buf.end as usize),
                "window line must borrow from the cached diff buffer"
            );
        }
        // Two windows over the same cache yield identical borrowed slices.
        let a: Vec<*const u8> = d.lines(1, 2).map(str::as_ptr).collect();
        let b: Vec<*const u8> = d.lines(1, 2).map(str::as_ptr).collect();
        assert_eq!(a, b, "settled content is not re-derived between frames");
    }

    #[test]
    fn merge_and_integration_events_invalidate_diff() {
        let mut st = ClientState::new("s".into());
        st.set_diff("m1".into(), "old diff".into());
        st.set_diff("m2".into(), "other diff".into());
        st.apply_event(&ev(
            1,
            "m1",
            None,
            kind::MERGE_COMPLETED,
            serde_json::json!({}),
        ));
        assert!(!st.diffs.contains_key("m1"));
        assert!(st.diffs.contains_key("m2"));
        st.apply_event(&ev(
            2,
            "m2",
            None,
            kind::INTEGRATION_FINALIZED,
            serde_json::json!({}),
        ));
        assert!(st.diffs.is_empty());
        // Invalidated entries can be re-cached without breaking the cap.
        for i in 0..DIFF_CACHE_CAP {
            st.set_diff(format!("n{i}"), "d".into());
        }
        assert_eq!(st.diffs.len(), DIFF_CACHE_CAP);
    }

    #[test]
    fn awaiting_approval_does_not_animate() {
        let mut st = ClientState::new("s".into());
        st.connectivity = Connectivity::Connected;
        st.missions
            .push(test_summary("m1", MissionStatus::AwaitingApproval));
        st.active_mission = Some("m1".into());
        assert!(!st.animating(), "awaiting_approval must not tick");
        st.mission_mut("m1").unwrap().mission.status = MissionStatus::AwaitingPlanApproval;
        assert!(!st.animating(), "awaiting_plan_approval must not tick");
        st.mission_mut("m1").unwrap().mission.status = MissionStatus::Running;
        assert!(st.animating(), "running keeps the spinner");
        st.mission_mut("m1").unwrap().mission.status = MissionStatus::Planning;
        assert!(st.animating(), "planning keeps the spinner");
    }
}
