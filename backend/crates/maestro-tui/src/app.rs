//! The App state machine and the single event-driven main loop:
//! `tokio::select!` over crossterm input, the SSE-fed message channel, and —
//! only while dirty — a one-shot frame-budget sleep. Zero timers at idle; a
//! ticker task exists only while something animates and is aborted when idle.

use crate::client::{initial_cursor, DaemonClient};
use crate::commands::{arg_hint_for, parse_slash, slash_suggestions, SlashCommand};
use crate::keymap::{Action, Ctx, Keymap};
use crate::state::{ClientState, Connectivity, EntryKind};
use crate::theme::Theme;
use crate::ui;
use crate::ui::composer::Composer;
use crate::ui::overlay::{Overlay, PaletteState, PickStage, PickerState, SessionPickerState};
use crate::ui::statusline::{Mode, StatusInput};
use crate::ui::transcript::TranscriptView;
use crossterm::event::{
    Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use futures::StreamExt;
use maestro_types::api::{
    ApprovalDecisionRequest, ChatSendResponse, MissionDetail, PlanDecisionRequest, RetryRequest,
    StateSnapshot, SteerRequest,
};
use maestro_types::domain::{Approval, ChatMessage, ChatRole, ChatSession, MissionStatus, Role};
use maestro_types::event::EventRecord;
use maestro_types::provider::ModelDiscovery;
use maestro_types::{new_id, now_ms, MaestroError, Result};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

/// Minimum interval between draws (~30fps), per UI-BRIEF's one-repaint-clock.
const FRAME: Duration = Duration::from_millis(33);
const TOAST_TTL: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(100);
const DAEMON_START_WINDOW: Duration = Duration::from_secs(10);

/// Options for [`crate::run_tui`].
pub struct TuiOptions {
    /// Project root (contains `maestro.toml` and `.maestro/`).
    pub root: PathBuf,
    /// Invoked when no daemon responds; the TUI then re-probes for ~10s.
    pub spawn_daemon: Option<Box<dyn Fn() -> std::io::Result<()> + Send>>,
}

#[derive(Debug)]
enum AppMsg {
    Event(EventRecord),
    SseDown,
    /// SSE reconnects exhausted; the task parked. No timers run until a
    /// user action, health probe, or daemon spawn restarts the stream.
    SseGaveUp,
    Tick,
    DaemonUp(DaemonClient, Vec<String>),
    /// An idle daemon from an older build was shut down; respawn and re-probe.
    DaemonStale,
    /// A busy daemon from an older build: connect, but tell the user.
    DaemonUpOutdated(DaemonClient),
    DaemonDown(String),
    MissionDetail(Box<MissionDetail>),
    Approvals(Vec<Approval>),
    Models {
        discovery: ModelDiscovery,
        intent: ModelsIntent,
    },
    Snapshot(Box<StateSnapshot>),
    RoleRoster(Vec<maestro_types::api::RoleModelInfo>),
    Diff {
        mission: String,
        unified: String,
    },
    Chat(Box<ChatSendResponse>),
    ImageAttached {
        id: String,
        name: String,
    },
    Previews(serde_json::Value),
    /// Session list arrived; `pick` opens the overlay, otherwise auto-resume
    /// the most recent session at startup.
    Sessions {
        sessions: Vec<ChatSession>,
        pick: bool,
    },
    /// A session's replayed history (switching or startup resume).
    SessionHistory {
        session: ChatSession,
        messages: Vec<ChatMessage>,
    },
    /// Role → model assignments (/roles).
    Roles(Vec<maestro_types::api::RoleModelInfo>),
    Submitted {
        mission_id: String,
    },
    OpOk(&'static str),
    CodeHistory(maestro_types::history::History),
    VersionPreview {
        preview: maestro_types::history::RestorePreview,
        undo: bool,
    },
    VersionDiff {
        id: String,
        unified: String,
    },
    VersionSaved(maestro_types::history::Version),
    VersionRestored(maestro_types::history::RestoreResult),
    OpErr(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelsIntent {
    Browse,
    Assign(Option<Role>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Chat,
    Tasks,
    Diff,
    Approvals,
    Events,
}

pub(crate) struct App {
    root: PathBuf,
    state: ClientState,
    theme: Theme,
    keymap: Keymap,
    tab: Tab,
    composer: Composer,
    /// Selected row in the slash-command suggestion popup.
    suggest_sel: usize,
    /// Suggestions dismissed with Esc until the input changes again.
    suggest_dismissed: bool,
    /// The user pointed at a suggestion with the arrows (authorizes Enter).
    suggest_picked: bool,
    /// Recent sessions shown on the welcome screen (newest first, ≤5).
    recent_sessions: Vec<ChatSession>,
    /// Caret in the welcome session strip; None until armed with ↑/↓.
    welcome_sel: Option<usize>,
    /// Provider id awaiting a masked API key in the composer.
    pending_connect: Option<String>,
    pending_images: Vec<(String, String)>,
    chat_scroll: usize,
    chat_max_scroll: usize,
    tasks_detail_width: u16,
    events_scroll: usize,
    diff_scroll: usize,
    tasks_view: ui::tasks::TasksView,
    approvals_view: ui::approvals::ApprovalsView,
    overlay: Option<Overlay>,
    sidebar_on: bool,
    tool_expanded: bool,
    transcript: TranscriptView,
    transcript_epoch: u64,
    branch: BranchCache,
    toast: Option<(String, Instant)>,
    ctrl_c_at: Option<Instant>,
    spinner_frame: usize,
    page_height: u16,
    dirty: bool,
    last_draw: Instant,
    quit: bool,
    client: Option<DaemonClient>,
    tx: mpsc::Sender<AppMsg>,
    ticker: Option<JoinHandle<()>>,
    sse: Option<JoinHandle<()>>,
    probe: Option<JoinHandle<()>>,
    jobs: JoinSet<()>,
    diff_loading: bool,
    spawn_daemon: Option<Box<dyn Fn() -> std::io::Result<()> + Send>>,
    daemon_spawn_attempted: bool,
}

impl Drop for App {
    fn drop(&mut self) {
        self.shutdown_tasks();
    }
}

impl App {
    fn new(opts: TuiOptions, tx: mpsc::Sender<AppMsg>) -> App {
        let mut state = ClientState::new(new_id());
        state.last_seq = initial_cursor(&opts.root);
        App {
            root: opts.root,
            state,
            theme: Theme::load(),
            keymap: Keymap::load(),
            tab: Tab::Chat,
            composer: Composer::default(),
            suggest_sel: 0,
            suggest_dismissed: false,
            suggest_picked: false,
            recent_sessions: Vec::new(),
            welcome_sel: None,
            pending_connect: None,
            pending_images: Vec::new(),
            chat_scroll: 0,
            chat_max_scroll: 0,
            tasks_detail_width: 80,
            events_scroll: 0,
            diff_scroll: 0,
            tasks_view: Default::default(),
            approvals_view: Default::default(),
            overlay: None,
            sidebar_on: true,
            tool_expanded: false,
            transcript: TranscriptView::default(),
            transcript_epoch: 0,
            branch: BranchCache::default(),
            toast: None,
            ctrl_c_at: None,
            spinner_frame: 0,
            page_height: 20,
            dirty: true,
            last_draw: Instant::now() - FRAME,
            quit: false,
            client: None,
            tx,
            ticker: None,
            sse: None,
            probe: None,
            jobs: JoinSet::new(),
            diff_loading: false,
            spawn_daemon: opts.spawn_daemon,
            daemon_spawn_attempted: false,
        }
    }

    fn shutdown_tasks(&mut self) {
        if let Some(h) = self.ticker.take() {
            h.abort();
        }
        if let Some(h) = self.sse.take() {
            h.abort();
        }
        if let Some(h) = self.probe.take() {
            h.abort();
        }
        self.jobs.abort_all();
    }

    // ---- daemon connectivity ------------------------------------------------

    fn startup(&mut self) {
        self.state.system_line("maestro — /help for commands");
        self.probe_daemon(false);
    }

    /// Probe `.maestro/daemon.json` + /health once; on failure optionally
    /// spawn the daemon and re-probe with capped backoff for ~10s.
    fn probe_daemon(&mut self, post_spawn: bool) {
        let root = self.root.clone();
        let tx = self.tx.clone();
        let window = if post_spawn {
            DAEMON_START_WINDOW
        } else {
            Duration::from_millis(1)
        };
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            let mut backoff = Duration::from_millis(400);
            loop {
                if let Ok(client) = DaemonClient::from_root(&root) {
                    if let Ok(health) = client.health().await {
                        // Stale-daemon handshake: a daemon left running from
                        // an older build answers with a different build id.
                        // Idle → restart it transparently; busy → connect
                        // anyway and say so (never interrupt a mission).
                        if health.build_id != maestro_types::build_id() {
                            let busy = match client.state_snapshot().await {
                                Ok(snap) => {
                                    snap.missions.iter().any(|m| m.mission.status.is_active())
                                }
                                Err(_) => true, // can't tell: don't kill it
                            };
                            if !busy && client.shutdown().await.is_ok() {
                                let _ = tx.send(AppMsg::DaemonStale).await;
                                return;
                            }
                            let _ = tx.send(AppMsg::DaemonUpOutdated(client)).await;
                            return;
                        }
                        let _ = tx.send(AppMsg::DaemonUp(client, health.mock_roles)).await;
                        return;
                    }
                }
                if started.elapsed() >= window {
                    let _ = tx
                        .send(AppMsg::DaemonDown(if post_spawn {
                            crate::daemon_startup_error(&root)
                        } else {
                            "no daemon running".into()
                        }))
                        .await;
                    return;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(2));
            }
        });
        if let Some(old) = self.probe.replace(handle) {
            old.abort();
        }
    }

    /// Fetch the session list; `pick` routes to the overlay, otherwise the
    /// most recent session is resumed (startup).
    fn fetch_sessions(&mut self, pick: bool) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.spawn_msg(async move {
            match client.chat_sessions().await {
                Ok(sessions) => AppMsg::Sessions { sessions, pick },
                Err(e) => AppMsg::OpErr(format!("sessions: {e}")),
            }
        });
    }

    /// Switch to `session`: replay its history into the transcript.
    fn resume_session(&mut self, session: ChatSession) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.spawn_msg(async move {
            match client.chat_history(&session.id).await {
                Ok(messages) => AppMsg::SessionHistory { session, messages },
                Err(e) => AppMsg::OpErr(format!("resume: {e}")),
            }
        });
    }

    fn on_daemon_up(&mut self, client: DaemonClient) {
        self.state.connectivity = Connectivity::Connected;
        self.client = Some(client.clone());
        self.spawn_msg(async move {
            match client.models().await {
                Ok(response) => AppMsg::RoleRoster(response.roles),
                Err(e) => AppMsg::OpErr(format!("team models: {e}")),
            }
        });
        self.fetch_sessions(false);
        self.branch.invalidate();
        self.start_sse();
        self.fetch_snapshot();
        self.dirty = true;
        self.ensure_ticker();
    }

    fn on_daemon_down(&mut self, why: String) {
        if !self.daemon_spawn_attempted {
            if let Some(spawn) = &self.spawn_daemon {
                self.daemon_spawn_attempted = true;
                match spawn() {
                    Ok(()) => {
                        self.state.connectivity = Connectivity::StartingDaemon;
                        self.probe_daemon(true);
                        self.dirty = true;
                        self.ensure_ticker();
                        return;
                    }
                    Err(e) => {
                        self.state
                            .push_entry(EntryKind::Error, format!("failed to spawn daemon: {e}"));
                    }
                }
            }
        }
        // Re-probes (e.g. from require_online) shouldn't repeat the hint.
        if self.state.connectivity != Connectivity::Offline {
            self.state.system_line(format!(
                "{why} — chat commands still work; mission ops need a daemon (`maestro serve`)"
            ));
        }
        self.state.connectivity = Connectivity::Offline;
        self.dirty = true;
        self.ensure_ticker();
    }

    fn start_sse(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let after = self.state.last_seq;
        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            let mut stream = client.events(None, after);
            while let Some(item) = stream.next().await {
                let msg = match item {
                    Ok(ev) => AppMsg::Event(ev),
                    Err(_) => AppMsg::SseDown,
                };
                if tx.send(msg).await.is_err() {
                    return;
                }
            }
            // The stream only ends when the client gave up reconnecting.
            let _ = tx.send(AppMsg::SseGaveUp).await;
        });
        if let Some(old) = self.sse.replace(handle) {
            old.abort();
        }
    }

    // ---- background jobs ----------------------------------------------------

    fn spawn_msg(&mut self, fut: impl std::future::Future<Output = AppMsg> + Send + 'static) {
        let tx = self.tx.clone();
        self.jobs.spawn(async move {
            let msg = fut.await;
            let _ = tx.send(msg).await;
        });
        // Reap finished jobs so the set stays bounded.
        while self.jobs.try_join_next().is_some() {}
    }

    fn online(&self) -> Option<DaemonClient> {
        match (&self.client, self.state.connectivity) {
            (Some(c), Connectivity::Connected) => Some(c.clone()),
            _ => None,
        }
    }

    fn require_online(&mut self) -> Option<DaemonClient> {
        let c = self.online();
        if c.is_none() {
            // A daemon-needing user action is one of the triggers that may
            // revive a parked SSE stream: one-shot health probe; on success
            // DaemonUp → start_sse. No recurring timer is left behind.
            if self.state.connectivity == Connectivity::Offline {
                self.probe_daemon(false);
            }
            self.show_toast("daemon offline — run `maestro serve` first");
        }
        c
    }

    fn fetch_snapshot(&mut self) {
        let Some(client) = self.online() else { return };
        self.spawn_msg(async move {
            match client.state_snapshot().await {
                Ok(s) => AppMsg::Snapshot(Box::new(s)),
                Err(e) => AppMsg::OpErr(format!("snapshot: {e}")),
            }
        });
    }

    fn fetch_mission(&mut self, id: String) {
        let Some(client) = self.online() else { return };
        self.spawn_msg(async move {
            match client.mission_detail(&id).await {
                Ok(d) => AppMsg::MissionDetail(Box::new(d)),
                Err(e) => AppMsg::OpErr(format!("mission: {e}")),
            }
        });
    }

    fn fetch_approvals(&mut self) {
        let Some(client) = self.online() else { return };
        self.spawn_msg(async move {
            match client.approvals(None).await {
                Ok(a) => AppMsg::Approvals(a),
                Err(e) => AppMsg::OpErr(format!("approvals: {e}")),
            }
        });
    }

    fn fetch_diff(&mut self) {
        let Some(mission) = self.diff_target() else {
            return;
        };
        if self.state.diffs.contains_key(&mission) || self.diff_loading {
            return;
        }
        let Some(client) = self.online() else { return };
        self.diff_loading = true;
        self.spawn_msg(async move {
            match client.diff(&mission).await {
                Ok(d) => AppMsg::Diff {
                    mission,
                    unified: d.unified,
                },
                Err(e) => AppMsg::OpErr(format!("diff: {e}")),
            }
        });
    }

    fn diff_target(&self) -> Option<String> {
        self.state
            .active_mission
            .clone()
            .or_else(|| self.state.missions.first().map(|m| m.mission.id.clone()))
    }

    // ---- ticker -------------------------------------------------------------

    fn animating(&self) -> bool {
        self.state.animating() || self.toast.is_some() || self.ctrl_c_at.is_some()
    }

    fn ensure_ticker(&mut self) {
        let need = self.animating();
        match (&self.ticker, need) {
            (None, true) => {
                let tx = self.tx.clone();
                self.ticker = Some(tokio::spawn(async move {
                    let mut interval = tokio::time::interval(TICK);
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        interval.tick().await;
                        if tx.send(AppMsg::Tick).await.is_err() {
                            return;
                        }
                    }
                }));
            }
            (Some(_), false) => {
                if let Some(h) = self.ticker.take() {
                    h.abort();
                }
            }
            _ => {}
        }
    }

    fn on_tick(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        let now = Instant::now();
        if let Some((_, at)) = &self.toast {
            if now.duration_since(*at) >= TOAST_TTL {
                self.toast = None;
            }
        }
        if let Some(at) = self.ctrl_c_at {
            if now.duration_since(at) >= TOAST_TTL {
                self.ctrl_c_at = None;
            }
        }
        self.dirty = true;
        self.ensure_ticker();
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
        self.dirty = true;
        self.ensure_ticker();
    }

    // ---- message handling ---------------------------------------------------

    fn on_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::Event(ev) => {
                self.state.connectivity = Connectivity::Connected;
                let card_change = matches!(
                    ev.kind.as_str(),
                    maestro_types::event::kind::APPROVAL_DECIDED
                        | maestro_types::event::kind::PLAN_APPROVED
                        | maestro_types::event::kind::PLAN_REJECTED
                );
                let applied = self.state.apply_event(&ev);
                if card_change {
                    self.transcript_epoch += 1;
                }
                if let Some(id) = applied.fetch_mission {
                    self.fetch_mission(id);
                }
                if applied.fetch_approvals {
                    self.fetch_approvals();
                }
                self.dirty = true;
                self.ensure_ticker();
            }
            AppMsg::SseDown => {
                if self.state.connectivity == Connectivity::Connected {
                    self.state.connectivity = Connectivity::Connecting;
                    self.dirty = true;
                }
            }
            AppMsg::SseGaveUp => {
                // Ignore a stale give-up if a fresh SSE task already
                // reconnected (connectivity went back to Connected).
                if self.state.connectivity == Connectivity::Connecting {
                    if let Some(h) = self.sse.take() {
                        h.abort();
                    }
                    self.state.connectivity = Connectivity::Offline;
                    self.state.system_line(
                        "daemon unreachable — reconnect paused; daemon commands retry",
                    );
                    self.dirty = true;
                    self.ensure_ticker();
                }
            }
            AppMsg::Tick => self.on_tick(),
            AppMsg::DaemonUp(client, mock_roles) => {
                if !mock_roles.is_empty() {
                    self.state.system_line(format!(
                        "⚠ mock providers: {} — replies are canned, no model is wired. \
                         edit maestro.toml providers or run /connect",
                        mock_roles.join(", ")
                    ));
                }
                self.state.mock_roles = mock_roles;
                self.on_daemon_up(client);
            }
            AppMsg::DaemonStale => {
                // The old-build daemon was already shut down by the probe;
                // spawn a fresh one and wait for it like a cold start.
                self.show_toast("daemon was from an older build — restarting");
                if let Some(spawn) = &self.spawn_daemon {
                    if spawn().is_ok() {
                        self.state.connectivity = Connectivity::StartingDaemon;
                        self.daemon_spawn_attempted = true;
                        self.probe_daemon(true);
                        self.ensure_ticker();
                        return;
                    }
                }
                self.on_daemon_down("stale daemon stopped; spawn failed".into());
            }
            AppMsg::DaemonUpOutdated(client) => {
                self.state.system_line(
                    "connected to a daemon from an older build (a mission is active) — \
                     it will pick up the new build after `maestro shutdown` once idle",
                );
                self.on_daemon_up(client);
            }
            AppMsg::DaemonDown(why) => self.on_daemon_down(why),
            AppMsg::MissionDetail(d) => {
                self.state.set_mission_detail(*d);
                self.tasks_view.clamp(self.state.active_tasks().len());
                self.dirty = true;
                self.ensure_ticker();
            }
            AppMsg::Approvals(a) => {
                self.state.set_approvals(a);
                self.approvals_view.clamp(self.state.approvals.len());
                self.transcript_epoch += 1;
                self.dirty = true;
            }
            AppMsg::Models { discovery, intent } => {
                self.state.set_models(discovery.clone());
                match intent {
                    ModelsIntent::Browse => {
                        self.overlay = Some(Overlay::ModelPicker(PickerState::new(
                            discovery.models,
                            None,
                            true,
                        )));
                    }
                    ModelsIntent::Assign(role) => {
                        self.overlay = Some(Overlay::ModelPicker(PickerState::new(
                            discovery.models,
                            role,
                            false,
                        )));
                    }
                }
                self.dirty = true;
            }
            AppMsg::Snapshot(s) => {
                let s = *s;
                self.state.set_missions(s.missions);
                self.state.set_approvals(s.approvals);
                self.state.provider_health = s.provider_health;
                // Follow the newest active mission if we have none.
                if self.state.active_mission.is_none() {
                    if let Some(active) = self
                        .state
                        .missions
                        .iter()
                        .find(|m| m.mission.status.is_active())
                    {
                        let id = active.mission.id.clone();
                        self.state.active_mission = Some(id.clone());
                        self.fetch_mission(id);
                    }
                }
                self.dirty = true;
                self.ensure_ticker();
            }
            AppMsg::Diff { mission, unified } => {
                self.diff_loading = false;
                self.state.set_diff(mission, unified);
                self.dirty = true;
            }
            AppMsg::Previews(v) => {
                if let Some(items) = v.as_array() {
                    if items.is_empty() {
                        self.state.system_line(
                            "No managed previews. Ask Maestro to start the app locally.",
                        );
                    }
                    for p in items {
                        self.state.system_line(format!(
                            "{} · {}\n{}\n{}",
                            p["url"].as_str().unwrap_or(""),
                            if p["running"].as_bool().unwrap_or(false) {
                                "running"
                            } else {
                                "stopped"
                            },
                            p["workspace"].as_str().unwrap_or(""),
                            p["logs"].as_str().unwrap_or("")
                        ));
                    }
                } else {
                    self.state.system_line("Managed previews stopped");
                }
                self.dirty = true;
            }
            AppMsg::ImageAttached { id, name } => {
                if self.pending_images.len() < 4 {
                    self.pending_images.push((id, name.clone()));
                    self.state.system_line(format!("Attached {name} — send a message to use it; /attach clear removes attachments"));
                } else {
                    self.state.system_line("Four images are already attached");
                }
                self.dirty = true;
            }
            AppMsg::Chat(resp) => {
                self.pending_images.clear();
                self.state.waiting_chat = false;
                let resp = *resp;
                for msg in resp.messages {
                    let kind = match msg.role {
                        maestro_types::domain::ChatRole::User => EntryKind::User,
                        maestro_types::domain::ChatRole::Assistant => EntryKind::Assistant,
                        maestro_types::domain::ChatRole::Agent => EntryKind::Assistant,
                        maestro_types::domain::ChatRole::System => EntryKind::System,
                    };
                    // Our own user line is already echoed locally.
                    if kind != EntryKind::User {
                        self.state.push_entry(kind, msg.content);
                    }
                }
                self.state.active_mission = resp.mission_id.clone();
                if resp.mission_id.is_none() {
                    self.state.pending_plan = None;
                }
                if let Some(mission_id) = resp.mission_id {
                    self.state.active_mission = Some(mission_id.clone());
                    self.state
                        .system_line(format!("mission {mission_id} launched"));
                    self.fetch_mission(mission_id);
                }
                self.dirty = true;
                self.ensure_ticker();
            }
            AppMsg::RoleRoster(roles) => {
                self.state.role_models = roles;
                self.dirty = true;
            }
            AppMsg::Roles(roles) => {
                self.state.role_models = roles.clone();
                // Aligned table straight into the transcript.
                self.state.system_line("role assignments");
                for r in roles {
                    let fallbacks = if r.fallbacks.is_empty() {
                        String::new()
                    } else {
                        format!("  (fallback: {})", r.fallbacks.join(", "))
                    };
                    self.state.system_line(format!(
                        "  {:<9} {}/{}{fallbacks}",
                        maestro_types::domain::Role::parse(&r.role)
                            .map(|r| r.guild_name())
                            .unwrap_or(&r.role),
                        r.provider,
                        r.model
                    ));
                }
                self.state.system_line(
                    "  change one with /model (fuzzy picker) — applies immediately when idle",
                );
                self.dirty = true;
            }
            AppMsg::Sessions { sessions, pick } => {
                if pick {
                    self.overlay = Some(Overlay::SessionPicker(SessionPickerState::new(sessions)));
                } else {
                    // Welcome strip: newest first, capped. Never auto-resume —
                    // a fresh session is the default, resuming is a choice.
                    let mut sessions = sessions;
                    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
                    sessions.truncate(5);
                    self.recent_sessions = sessions;
                    self.welcome_sel = None;
                }
                self.dirty = true;
            }
            AppMsg::SessionHistory { session, messages } => {
                self.state.session_id = session.id.clone();
                self.state.transcript.clear();
                self.transcript_epoch += 1;
                for msg in messages {
                    let kind = match msg.role {
                        ChatRole::User => EntryKind::User,
                        ChatRole::Assistant | ChatRole::Agent => EntryKind::Assistant,
                        ChatRole::System => EntryKind::System,
                    };
                    self.state.push_entry(kind, msg.content);
                }
                self.state.system_line(format!(
                    "resumed session — {}",
                    crate::ui::clip(&session.title, 60)
                ));
                if let Some(mission_id) = session.active_mission_id {
                    self.state.active_mission = Some(mission_id.clone());
                    self.fetch_mission(mission_id);
                }
                self.chat_scroll = 0;
                self.dirty = true;
                self.ensure_ticker();
            }
            AppMsg::Submitted { mission_id } => {
                self.pending_images.clear();
                self.state.active_mission = Some(mission_id.clone());
                self.state
                    .system_line(format!("mission {mission_id} submitted"));
                self.fetch_mission(mission_id);
                self.dirty = true;
                self.ensure_ticker();
            }
            AppMsg::OpOk(what) => {
                if what == "steering directive sent" {
                    self.pending_images.clear();
                }
                self.show_toast(what);
            }
            AppMsg::CodeHistory(history) => {
                self.overlay = Some(Overlay::History(ui::history::HistoryView::new(history)));
                self.dirty = true;
            }
            AppMsg::VersionPreview { preview, undo } => {
                let mut view = ui::history::HistoryView::new(maestro_types::history::History {
                    versions: vec![],
                    current_id: None,
                });
                view.detail = Some(format!("{}\nVersion: {} · {}\n\n{} unfinished missions will stop. Start a new request after restoring.\nYour conversation and a recovery version will be retained.\n{}\n{}",
                    preview.target.label,preview.target.id,preview.target.kind,preview.affected_missions.len(),
                    if preview.conflicts.is_empty(){"Changes to working files:".into()}else{format!("CONFLICTS — files unchanged:\n{}",preview.conflicts.join("\n"))},
                    if preview.unified.is_empty(){"No file changes."}else{&preview.unified}));
                view.preview = Some(preview);
                view.undo = undo;
                self.overlay = Some(Overlay::History(view));
                self.dirty = true;
            }
            AppMsg::VersionDiff { id, unified } => {
                let mut view = ui::history::HistoryView::new(maestro_types::history::History {
                    versions: vec![],
                    current_id: None,
                });
                view.detail = Some(format!(
                    "{id}\n\n{}",
                    if unified.is_empty() {
                        "No file changes."
                    } else {
                        &unified
                    }
                ));
                self.overlay = Some(Overlay::History(view));
                self.dirty = true;
            }
            AppMsg::VersionSaved(v) => {
                self.state
                    .system_line(format!("saved version {} — {}", v.id, v.label));
                self.dirty = true;
            }
            AppMsg::VersionRestored(result) => {
                self.overlay = None;
                self.state.system_line(format!(
                    "{}\nRecovery version: {}. Start a new request to continue from these files.",
                    result.version.label, result.recovery_id
                ));
                self.state.active_mission = None;
                self.state.diffs.clear();
                self.dirty = true;
            }
            AppMsg::OpErr(err) => {
                if let Some(Overlay::History(v)) = self.overlay.as_mut() {
                    v.busy = false;
                    v.preview = None;
                    v.scroll = 0;
                    v.detail=Some(format!("Could not complete operation: {err}\n\nClose and reopen /history or /undo for a fresh preview.\n\n{}",v.detail.as_deref().unwrap_or("")));
                }
                self.state.waiting_chat = false;
                self.state.push_entry(EntryKind::Error, err);
                self.dirty = true;
                self.ensure_ticker();
            }
        }
    }

    // ---- input --------------------------------------------------------------

    fn on_input(&mut self, event: CtEvent) {
        match event {
            CtEvent::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.on_key(key)
            }
            CtEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    if matches!(self.overlay, Some(Overlay::History(_))) =>
                {
                    let code = if mouse.kind == MouseEventKind::ScrollUp {
                        KeyCode::Up
                    } else {
                        KeyCode::Down
                    };
                    for _ in 0..3 {
                        self.history_key(KeyEvent::new(code, KeyModifiers::NONE));
                    }
                }
                MouseEventKind::ScrollUp => self.scroll_current(3),
                MouseEventKind::ScrollDown => self.scroll_current(-3),
                _ => {}
            },
            CtEvent::Resize(_, _) => {
                self.dirty = true;
            }
            CtEvent::Paste(text) if self.tab == Tab::Chat && self.overlay.is_none() => {
                self.composer.insert_str(&text);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        // Ctrl+C double-tap always wins.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.ctrl_c_at.map(|at| at.elapsed() < TOAST_TTL) == Some(true) {
                self.quit = true;
            } else {
                self.ctrl_c_at = Some(Instant::now());
                self.show_toast("press ctrl+c again to quit");
            }
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(key);
            return;
        }
        if self.pending_connect.is_some() {
            self.masked_key(key);
            return;
        }
        match self.tab {
            Tab::Chat => self.chat_key(key),
            _ => self.view_key(key),
        }
    }

    fn ctx_for_tab(&self) -> Ctx {
        match self.tab {
            Tab::Chat => Ctx::Global,
            Tab::Tasks => Ctx::Tasks,
            Tab::Diff => Ctx::Diff,
            Tab::Approvals => Ctx::Approvals,
            Tab::Events => Ctx::Events,
        }
    }

    /// Current slash suggestions, or empty when inactive/dismissed.
    fn suggestions(&self) -> Vec<SlashCommand> {
        if self.suggest_dismissed || self.pending_connect.is_some() {
            return Vec::new();
        }
        slash_suggestions(self.composer.text())
    }

    /// Complete the selected suggestion into the composer. No-arg commands
    /// submit immediately on Enter; arg-taking commands leave a trailing
    /// space with the ghost hint showing.
    fn complete_suggestion(&mut self, cmd: SlashCommand, submit: bool) {
        self.composer.clear();
        self.composer.insert_str(&format!("/{}", cmd.name()));
        if cmd.arg_hint().is_some() {
            self.composer.insert_str(" ");
        } else if submit {
            self.submit_composer();
        }
        self.suggest_sel = 0;
        self.suggest_dismissed = false;
        self.suggest_picked = false;
    }

    fn chat_key(&mut self, key: KeyEvent) {
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        // Slash-command autocomplete intercepts navigation while the finder
        // is open (the input is a bare `/token`), even on zero matches.
        let token_active =
            !self.suggest_dismissed && crate::commands::slash_token(self.composer.text()).is_some();
        if token_active {
            let sugg = self.suggestions();
            let sel = self.suggest_sel.min(sugg.len().saturating_sub(1));
            match key.code {
                KeyCode::Up if !sugg.is_empty() => {
                    // Wrap: habit from every picker.
                    self.suggest_sel = if sel == 0 { sugg.len() - 1 } else { sel - 1 };
                    self.suggest_picked = true;
                    return;
                }
                KeyCode::Down if !sugg.is_empty() => {
                    self.suggest_sel = if sel + 1 >= sugg.len() { 0 } else { sel + 1 };
                    self.suggest_picked = true;
                    return;
                }
                KeyCode::Tab if !sugg.is_empty() => {
                    self.complete_suggestion(sugg[sel], false);
                    return;
                }
                KeyCode::Enter if plain && !sugg.is_empty() => {
                    // A fuzzy match is a suggestion, never an authorization:
                    // Enter runs the highlighted row only when the typed
                    // token is a real prefix of it, or the user pointed at
                    // it with the arrows. Anything else submits as typed.
                    let token = crate::commands::slash_token(self.composer.text())
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if self.suggest_picked || sugg[sel].name().starts_with(&token) {
                        self.complete_suggestion(sugg[sel], true);
                        return;
                    }
                }
                KeyCode::Esc => {
                    self.suggest_dismissed = true;
                    return;
                }
                _ => {}
            }
        }
        // Welcome session strip: arrows pick a recent session, Enter resumes.
        // Live only while the screen still IS the welcome (pristine, empty
        // composer, no overlay) — one keystroke of typing disarms it.
        if self.composer.is_empty()
            && !self.recent_sessions.is_empty()
            && self
                .state
                .transcript
                .iter()
                .all(|e| e.kind == EntryKind::System)
        {
            match key.code {
                KeyCode::Down => {
                    let n = self.recent_sessions.len();
                    self.welcome_sel = Some(match self.welcome_sel {
                        None => 0,
                        Some(i) => (i + 1).min(n - 1),
                    });
                    return;
                }
                KeyCode::Up => {
                    self.welcome_sel = match self.welcome_sel {
                        None | Some(0) => None,
                        Some(i) => Some(i - 1),
                    };
                    return;
                }
                KeyCode::Enter if self.welcome_sel.is_some() => {
                    if let Some(session) = self
                        .welcome_sel
                        .and_then(|i| self.recent_sessions.get(i))
                        .cloned()
                    {
                        self.resume_session(session);
                    }
                    return;
                }
                KeyCode::Esc if self.welcome_sel.is_some() => {
                    self.welcome_sel = None;
                    return;
                }
                _ => {}
            }
        }
        let text_before = self.composer.text().to_string();
        // Plan card decisions while the composer is empty.
        if self.composer.is_empty() && plain && self.state.pending_plan.is_some() {
            if let Some(action) = self.keymap.lookup(Ctx::PlanCard, key.code, key.modifiers) {
                match action {
                    Action::Approve => {
                        self.decide_plan(true);
                        return;
                    }
                    Action::Reject => {
                        self.decide_plan(false);
                        return;
                    }
                    _ => {}
                }
            }
        }
        // Chorded / non-char bindings, and plain bindings when composer empty.
        let is_char = matches!(key.code, KeyCode::Char(_));
        if !is_char || !plain || self.composer.is_empty() {
            if let Some(action) = self.keymap.lookup(Ctx::Global, key.code, key.modifiers) {
                // While typing, don't steal plain chars for tab switching.
                if !(is_char && plain && !self.composer.is_empty()) {
                    // Escape clears the composer first if it has content.
                    if action == Action::Escape && !self.composer.is_empty() {
                        self.composer.clear();
                        return;
                    }
                    if self.run_action(action) {
                        return;
                    }
                }
            }
        }
        // Composer editing.
        match key.code {
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => self.composer.newline(),
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.newline()
            }
            KeyCode::Enter => self.submit_composer(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.kill_line_start()
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.kill_word()
            }
            KeyCode::Char(c) if plain || key.modifiers == KeyModifiers::SHIFT => {
                self.composer.insert(c)
            }
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            // Up/Down move within a multiline draft; at the edges they
            // recall history (guard side effect: the move itself).
            KeyCode::Up if !self.composer.cursor_up() => self.composer.history_prev(),
            KeyCode::Down if !self.composer.cursor_down() => self.composer.history_next(),
            _ => {}
        }
        // Any input change resets the suggestion cursor and un-dismisses.
        if self.composer.text() != text_before {
            self.suggest_sel = 0;
            self.suggest_dismissed = false;
            self.suggest_picked = false;
        }
    }

    fn view_key(&mut self, key: KeyEvent) {
        let ctx = self.ctx_for_tab();
        if let Some(action) = self.keymap.lookup(ctx, key.code, key.modifiers) {
            self.run_action(action);
        }
    }

    /// Returns true when the action was handled.
    fn run_action(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => self.quit = true,
            Action::Help => self.overlay = Some(Overlay::Help { scroll: 0 }),
            Action::Palette => self.overlay = Some(Overlay::Palette(PaletteState::default())),
            Action::ToggleSidebar => self.sidebar_on = !self.sidebar_on,
            Action::ToggleToolDetail => {
                self.tool_expanded = !self.tool_expanded;
                self.transcript_epoch += 1;
            }
            Action::TabChat => self.tab = Tab::Chat,
            Action::TabTasks => self.tab = Tab::Tasks,
            Action::TabDiff => {
                self.tab = Tab::Diff;
                self.fetch_diff();
            }
            Action::TabApprovals => self.tab = Tab::Approvals,
            Action::TabEvents => self.tab = Tab::Events,
            Action::PageUp => self.scroll_current(self.page_height as i64),
            Action::PageDown => self.scroll_current(-(self.page_height as i64)),
            Action::ScrollUp => self.scroll_current(1),
            Action::ScrollDown => self.scroll_current(-1),
            Action::ScrollBottom => self.set_scroll_current(0),
            Action::Escape => match self.tab {
                Tab::Chat => self.set_scroll_current(0),
                _ => {
                    if self.tab == Tab::Tasks && self.tasks_view.detail_open {
                        self.tasks_view.detail_open = false;
                    } else {
                        self.tab = Tab::Chat;
                    }
                }
            },
            Action::Up => self.select_move(-1),
            Action::Down => self.select_move(1),
            Action::Select => {
                if self.tab == Tab::Tasks {
                    self.tasks_view.detail_open = !self.tasks_view.detail_open;
                    self.tasks_view.detail_scroll = 0;
                }
            }
            Action::Retry => self.retry_selected(),
            Action::ViewDiff => {
                self.tab = Tab::Diff;
                self.fetch_diff();
            }
            Action::Approve => self.decide_selected_approval(true),
            Action::Reject => self.decide_selected_approval(false),
            Action::Interrupt => {}
        }
        true
    }

    fn select_move(&mut self, delta: i64) {
        match self.tab {
            Tab::Tasks if self.tasks_view.detail_open => self.scroll_current(-delta),
            Tab::Tasks => {
                let len = self.state.active_tasks().len();
                let cur = self.tasks_view.selected as i64;
                self.tasks_view.selected =
                    (cur + delta).clamp(0, len.saturating_sub(1) as i64) as usize;
            }
            Tab::Approvals => {
                let len = self.state.approvals.len();
                let cur = self.approvals_view.selected as i64;
                self.approvals_view.selected =
                    (cur + delta).clamp(0, len.saturating_sub(1) as i64) as usize;
            }
            Tab::Events => self.scroll_current(-delta),
            Tab::Diff => self.scroll_current(-delta),
            Tab::Chat => self.scroll_current(-delta),
        }
    }

    fn scroll_current(&mut self, delta: i64) {
        let apply = |v: usize, d: i64| -> usize {
            if d >= 0 {
                v.saturating_add(d as usize)
            } else {
                v.saturating_sub((-d) as usize)
            }
        };
        match self.tab {
            Tab::Tasks if self.tasks_view.detail_open => {
                let max = self
                    .tasks_view
                    .selected_task(&self.state)
                    .map(|task| {
                        ui::tasks::detail_lines(task, &self.theme, self.tasks_detail_width as usize)
                            .len()
                            .saturating_sub(self.page_height.saturating_sub(1) as usize)
                    })
                    .unwrap_or(0);
                self.tasks_view.detail_scroll =
                    apply(self.tasks_view.detail_scroll, -delta).min(max);
            }
            Tab::Tasks => self.select_move(-delta),
            Tab::Chat => {
                self.chat_scroll = apply(self.chat_scroll, delta).min(self.chat_max_scroll);
            }
            Tab::Events => {
                self.events_scroll =
                    apply(self.events_scroll, delta).min(self.state.events.len().saturating_sub(1));
            }
            Tab::Diff => {
                // Diff scroll counts from the top; clamp to the cached line
                // count (precomputed — no per-keypress line scan).
                let lines = self
                    .diff_target()
                    .and_then(|m| self.state.diffs.get(&m))
                    .map(|d| d.line_count())
                    .unwrap_or(0);
                self.diff_scroll = apply(self.diff_scroll, -delta).min(lines.saturating_sub(1));
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn set_scroll_current(&mut self, v: usize) {
        match self.tab {
            Tab::Chat => self.chat_scroll = v,
            Tab::Events => self.events_scroll = v,
            Tab::Diff => self.diff_scroll = v,
            _ => {}
        }
    }

    // ---- masked /connect input ----------------------------------------------

    fn masked_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.pending_connect = None;
                self.composer.masked = false;
                self.composer.clear();
                self.state.system_line("connect cancelled");
            }
            KeyCode::Enter => {
                let secret = self.composer.take();
                self.composer.masked = false;
                let provider = self.pending_connect.take().unwrap_or_default();
                if secret.trim().is_empty() {
                    self.state.system_line("connect cancelled (empty key)");
                    return;
                }
                // The daemon API has no credentials endpoint (SPEC §Daemon
                // HTTP API); write locally like the TS version.
                match crate::local::write_credential(&self.root, &provider, secret.trim()) {
                    Ok(()) => self.state.system_line(format!(
                        "credential stored for {provider} — restart the daemon to apply"
                    )),
                    Err(e) => self
                        .state
                        .push_entry(EntryKind::Error, format!("credential write failed: {e}")),
                }
            }
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Char(c) => self.composer.insert(c),
            _ => {}
        }
    }

    // ---- overlays -------------------------------------------------------------

    fn overlay_key(&mut self, key: KeyEvent) {
        if matches!(self.overlay, Some(Overlay::History(_))) {
            self.history_key(key);
            return;
        }
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        match overlay {
            Overlay::History(_) => {}
            Overlay::Help { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlay = None,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll += 10,
                _ => {}
            },
            Overlay::Palette(p) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Up => p.selected = p.selected.saturating_sub(1),
                KeyCode::Down => {
                    let n = p.ranked().len();
                    p.selected = (p.selected + 1).min(n.saturating_sub(1));
                }
                KeyCode::Backspace => {
                    p.input.pop();
                    p.selected = 0;
                }
                KeyCode::Char(c) => {
                    p.input.push(c);
                    p.selected = 0;
                }
                KeyCode::Enter => {
                    let ranked = p.ranked();
                    let chosen = ranked.get(p.selected).copied();
                    self.overlay = None;
                    if let Some(cmd) = chosen {
                        self.palette_pick(cmd);
                    }
                }
                _ => {}
            },
            Overlay::SessionPicker(p) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Up => p.selected = p.selected.saturating_sub(1),
                KeyCode::Down => {
                    let n = p.ranked().len();
                    p.selected = (p.selected + 1).min(n.saturating_sub(1));
                }
                KeyCode::Backspace => {
                    p.filter.pop();
                    p.selected = 0;
                }
                KeyCode::Char(c) => {
                    p.filter.push(c);
                    p.selected = 0;
                }
                KeyCode::Enter => {
                    let ranked = p.ranked();
                    let chosen = ranked.get(p.selected).map(|&i| p.sessions[i].clone());
                    self.overlay = None;
                    if let Some(session) = chosen {
                        self.resume_session(session);
                    }
                }
                _ => {}
            },
            Overlay::ModelPicker(p) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Up => p.selected = p.selected.saturating_sub(1),
                KeyCode::Down => {
                    let n = match p.stage {
                        PickStage::Model => p.ranked_models().len(),
                        PickStage::Role => Role::ALL.len(),
                    };
                    p.selected = (p.selected + 1).min(n.saturating_sub(1));
                }
                KeyCode::Backspace if p.stage == PickStage::Model => {
                    p.filter.pop();
                    p.selected = 0;
                }
                KeyCode::Char(c) if p.stage == PickStage::Model => {
                    p.filter.push(c);
                    p.selected = 0;
                }
                KeyCode::Enter => self.picker_enter(),
                _ => {}
            },
        }
        self.dirty = true;
    }

    fn palette_pick(&mut self, cmd: SlashCommand) {
        match cmd {
            // Commands that need arguments get prefilled into the composer.
            SlashCommand::Ask
            | SlashCommand::Run
            | SlashCommand::Steer
            | SlashCommand::Restore
            | SlashCommand::Attach
            | SlashCommand::Connect => {
                self.tab = Tab::Chat;
                self.composer.clear();
                self.composer.insert_str(&format!("/{} ", cmd.name()));
            }
            _ => self.run_command(cmd, ""),
        }
    }

    fn picker_enter(&mut self) {
        let Some(Overlay::ModelPicker(p)) = self.overlay.as_mut() else {
            return;
        };
        match p.stage {
            PickStage::Model => {
                let ranked = p.ranked_models();
                let Some(&idx) = ranked.get(p.selected) else {
                    return;
                };
                let m = &p.models[idx];
                if p.browse_only {
                    let text = format!("{}/{}", m.provider_id, m.model);
                    self.overlay = None;
                    self.state.system_line(format!("model: {text}"));
                    return;
                }
                p.chosen = Some((m.provider_id.clone(), m.model.clone()));
                if p.role.is_some() {
                    self.assign_picked();
                } else {
                    p.stage = PickStage::Role;
                    p.selected = 0;
                }
            }
            PickStage::Role => {
                let role = Role::ALL[p.selected.min(Role::ALL.len() - 1)];
                p.role = Some(role);
                self.assign_picked();
            }
        }
    }

    fn assign_picked(&mut self) {
        let Some(Overlay::ModelPicker(p)) = self.overlay.take() else {
            return;
        };
        let (Some(role), Some((provider, model))) = (p.role, p.chosen) else {
            return;
        };
        self.apply_role_model(role, provider, model);
    }

    /// Write the assignment to the config in effect, then apply it: an idle
    /// daemon is restarted on the spot; a busy one gets a note instead of
    /// having its missions yanked.
    fn apply_role_model(&mut self, role: Role, provider: String, model: String) {
        // No assignment endpoint in the daemon API (SPEC): edit the config.
        match crate::local::set_role_model(&self.root, role.as_str(), &provider, &model) {
            Ok(path) => {
                self.state.system_line(format!(
                    "{} → {provider}/{model} written to {}",
                    role.as_str(),
                    path.display()
                ));
                let busy = self
                    .state
                    .missions
                    .iter()
                    .any(|m| m.mission.status.is_active());
                match (self.online(), busy) {
                    (Some(client), false) => {
                        // Idle: bounce the daemon so the change is live now.
                        self.state.connectivity = Connectivity::StartingDaemon;
                        self.state.system_line("restarting daemon to apply…");
                        let spawn_ok = self.spawn_daemon.is_some();
                        self.spawn_msg(async move {
                            match client.shutdown().await {
                                Ok(()) => AppMsg::OpOk("daemon stopping"),
                                Err(e) => AppMsg::OpErr(format!("shutdown: {e}")),
                            }
                        });
                        if spawn_ok {
                            if let Some(spawn) = &self.spawn_daemon {
                                let _ = spawn();
                            }
                            self.probe_daemon(true);
                        }
                        self.ensure_ticker();
                    }
                    (Some(_), true) => {
                        self.show_toast("applies after the active mission (restart daemon)")
                    }
                    (None, _) => {}
                }
            }
            Err(e) => self
                .state
                .push_entry(EntryKind::Error, format!("config edit failed: {e}")),
        }
    }

    // ---- composer submission --------------------------------------------------

    fn submit_composer(&mut self) {
        if self.composer.text().trim().is_empty() {
            self.composer.clear();
            return;
        }
        let text = self.composer.take().trim().to_string();
        self.chat_scroll = 0;
        match parse_slash(&text) {
            Some((cmd, args)) => {
                self.state.push_entry(EntryKind::User, text.clone());
                let args = args.to_string();
                self.run_command(cmd, &args);
            }
            None if text.starts_with('/') => {
                self.state.push_entry(EntryKind::User, text);
                self.state
                    .system_line("unknown command — /help lists commands");
            }
            None => {
                self.state.push_entry(EntryKind::User, text.clone());
                self.plain_text(text);
            }
        }
        self.ensure_ticker();
    }

    /// Plain text routing: steer a running mission, else send to chat.
    fn with_images(&mut self, mut text: String) -> String {
        for (id, name) in &self.pending_images {
            text.push_str(&format!("\nAttached image: {name} [[image:{id}]]"));
        }
        text
    }
    fn plain_text(&mut self, text: String) {
        let text = self.with_images(text);
        let steer_target = self.state.active_summary().and_then(|m| {
            matches!(
                m.mission.status,
                MissionStatus::Running | MissionStatus::Planning | MissionStatus::Planned
            )
            .then(|| m.mission.id.clone())
        });
        let Some(client) = self.require_online() else {
            return;
        };
        match steer_target {
            Some(mission_id) => {
                self.spawn_msg(async move {
                    match client
                        .steer(
                            &mission_id,
                            &SteerRequest {
                                text,
                                task_id: None,
                            },
                        )
                        .await
                    {
                        Ok(()) => AppMsg::OpOk("steering directive sent"),
                        Err(e) => AppMsg::OpErr(format!("steer: {e}")),
                    }
                });
            }
            None => {
                self.state.waiting_chat = true;
                let session = self.state.session_id.clone();
                self.spawn_msg(async move {
                    match client.chat_send(&session, &text).await {
                        Ok(resp) => AppMsg::Chat(Box::new(resp)),
                        Err(e) => AppMsg::OpErr(format!("chat: {e}")),
                    }
                });
            }
        }
    }

    fn run_command(&mut self, cmd: SlashCommand, args: &str) {
        let with_images = if matches!(
            cmd,
            SlashCommand::Ask | SlashCommand::Run | SlashCommand::Steer
        ) && !args.is_empty()
        {
            self.with_images(args.to_string())
        } else {
            args.to_string()
        };
        let args = with_images.as_str();
        match cmd {
            SlashCommand::Preview => {
                if !args.is_empty() && args != "stop" {
                    self.show_toast("usage: /preview [stop]");
                    return;
                }
                let Some(client) = self.require_online() else {
                    return;
                };
                let stop = args == "stop";
                self.spawn_msg(async move {
                    match client.previews(stop).await {
                        Ok(v) => AppMsg::Previews(v),
                        Err(e) => AppMsg::OpErr(format!("preview: {e}")),
                    }
                });
            }
            SlashCommand::Attach => {
                if args == "clear" {
                    self.pending_images.clear();
                    self.state.system_line("Attachments cleared");
                    return;
                }
                if args.is_empty() {
                    self.show_toast("usage: /attach <image-path>");
                    return;
                }
                if self.state.waiting_chat {
                    self.show_toast(
                        "Wait for the current chat response before attaching another image",
                    );
                    return;
                }
                if self.pending_images.len() >= 4 {
                    self.show_toast("Attach at most four images");
                    return;
                }
                let Some(client) = self.require_online() else {
                    return;
                };
                let path = args.trim_matches('"').trim_matches('\'').to_string();
                self.spawn_msg(async move {
                    match client.upload_image(&path).await {
                        Ok(v) => AppMsg::ImageAttached {
                            id: v["imageId"].as_str().unwrap_or_default().into(),
                            name: v["name"].as_str().unwrap_or("image").into(),
                        },
                        Err(e) => AppMsg::OpErr(format!("attach: {e}")),
                    }
                });
            }
            SlashCommand::History => {
                let Some(client) = self.require_online() else {
                    return;
                };
                let args = args.to_string();
                self.spawn_msg(async move {
                    if let Some(label) = args.strip_prefix("save ") {
                        match client.save_version(label).await {
                            Ok(v) => AppMsg::VersionSaved(v),
                            Err(e) => AppMsg::OpErr(e.to_string()),
                        }
                    } else if let Some(id) = args.strip_prefix("diff ") {
                        match client.version_diff(id).await {
                            Ok(unified) => AppMsg::VersionDiff {
                                id: id.into(),
                                unified,
                            },
                            Err(e) => AppMsg::OpErr(e.to_string()),
                        }
                    } else {
                        match client.code_history().await {
                            Ok(h) => AppMsg::CodeHistory(h),
                            Err(e) => AppMsg::OpErr(e.to_string()),
                        }
                    }
                });
            }
            SlashCommand::Undo => self.request_restore_preview(None),
            SlashCommand::Restore => {
                if args.trim().is_empty() {
                    self.show_toast("usage: /restore <version>");
                } else {
                    self.request_restore_preview(Some(args.trim().to_string()));
                }
            }
            SlashCommand::Help => self.overlay = Some(Overlay::Help { scroll: 0 }),
            SlashCommand::Quit => self.quit = true,
            SlashCommand::New => {
                self.pending_images.clear();
                self.state.session_id = new_id();
                self.state.transcript.clear();
                self.transcript_epoch += 1;
                self.state.active_mission = None;
                self.state.system_line("new session");
            }
            SlashCommand::Settings => self.show_settings(),
            SlashCommand::Roles => {
                let Some(client) = self.require_online() else {
                    return;
                };
                self.spawn_msg(async move {
                    match client.models().await {
                        Ok(resp) => AppMsg::Roles(resp.roles),
                        Err(e) => AppMsg::OpErr(format!("roles: {e}")),
                    }
                });
            }
            SlashCommand::Sessions => {
                if self.require_online().is_none() {
                    return;
                }
                self.fetch_sessions(true);
            }
            SlashCommand::Ask => {
                if args.is_empty() {
                    self.show_toast("usage: /ask <question>");
                    return;
                }
                let text = args.to_string();
                let Some(client) = self.require_online() else {
                    return;
                };
                self.state.waiting_chat = true;
                let session = self.state.session_id.clone();
                self.spawn_msg(async move {
                    match client.chat_send(&session, &text).await {
                        Ok(resp) => AppMsg::Chat(Box::new(resp)),
                        Err(e) => AppMsg::OpErr(format!("chat: {e}")),
                    }
                });
            }
            SlashCommand::Run => {
                if args.is_empty() {
                    self.show_toast("usage: /run <objective>");
                    return;
                }
                let objective = args.to_string();
                let Some(client) = self.require_online() else {
                    return;
                };
                let session = self.state.session_id.clone();
                self.spawn_msg(async move {
                    match client.submit_mission(&objective, Some(&session)).await {
                        Ok(r) => AppMsg::Submitted {
                            mission_id: r.mission_id,
                        },
                        Err(e) => AppMsg::OpErr(format!("submit: {e}")),
                    }
                });
            }
            SlashCommand::Steer => {
                if args.is_empty() {
                    self.show_toast("usage: /steer <text>");
                    return;
                }
                let Some(mission) = self.state.active_mission.clone() else {
                    self.show_toast("no active mission");
                    return;
                };
                let Some(client) = self.require_online() else {
                    return;
                };
                let text = args.to_string();
                self.spawn_msg(async move {
                    match client
                        .steer(
                            &mission,
                            &SteerRequest {
                                text,
                                task_id: None,
                            },
                        )
                        .await
                    {
                        Ok(()) => AppMsg::OpOk("steering directive sent"),
                        Err(e) => AppMsg::OpErr(format!("steer: {e}")),
                    }
                });
            }
            SlashCommand::Plan => match args {
                "approve" => self.decide_plan(true),
                "reject" => self.decide_plan(false),
                _ => {
                    let Some(mission) = self
                        .state
                        .pending_plan
                        .clone()
                        .or_else(|| self.state.active_mission.clone())
                    else {
                        self.show_toast("no mission with a plan");
                        return;
                    };
                    if self.state.plans.contains_key(&mission) {
                        let entry = EntryKind::PlanCard {
                            mission_id: mission,
                        };
                        self.state.push_entry(entry, String::new());
                    } else {
                        self.fetch_mission(mission);
                    }
                }
            },
            SlashCommand::Approve => self.decide_approval_by_arg(args, true),
            SlashCommand::Reject => self.decide_approval_by_arg(args, false),
            SlashCommand::Status => {
                self.fetch_snapshot();
                let daemon = match self.state.connectivity {
                    Connectivity::Connected => "connected",
                    Connectivity::Connecting => "reconnecting",
                    Connectivity::StartingDaemon => "starting",
                    Connectivity::Offline => "offline",
                };
                let missions = self.state.missions.len();
                let approvals = self.state.approvals.len();
                let active = self
                    .state
                    .active_summary()
                    .map(|m| format!("{} ({})", m.mission.objective, m.mission.status.as_str()))
                    .unwrap_or_else(|| "none".into());
                self.state.system_line(format!(
                    "daemon {daemon} · {missions} missions · {approvals} pending approvals · active: {active}"
                ));
            }
            SlashCommand::Models => self.open_models(ModelsIntent::Browse),
            SlashCommand::Model => {
                // Forms: `/model` (pick model, then role), `/model coder`
                // (role fixed, pick model), `/model coder prov/model`
                // (direct assignment, no picker).
                let mut parts = args.split_whitespace();
                let role_arg = parts.next().unwrap_or("");
                let target = parts.next().unwrap_or("");
                let role = Role::parse(role_arg);
                if !role_arg.is_empty() && role.is_none() {
                    self.show_toast(
                        "usage: /model [planner|designer|coder|reviewer|security] [provider/model]",
                    );
                    return;
                }
                if let (Some(role), false) = (role, target.is_empty()) {
                    match target.split_once('/') {
                        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
                            self.apply_role_model(role, provider.to_string(), model.to_string());
                        }
                        _ => self.show_toast("model must be provider/model (see /models)"),
                    }
                    return;
                }
                self.open_models(ModelsIntent::Assign(role));
            }
            SlashCommand::Connect => {
                let provider = args.trim();
                if provider.is_empty() {
                    self.show_toast("usage: /connect <providerId>");
                    return;
                }
                self.pending_connect = Some(provider.to_string());
                self.composer.clear();
                self.composer.masked = true;
                self.state.system_line(format!(
                    "enter API key for {provider} (enter saves · esc cancels)"
                ));
            }
        }
    }

    fn request_restore_preview(&mut self, id: Option<String>) {
        let Some(client) = self.require_online() else {
            return;
        };
        self.spawn_msg(async move {
            match client.preview_restore(id.as_deref()).await {
                Ok(preview) => AppMsg::VersionPreview {
                    preview,
                    undo: id.is_none(),
                },
                Err(e) => AppMsg::OpErr(e.to_string()),
            }
        });
    }

    fn history_key(&mut self, key: KeyEvent) {
        let Some(Overlay::History(view)) = self.overlay.as_mut() else {
            return;
        };
        if key.code == KeyCode::Esc {
            self.overlay = None;
            self.dirty = true;
            return;
        }
        if view.busy {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if view.detail.is_some() {
                    view.scroll = view.scroll.saturating_sub(1)
                } else {
                    view.selected = view.selected.saturating_sub(1)
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if view.detail.is_some() {
                    view.scroll = view.scroll.saturating_add(1).min(view.scroll_max.get())
                } else {
                    view.selected =
                        (view.selected + 1).min(view.history.versions.len().saturating_sub(1))
                }
            }
            KeyCode::PageUp => {
                if view.detail.is_some() {
                    view.scroll = view.scroll.saturating_sub(10)
                } else {
                    view.selected = view.selected.saturating_sub(10)
                }
            }
            KeyCode::PageDown => {
                if view.detail.is_some() {
                    view.scroll = view.scroll.saturating_add(10).min(view.scroll_max.get())
                } else {
                    view.selected =
                        (view.selected + 10).min(view.history.versions.len().saturating_sub(1))
                }
            }
            KeyCode::Char('u') if view.detail.is_none() => self.request_restore_preview(None),
            KeyCode::Char('r') if view.detail.is_none() => {
                let id = view
                    .history
                    .versions
                    .get(view.selected)
                    .map(|v| v.id.clone());
                if let Some(id) = id {
                    self.request_restore_preview(Some(id));
                }
            }
            KeyCode::Enter => {
                if let Some(preview) = &view.preview {
                    if !preview.conflicts.is_empty() {
                        return;
                    }
                    let id = if view.undo {
                        None
                    } else {
                        Some(preview.target.id.clone())
                    };
                    let token = preview.token.clone();
                    view.busy = true;
                    let Some(client) = self.require_online() else {
                        return;
                    };
                    self.spawn_msg(async move {
                        match client.restore_version(id.as_deref(), &token).await {
                            Ok(r) => AppMsg::VersionRestored(r),
                            Err(e) => AppMsg::OpErr(e.to_string()),
                        }
                    });
                } else if view.detail.is_none() {
                    if let Some(v) = view.history.versions.get(view.selected) {
                        let id = v.id.clone();
                        let title = format!("{}\n{} · {}", v.label, v.id, v.kind);
                        let Some(client) = self.require_online() else {
                            return;
                        };
                        self.spawn_msg(async move {
                            match client.version_diff(&id).await {
                                Ok(unified) => AppMsg::VersionDiff { id: title, unified },
                                Err(e) => AppMsg::OpErr(e.to_string()),
                            }
                        });
                    }
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn open_models(&mut self, intent: ModelsIntent) {
        if let Some(discovery) = self.state.models.clone() {
            self.on_msg(AppMsg::Models { discovery, intent });
            return;
        }
        let Some(client) = self.require_online() else {
            return;
        };
        self.spawn_msg(async move {
            match client.models().await {
                Ok(m) => AppMsg::Models {
                    discovery: m.discovery,
                    intent,
                },
                Err(e) => AppMsg::OpErr(format!("models: {e}")),
            }
        });
    }

    fn show_settings(&mut self) {
        let theme_path = crate::theme::config_file("theme.json")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.config/maestro/theme.json".into());
        let keys_path = crate::theme::config_file("keybindings.json")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.config/maestro/keybindings.json".into());
        let lines = [
            format!("config      {}", self.root.join("maestro.toml").display()),
            format!(
                "credentials {}",
                self.root.join(".maestro/credentials.json").display()
            ),
            format!("theme       {theme_path}"),
            format!("keybindings {keys_path}"),
        ];
        for l in lines {
            self.state.system_line(l);
        }
    }

    fn decide_plan(&mut self, approve: bool) {
        let Some(mission) = self.state.pending_plan.clone() else {
            self.show_toast("no plan awaiting approval");
            return;
        };
        let Some(client) = self.require_online() else {
            return;
        };
        self.spawn_msg(async move {
            match client
                .plan_decide(
                    &mission,
                    &PlanDecisionRequest {
                        approve,
                        edited_plan: None,
                    },
                )
                .await
            {
                Ok(()) => AppMsg::OpOk(if approve {
                    "plan approved"
                } else {
                    "plan rejected"
                }),
                Err(e) => AppMsg::OpErr(format!("plan: {e}")),
            }
        });
    }

    fn decide_selected_approval(&mut self, approve: bool) {
        let Some(approval) = self
            .state
            .approvals
            .get(if self.tab == Tab::Approvals {
                self.approvals_view.selected
            } else {
                0
            })
            .map(|a| a.id.clone())
        else {
            self.show_toast("no pending approvals");
            return;
        };
        self.decide_approval(approval, approve);
    }

    fn decide_approval_by_arg(&mut self, args: &str, approve: bool) {
        let id = if args.trim().is_empty() {
            match self.state.approvals.first() {
                Some(a) => a.id.clone(),
                None => {
                    self.show_toast("no pending approvals");
                    return;
                }
            }
        } else {
            args.trim().to_string()
        };
        self.decide_approval(id, approve);
    }

    fn decide_approval(&mut self, id: String, approve: bool) {
        let Some(client) = self.require_online() else {
            return;
        };
        self.spawn_msg(async move {
            match client
                .approval_decide(
                    &id,
                    &ApprovalDecisionRequest {
                        approve,
                        note: None,
                    },
                )
                .await
            {
                Ok(()) => AppMsg::OpOk(if approve { "approved" } else { "rejected" }),
                Err(e) => AppMsg::OpErr(format!("approval: {e}")),
            }
        });
    }

    fn retry_selected(&mut self) {
        let Some(task) = self.tasks_view.selected_task(&self.state) else {
            self.show_toast("no task selected");
            return;
        };
        let (mission, task_id) = (task.mission_id.clone(), task.id.clone());
        let Some(client) = self.require_online() else {
            return;
        };
        self.spawn_msg(async move {
            match client
                .retry(
                    &mission,
                    &RetryRequest {
                        task_id,
                        model_override: None,
                    },
                )
                .await
            {
                Ok(()) => AppMsg::OpOk("retry queued"),
                Err(e) => AppMsg::OpErr(format!("retry: {e}")),
            }
        });
    }

    // ---- draw -----------------------------------------------------------------

    fn mode(&self) -> Mode {
        if self.state.pending_plan.is_some() || !self.state.approvals.is_empty() {
            Mode::Approval
        } else if self
            .state
            .active_summary()
            .map(|m| m.mission.status.is_active())
            .unwrap_or(false)
        {
            Mode::Mission
        } else {
            Mode::Chat
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.tab {
            // Permanent key bar (keelcode rule: a shortcut you cannot see is
            // a shortcut you do not use). Fixed order; statusline drops from
            // the tail under width pressure.
            Tab::Chat => vec![
                ("/", "cmds"),
                ("↵", "send"),
                ("^j", "newline"),
                ("^p", "palette"),
                ("1-5", "tabs"),
                ("^b", "sidebar"),
            ],
            Tab::Tasks if self.tasks_view.detail_open => {
                vec![("j/k", "scroll"), ("PgUp/Dn", "page"), ("esc", "back")]
            }
            Tab::Tasks => vec![
                ("j/k", "select"),
                ("↵", "detail"),
                ("r", "retry"),
                ("d", "diff"),
            ],
            Tab::Diff => vec![("j/k", "scroll"), ("esc", "chat")],
            Tab::Approvals => vec![("a", "approve"), ("x", "reject"), ("v", "diff")],
            Tab::Events => vec![("j/k", "scroll"), ("end", "follow")],
        }
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        self.tasks_detail_width = area.width;
        let composer_h = if self.tab == Tab::Chat && area.height > 8 {
            self.composer.rows(area.width) + 1
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(composer_h),
                Constraint::Length(1),
            ])
            .split(area);
        let (header_area, main_area, composer_area, status_area) =
            (chunks[0], chunks[1], chunks[2], chunks[3]);
        self.page_height = main_area.height.saturating_sub(1).max(1);

        // Header.
        let cwd_tail = tail_path(&self.root);
        let root = &self.root;
        let branch = self.branch.get(Instant::now(), || git_branch(root));
        let mission_status = self
            .state
            .active_summary()
            .map(|m| m.mission.status.as_str().to_string());
        let model = self.state.models.as_ref().and_then(|d| {
            d.models
                .first()
                .map(|m| format!("{}/{}", m.provider_id, m.model))
        });
        let segments = ui::header::build_segments(
            "maestro",
            &cwd_tail,
            branch.as_deref(),
            mission_status.as_deref(),
            model.as_deref(),
        );
        ui::header::render(frame, header_area, &self.theme, &segments);

        // Main area per tab.
        match self.tab {
            Tab::Chat => self.draw_chat(frame, main_area),
            Tab::Tasks => {
                ui::tasks::render(frame, main_area, &self.theme, &self.state, &self.tasks_view)
            }
            Tab::Diff => {
                // Borrow the cached diff — never clone it per frame.
                let diff = self.diff_target().and_then(|m| self.state.diffs.get(&m));
                ui::diff::render(
                    frame,
                    main_area,
                    &self.theme,
                    diff,
                    self.diff_scroll,
                    self.diff_loading,
                );
            }
            Tab::Approvals => ui::approvals::render(
                frame,
                main_area,
                &self.theme,
                &self.state,
                &self.approvals_view,
            ),
            Tab::Events => ui::events::render(
                frame,
                main_area,
                &self.theme,
                &self.state,
                self.events_scroll,
            ),
        }

        // Composer (chat only).
        if composer_h > 0 {
            let placeholder = if self.pending_connect.is_some() {
                "API key (masked)"
            } else {
                "what should we build?  (/ for commands)"
            };
            self.composer
                .render(frame, composer_area, &self.theme, placeholder, true);

            // Slash autocomplete popup / arg ghost hint, anchored above the
            // composer, painted over the transcript. Overlays win the space.
            if self.overlay.is_none() && self.pending_connect.is_none() {
                let token = (!self.suggest_dismissed)
                    .then(|| crate::commands::slash_token(self.composer.text()))
                    .flatten()
                    .map(str::to_string);
                if let Some(token) = token {
                    let sugg = self.suggestions();
                    let h = ui::suggest::popup_height(sugg.len()).min(main_area.height);
                    let pop = Rect {
                        x: main_area.x,
                        y: composer_area.y.saturating_sub(h),
                        width: main_area.width,
                        height: h,
                    };
                    let sel = self.suggest_sel.min(sugg.len().saturating_sub(1));
                    ui::suggest::render_popup(frame, pop, &self.theme, &sugg, sel, &token);
                } else if let Some((cmd, hint)) = arg_hint_for(self.composer.text()) {
                    if main_area.height > 0 {
                        let pop = Rect {
                            x: main_area.x,
                            y: composer_area.y.saturating_sub(1),
                            width: main_area.width,
                            height: 1,
                        };
                        ui::suggest::render_arg_hint(frame, pop, &self.theme, cmd, hint);
                    }
                }
            }
        }

        // Status line.
        let stats = self.state.active_summary().map(|m| m.stats.clone());
        let elapsed = self.state.active_summary().and_then(|m| {
            m.mission
                .status
                .is_active()
                .then(|| (now_ms() - m.mission.created_at) / 1000)
        });
        let toast_text = self.toast.as_ref().map(|(t, _)| t.clone()).or_else(|| {
            matches!(self.state.connectivity, Connectivity::StartingDaemon)
                .then(|| "starting daemon…".to_string())
        });
        let hints = self.hints();
        let input = StatusInput {
            mode: self.mode(),
            hints: &hints,
            pending_approvals: self.state.pending_approvals(),
            daemon: self.state.connectivity,
            tokens: stats.as_ref().map(|s| s.total_tokens()).unwrap_or(0),
            cost_usd: stats.as_ref().map(|s| s.cost_usd()).unwrap_or(0.0),
            elapsed_secs: elapsed,
            toast: toast_text.as_deref(),
        };
        ui::statusline::render(frame, status_area, &self.theme, &input);

        // Overlay last.
        if let Some(overlay) = &self.overlay {
            ui::overlay::render(frame, area, &self.theme, overlay, &self.keymap);
        }
    }

    fn draw_chat(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let show_sidebar = self.sidebar_on && area.width >= ui::sidebar::AUTO_HIDE_BELOW;
        let (chat_area, sidebar_area) = if show_sidebar {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(20),
                    Constraint::Length(ui::sidebar::SIDEBAR_WIDTH),
                ])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // Until the first real turn commits, the chat area is the welcome
        // screen (static: it teaches the composer instead of describing it).
        let pristine = self
            .state
            .transcript
            .iter()
            .all(|e| e.kind == EntryKind::System)
            && self.state.missions.is_empty();
        if pristine {
            match self.state.connectivity {
                // Startup: the UI arrives, it doesn't pop.
                Connectivity::Connecting | Connectivity::StartingDaemon => {
                    let phase = if self.state.connectivity == Connectivity::StartingDaemon {
                        "starting the daemon…"
                    } else {
                        "connecting…"
                    };
                    ui::welcome::render_loading(
                        frame,
                        chat_area,
                        &self.theme,
                        ui::spinner(self.spinner_frame),
                        phase,
                    );
                }
                _ => {
                    ui::welcome::render(
                        frame,
                        chat_area,
                        &self.theme,
                        &ui::welcome::WelcomeInput {
                            mock_roles: &self.state.mock_roles,
                            sessions: &self.recent_sessions,
                            session_sel: self.welcome_sel,
                        },
                    );
                }
            }
        } else {
            let previous_lines = self.transcript.total_lines();
            self.transcript.sync(
                &self.state,
                &self.theme,
                chat_area.width,
                self.transcript_epoch,
                self.tool_expanded,
            );
            let tail = ui::transcript::tail_lines(
                &self.state,
                &self.theme,
                chat_area.width,
                self.spinner_frame,
            );
            if self.chat_scroll > 0 {
                // Keep the viewed history still while new entries arrive.
                self.chat_scroll = self
                    .chat_scroll
                    .saturating_add(self.transcript.total_lines().saturating_sub(previous_lines));
            }
            self.chat_max_scroll = (self.transcript.total_lines() + tail.len())
                .saturating_sub(chat_area.height as usize);
            self.chat_scroll = self.chat_scroll.min(self.chat_max_scroll);
            self.transcript
                .render(frame, chat_area, &self.theme, &tail, self.chat_scroll);
        }

        if let Some(sidebar_area) = sidebar_area {
            ui::sidebar::render(
                frame,
                sidebar_area,
                &self.theme,
                &self.state,
                self.spinner_frame,
            );
        }
    }
}

fn tail_path(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        s
    } else {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    }
}

/// Best-effort branch name from `.git/HEAD` (no git subprocess).
fn git_branch(root: &std::path::Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).ok()?;
    let head = head.trim();
    if let Some(rest) = head.strip_prefix("ref: refs/heads/") {
        Some(rest.to_string())
    } else {
        Some(head.chars().take(8).collect())
    }
}

/// TTL cache for the header's git branch so the draw path doesn't hit the
/// filesystem every frame. Also invalidated on daemon reconnect.
#[derive(Default)]
struct BranchCache {
    value: Option<String>,
    read_at: Option<Instant>,
}

impl BranchCache {
    const TTL: Duration = Duration::from_secs(2);

    /// Return the cached branch, invoking `read` only when the entry is
    /// missing or older than [`Self::TTL`].
    fn get(&mut self, now: Instant, read: impl FnOnce() -> Option<String>) -> Option<String> {
        let fresh = self
            .read_at
            .is_some_and(|at| now.duration_since(at) < Self::TTL);
        if !fresh {
            self.value = read();
            self.read_at = Some(now);
        }
        self.value.clone()
    }

    /// Force a re-read on the next `get` (e.g. daemon reconnect).
    fn invalidate(&mut self) {
        self.read_at = None;
    }
}

/// Main loop: see module docs. Terminal setup/teardown lives in `run_tui`.
pub(crate) async fn run(opts: TuiOptions) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AppMsg>(512);
    let mut app = App::new(opts, tx);
    app.startup();

    let mut guard = crate::term::TermGuard::enter()?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal =
        ratatui::Terminal::new(backend).map_err(|e| MaestroError::Io(format!("terminal: {e}")))?;
    let mut input = crossterm::event::EventStream::new();

    while !app.quit {
        let frame_deadline =
            tokio::time::Instant::now() + FRAME.saturating_sub(app.last_draw.elapsed());
        tokio::select! {
            maybe = input.next() => {
                match maybe {
                    Some(Ok(ev)) => app.on_input(ev),
                    Some(Err(_)) => {}
                    None => break,
                }
            }
            maybe = rx.recv() => {
                match maybe {
                    Some(msg) => app.on_msg(msg),
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(frame_deadline), if app.dirty => {}
        }
        // Coalesce whatever else is already queued before drawing.
        while let Ok(msg) = rx.try_recv() {
            app.on_msg(msg);
        }
        if app.dirty && app.last_draw.elapsed() >= FRAME {
            terminal
                .draw(|f| app.draw(f))
                .map_err(|e| MaestroError::Io(format!("draw: {e}")))?;
            app.dirty = false;
            app.last_draw = Instant::now();
        }
    }

    app.shutdown_tasks();
    guard.restore();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_cache_reads_at_most_once_per_ttl() {
        let mut cache = BranchCache::default();
        let mut reads = 0u32;
        let t0 = Instant::now();

        // First get performs the read.
        let v = cache.get(t0, || {
            reads += 1;
            Some("main".into())
        });
        assert_eq!(v.as_deref(), Some("main"));
        assert_eq!(reads, 1);

        // Within the TTL: cached, no read even if the value changed on disk.
        let v = cache.get(t0 + Duration::from_millis(500), || {
            reads += 1;
            Some("other".into())
        });
        assert_eq!(v.as_deref(), Some("main"));
        assert_eq!(reads, 1);

        // Past the TTL: re-read.
        let v = cache.get(t0 + BranchCache::TTL + Duration::from_millis(1), || {
            reads += 1;
            Some("other".into())
        });
        assert_eq!(v.as_deref(), Some("other"));
        assert_eq!(reads, 2);

        // Invalidate (daemon reconnect) forces an immediate re-read.
        cache.invalidate();
        let v = cache.get(t0 + BranchCache::TTL + Duration::from_millis(2), || {
            reads += 1;
            None
        });
        assert_eq!(v, None);
        assert_eq!(reads, 3);

        // A `None` result is cached too (no per-frame retry on missing .git).
        let _ = cache.get(t0 + BranchCache::TTL + Duration::from_millis(3), || {
            reads += 1;
            Some("main".into())
        });
        assert_eq!(reads, 3);
    }
}
