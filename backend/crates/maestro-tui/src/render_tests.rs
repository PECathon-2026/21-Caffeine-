//! TestBackend snapshot-ish tests: no real terminal, no processes.

use crate::state::{CachedDiff, ClientState, Connectivity, EntryKind};
use crate::theme::Theme;
use crate::ui;
use crate::ui::statusline::{Mode, StatusInput};
use crate::ui::transcript::TranscriptView;
use maestro_types::domain::{
    Approval, ApprovalStatus, MissionStatus, RiskLevel, Role, Task, TaskStatus,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

fn term(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(width, height)).unwrap()
}

fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buf = terminal.backend().buffer();
    let area = *buf.area();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn header_segments() -> Vec<String> {
    ui::header::build_segments(
        "maestro",
        "~/projects/maestro-rs",
        Some("main*"),
        Some("running"),
        Some("anthropic/claude-sonnet-4-20250514"),
    )
}

#[test]
fn header_render_sheds_at_narrow_widths() {
    // 120: everything visible.
    let mut t = term(120, 1);
    let segs = header_segments();
    t.draw(|f| ui::header::render(f, f.area(), &Theme::dark(), &segs))
        .unwrap();
    let line = rows(&t).remove(0);
    assert!(line.contains("maestro"));
    assert!(line.contains("claude-sonnet"));
    assert!(line.contains("running"));

    // 80: model shed, brand + cwd retained.
    let mut t = term(80, 1);
    t.draw(|f| ui::header::render(f, f.area(), &Theme::dark(), &segs))
        .unwrap();
    let line = rows(&t).remove(0);
    assert!(line.contains("maestro"));
    assert!(line.contains("maestro-rs"));
    assert!(!line.contains("claude-sonnet"));

    // 40: only the leftmost segments survive; never wraps.
    let mut t = term(40, 1);
    t.draw(|f| ui::header::render(f, f.area(), &Theme::dark(), &segs))
        .unwrap();
    let line = rows(&t).remove(0);
    assert!(line.starts_with("maestro"));
    assert!(!line.contains("running"));
}

#[test]
fn status_line_sheds_hints_but_keeps_mode_and_dot() {
    let hints = [("^p", "commands"), ("1-5", "tabs")];
    let input = StatusInput {
        mode: Mode::Mission,
        hints: &hints,
        pending_approvals: 3,
        daemon: Connectivity::Connected,
        tokens: 42_000,
        cost_usd: 0.55,
        elapsed_secs: Some(75),
        toast: None,
    };
    let mut t = term(110, 1);
    t.draw(|f| ui::statusline::render(f, f.area(), &Theme::dark(), &input))
        .unwrap();
    let line = rows(&t).remove(0);
    assert!(line.contains("MISSION"));
    assert!(line.contains("commands"));
    assert!(line.contains("⚠ 3"));
    assert!(line.contains("42.0k tok"));
    assert!(line.contains("$0.55"));
    assert!(line.contains("1m15s"));
    assert!(line.contains("●"));

    let mut t = term(36, 1);
    t.draw(|f| ui::statusline::render(f, f.area(), &Theme::dark(), &input))
        .unwrap();
    let line = rows(&t).remove(0);
    assert!(line.contains("MISSION"));
    assert!(line.contains("●"));
    assert!(line.contains("⚠ 3"));
    assert!(!line.contains("commands"));
}

#[test]
fn transcript_renders_gutter_glyphs() {
    let mut state = ClientState::new("s".into());
    state.push_entry(EntryKind::User, "build the widget");
    state.push_entry(EntryKind::Assistant, "on it");
    state.push_entry(EntryKind::Tool, "read_file {\"path\":\"src/x.rs\"}");
    state.push_entry(EntryKind::ToolResult, "fn main() {}");
    state.push_entry(EntryKind::System, "mission completed");
    let theme = Theme::dark();
    let mut view = TranscriptView::default();
    let mut t = term(60, 8);
    t.draw(|f| {
        view.sync(&state, &theme, 60, 0, false);
        view.render(f, f.area(), &theme, &[], 0);
    })
    .unwrap();
    let lines = rows(&t);
    let gutters: Vec<char> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().next().unwrap())
        .collect();
    assert_eq!(gutters, vec!['❯', '◆', '⏵', '⎿', '·']);
}

fn approval() -> Approval {
    Approval {
        id: "ap1".into(),
        mission_id: "m1".into(),
        task_id: None,
        action: "integrate".into(),
        resource: "mission".into(),
        patch_hash: None,
        policy_snapshot: serde_json::json!({"reason": "final approval required"}),
        status: ApprovalStatus::Pending,
        expires_at: None,
        decided_at: None,
        decided_by: None,
        note: None,
        created_at: 1,
    }
}

#[test]
fn approval_card_renders_in_approvals_tab() {
    let mut state = ClientState::new("s".into());
    state.approvals.push(approval());
    let view = ui::approvals::ApprovalsView::default();
    let mut t = term(70, 10);
    t.draw(|f| ui::approvals::render(f, f.area(), &Theme::dark(), &state, &view))
        .unwrap();
    let text = rows(&t).join("\n");
    assert!(text.contains("◆ Approval"));
    assert!(text.contains("integrate"));
    assert!(text.contains("final approval required"));
    assert!(text.contains("[a] approve"));
    assert!(text.contains("[x] reject"));
    assert!(text.contains("[v] view diff"));
}

fn task(id: &str, status: TaskStatus, role: Role) -> Task {
    Task {
        id: id.into(),
        mission_id: "m1".into(),
        task_key: format!("task-{id}"),
        role,
        title: format!("Task {id}"),
        description: String::new(),
        status,
        dependencies: vec![],
        acceptance: vec![],
        risk: RiskLevel::Low,
        attempts: 1,
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
        created_at: 1,
        updated_at: 1,
    }
}

#[test]
fn task_list_renders_status_glyphs() {
    let mut state = ClientState::new("s".into());
    state
        .missions
        .push(crate::state::test_summary("m1", MissionStatus::Running));
    state.active_mission = Some("m1".into());
    state.tasks.insert(
        "m1".into(),
        vec![
            task("a", TaskStatus::Completed, Role::Coder),
            task("b", TaskStatus::Failed, Role::Reviewer),
            task("c", TaskStatus::Running, Role::Coder),
            task("d", TaskStatus::Ready, Role::Security),
            task("e", TaskStatus::Blocked, Role::Designer),
        ],
    );
    let view = ui::tasks::TasksView::default();
    let mut t = term(60, 8);
    t.draw(|f| ui::tasks::render(f, f.area(), &Theme::dark(), &state, &view))
        .unwrap();
    let lines = rows(&t);
    let glyphs: Vec<char> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().next().unwrap())
        .collect();
    assert_eq!(glyphs, vec!['✓', '✗', '▸', '○', '⊘']);
    // Role glyphs present too.
    assert!(lines[0].contains("侍 "));
    assert!(lines[1].contains("守 "));
}

#[test]
fn sidebar_team_panel_shows_running_tasks_with_spinner() {
    let mut state = ClientState::new("s".into());
    state
        .missions
        .push(crate::state::test_summary("m1", MissionStatus::Running));
    state.active_mission = Some("m1".into());
    state.tasks.insert(
        "m1".into(),
        vec![task("c", TaskStatus::Running, Role::Coder)],
    );
    let mut t = term(40, 10);
    t.draw(|f| ui::sidebar::render(f, f.area(), &Theme::dark(), &state, 0))
        .unwrap();
    let text = rows(&t).join("\n");
    assert!(text.contains("team"));
    assert!(text.contains("Task c"));
    assert!(text.contains(ui::SPINNER[0]));
    assert!(text.contains("tasks"));
}

#[test]
fn plan_card_renders_in_transcript() {
    let mut state = ClientState::new("s".into());
    state.active_mission = Some("m1".into());
    state.pending_plan = Some("m1".into());
    state.plans.insert(
        "m1".into(),
        maestro_types::plan::Plan {
            summary: "Do the thing".into(),
            tasks: vec![maestro_types::plan::PlanTask {
                key: "write-code".into(),
                role: Role::Coder,
                title: "Write code".into(),
                description: String::new(),
                dependencies: vec![],
                acceptance: vec![],
                risk: RiskLevel::Low,
                write_scopes: vec!["src/**".into()],
            }],
        },
    );
    state.push_entry(
        EntryKind::PlanCard {
            mission_id: "m1".into(),
        },
        String::new(),
    );
    let theme = Theme::dark();
    let mut view = TranscriptView::default();
    let mut t = term(70, 10);
    t.draw(|f| {
        view.sync(&state, &theme, 70, 0, false);
        view.render(f, f.area(), &theme, &[], 0);
    })
    .unwrap();
    let text = rows(&t).join("\n");
    assert!(text.contains("Plan approval"));
    assert!(text.contains("Do the thing"));
    assert!(text.contains("write-code"));
    assert!(text.contains("[a] approve"));
}

#[test]
fn diff_render_uses_row_tints(/* full-row backgrounds asserted in ui::diff */) {
    let unified = CachedDiff::new("@@ -1,2 +1,2 @@\n-old line\n+new line\n context".into());
    let mut t = term(50, 6);
    let theme = Theme::dark();
    t.draw(|f| ui::diff::render(f, f.area(), &theme, Some(&unified), 0, false))
        .unwrap();
    let buf = t.backend().buffer();
    // Row 1 = removed line: background must be diff_removed_bg across the row.
    assert_eq!(buf[(0u16, 1u16)].style().bg, Some(theme.diff_removed_bg));
    assert_eq!(buf[(45u16, 1u16)].style().bg, Some(theme.diff_removed_bg));
    // Row 2 = added line.
    assert_eq!(buf[(0u16, 2u16)].style().bg, Some(theme.diff_added_bg));
    let lines = rows(&t);
    assert!(lines[0].contains("@@"));
    assert!(lines[1].contains("-old line"));
    assert!(lines[2].contains("+new line"));
}

#[test]
fn events_tab_renders_ring_tail() {
    let mut state = ClientState::new("s".into());
    for i in 1..=5 {
        state.apply_event(&maestro_types::event::EventRecord {
            seq: i,
            mission_id: "m1".into(),
            task_id: None,
            kind: "task.status".into(),
            payload: serde_json::json!({"n": i}),
            created_at: 0,
        });
    }
    let mut t = term(60, 3);
    t.draw(|f| ui::events::render(f, f.area(), &Theme::dark(), &state, 0))
        .unwrap();
    let text = rows(&t).join("\n");
    // Follow mode shows the newest events.
    assert!(text.contains("5"));
    assert!(text.contains("task.status"));
}

// Guard: the Rect import is used implicitly by helpers above on some
// toolchains; keep the compiler honest.
#[allow(dead_code)]
fn _rect_used(r: Rect) -> u16 {
    r.width
}

// ---- slash autocomplete & welcome ------------------------------------------

#[test]
fn suggest_popup_lists_matches_and_marks_the_selection() {
    use crate::commands::slash_suggestions;
    let items = slash_suggestions("/st");
    assert!(items.len() >= 2);
    let mut t = term(90, 10);
    t.draw(|f| {
        ui::suggest::render_popup(f, f.area(), &Theme::dark(), &items, 1, "st");
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(all.contains("❯ /"), "selected row carries the run marker");
    assert!(all.contains("steer"));
    assert!(all.contains("status"));
    assert!(all.contains("tab complete"));
}

#[test]
fn suggest_popup_zero_matches_stays_open_with_an_answer() {
    let mut t = term(80, 4);
    t.draw(|f| {
        ui::suggest::render_popup(f, f.area(), &Theme::dark(), &[], 0, "zzz");
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(all.contains("no matches for /zzz"));
}

#[test]
fn suggest_popup_windows_long_lists_with_edge_markers() {
    use crate::commands::SlashCommand;
    let items: Vec<SlashCommand> = SlashCommand::ALL.to_vec();
    assert!(items.len() > ui::suggest::MAX_VISIBLE);
    let mut t = term(90, ui::suggest::popup_height(items.len()));
    // Select the last row: the window must scroll and show a top ↑ marker.
    t.draw(|f| {
        ui::suggest::render_popup(f, f.area(), &Theme::dark(), &items, items.len() - 1, "");
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(all.contains("↑ /"), "off-window rows above are signalled");
    assert!(all.contains("quit"), "the selected tail row is visible");
}

#[test]
fn arg_hint_renders_ghost_usage() {
    use crate::commands::SlashCommand;
    let mut t = term(80, 1);
    t.draw(|f| {
        ui::suggest::render_arg_hint(
            f,
            f.area(),
            &Theme::dark(),
            SlashCommand::Steer,
            "<directive>",
        );
    })
    .unwrap();
    let line = rows(&t).remove(0);
    assert!(line.contains("/steer"));
    assert!(line.contains("<directive>"));
    assert!(line.contains("steer the active mission"));
}

#[test]
fn welcome_screen_teaches_the_composer() {
    let mut t = term(100, 18);
    t.draw(|f| {
        ui::welcome::render(
            f,
            f.area(),
            &Theme::dark(),
            &ui::welcome::WelcomeInput {
                mock_roles: &[],
                sessions: &[],
                session_sel: None,
            },
        );
    })
    .unwrap();
    let all = rows(&t).join("\n");
    // The header owns the brand — the welcome must NOT repeat it.
    assert!(!all.contains("maestro"), "brand belongs to the header only");
    assert!(all.contains("multi-model mission control"));
    assert!(all.contains("plain words become a mission"));
    assert!(all.contains("/run <objective>"));
    assert!(all.contains("/help"));
}

#[test]
fn loading_screen_centers_brand_and_phase() {
    let mut t = term(100, 12);
    t.draw(|f| {
        ui::welcome::render_loading(f, f.area(), &Theme::dark(), "⠋", "starting the daemon…");
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(all.contains("maestro"));
    assert!(all.contains("starting the daemon…"));
}

#[test]
fn welcome_screen_shouts_about_mock_providers() {
    let mut t = term(110, 22);
    let roles = vec!["planner".to_string(), "coder".to_string()];
    t.draw(|f| {
        ui::welcome::render(
            f,
            f.area(),
            &Theme::dark(),
            &ui::welcome::WelcomeInput {
                mock_roles: &roles,
                sessions: &[],
                session_sel: None,
            },
        );
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(all.contains("mock providers"));
    assert!(all.contains("planner, coder"));
    assert!(all.contains("/connect"));
}

#[test]
fn assistant_entries_render_markdown_not_asterisks() {
    let state = ClientState::new("s".into());
    let entry = crate::state::TranscriptEntry {
        id: "a1".into(),
        kind: EntryKind::Assistant,
        text: "A few things:\n- **Understanding the codebase** — architecture\n- **Design decisions** — trade-offs".into(),
    };
    let lines = ui::transcript::entry_lines(&entry, &state, &Theme::dark(), 100, false);
    let text: String = lines
        .iter()
        .map(|l| l.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("**"), "asterisks must be consumed: {text}");
    assert!(text.contains("• Understanding the codebase"));
    assert!(
        text.contains("◆"),
        "gutter glyph preserved on the first line"
    );
    // The bold run carries a distinct style from its neighbours.
    let styled_line = &lines[1];
    let styles: Vec<_> = styled_line.iter().map(|s| s.style).collect();
    assert!(styles.windows(2).any(|w| w[0] != w[1]));
}

#[test]
fn welcome_lists_recent_sessions_with_a_caret() {
    use maestro_types::domain::ChatSession;
    let now = maestro_types::now_ms();
    let sessions = vec![
        ChatSession {
            id: "s1".into(),
            title: "Heya".into(),
            active_mission_id: None,
            created_at: now - 60_000,
            updated_at: now - 60_000,
        },
        ChatSession {
            id: "s2".into(),
            title: "plan the finance dashboard".into(),
            active_mission_id: None,
            created_at: now - 7_200_000,
            updated_at: now - 7_200_000,
        },
    ];
    let mut t = term(110, 26);
    t.draw(|f| {
        ui::welcome::render(
            f,
            f.area(),
            &Theme::dark(),
            &ui::welcome::WelcomeInput {
                mock_roles: &[],
                sessions: &sessions,
                session_sel: Some(1),
            },
        );
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(all.contains("recent sessions"));
    assert!(all.contains("Heya"));
    assert!(
        all.contains("❯ plan the finance dashboard"),
        "caret on the selected row"
    );
    assert!(all.contains("m ago") || all.contains("h ago"));
    assert!(all.contains("↵ resume"));
}

#[test]
fn overlay_windows_follow_the_selection() {
    use ui::overlay::{row_marker, window_start};
    // Selection past the window scrolls the start; edges get ↑/↓ markers.
    assert_eq!(window_start(0, 60, 10), 0);
    assert_eq!(window_start(9, 60, 10), 0);
    assert_eq!(window_start(10, 60, 10), 1, "selection stays visible");
    assert_eq!(window_start(59, 60, 10), 50);
    assert_eq!(window_start(3, 5, 10), 0, "short lists never scroll");
    // Scrolled window: selection rides the bottom edge, top edge shows ↑.
    let (start, vis, len) = (window_start(30, 60, 10), 10, 60);
    assert_eq!(start, 21);
    assert_eq!(row_marker(30, 30, start, vis, len), "❯ ");
    assert_eq!(row_marker(start, 30, start, vis, len), "↑ ");
    assert_eq!(row_marker(start + 1, 30, start, vis, len), "  ");
    // Unscrolled window with more rows below: bottom edge shows ↓.
    assert_eq!(row_marker(9, 3, 0, 10, 60), "↓ ");
}

#[test]
fn model_picker_scrolls_the_window_to_the_selection() {
    use maestro_types::provider::DiscoveredModel;
    let models: Vec<DiscoveredModel> = (0..60)
        .map(|i| DiscoveredModel {
            provider_id: "prov".into(),
            model: format!("model-{i:02}"),
            display_name: None,
            context_window: None,
            discovered: true,
        })
        .collect();
    let mut p = ui::overlay::PickerState::new(models, None, true);
    p.selected = 40;
    let mut t = term(90, 18);
    t.draw(|f| {
        ui::overlay::render(
            f,
            f.area(),
            &Theme::dark(),
            &ui::overlay::Overlay::ModelPicker(p),
            &crate::keymap::Keymap::load(),
        );
    })
    .unwrap();
    let all = rows(&t).join("\n");
    assert!(
        all.contains("❯ prov/model-40"),
        "selected row visible: {all}"
    );
    assert!(!all.contains("model-00"), "window scrolled past the top");
    assert!(all.contains("↑ "), "top edge marker shows more rows above");
    assert!(all.contains("60 models"));
}

#[test]
fn blocked_sidebar_keeps_team_models_and_explains_failure() {
    let mut state = ClientState::new("s".into());
    state.active_mission = Some("m1".into());
    state
        .missions
        .push(crate::state::test_summary("m1", MissionStatus::Blocked));
    state.role_models.push(maestro_types::api::RoleModelInfo {
        role: "reviewer".into(),
        provider: "foundry-kimi".into(),
        model: "Kimi-K2.7-Code".into(),
        fallbacks: vec![],
    });
    let mut task = task("t1", TaskStatus::Failed, Role::Reviewer);
    task.role = Role::Reviewer;
    task.status_reason = Some("repository inspection failed".into());
    state.tasks.insert("m1".into(), vec![task]);
    let mut t = term(40, 24);
    t.draw(|f| ui::sidebar::render(f, f.area(), &Theme::dark(), &state, 0))
        .unwrap();
    let screen = rows(&t).join("\n");
    assert!(screen.contains("blocked"));
    assert!(screen.contains("Kimi-K2.7-Code"));
    assert!(screen.contains("repository inspection failed"));
}

#[test]
fn transcript_overscroll_keeps_first_page_and_never_erases_history() {
    let mut state = ClientState::new("s".into());
    state.push_entry(EntryKind::User, "Explain this project");
    for i in 0..30 {
        state.push_entry(EntryKind::Assistant, format!("answer line {i}"));
    }
    let mut view = TranscriptView::default();
    view.sync(&state, &Theme::dark(), 60, 0, false);
    let mut t = term(60, 10);
    t.draw(|f| view.render(f, f.area(), &Theme::dark(), &[], usize::MAX))
        .unwrap();
    let screen = rows(&t);
    assert!(screen[0].contains("Explain this project"));
    assert!(screen[9].contains("answer line 8"));
    assert_eq!(screen.iter().filter(|r| !r.trim().is_empty()).count(), 10);
    t.draw(|f| view.render(f, f.area(), &Theme::dark(), &[], 0))
        .unwrap();
    let screen = rows(&t);
    assert!(
        screen[0].contains("Explain this project"),
        "user request stays pinned"
    );
    assert!(
        screen[9].contains("answer line 29"),
        "latest answer remains visible"
    );
    assert_eq!(t.backend().buffer()[(30, 0)].bg, Theme::dark().panel);
    assert_eq!(state.transcript.len(), 31);
    // Shrinking the viewport and toggling tool detail must still clamp safely.
    view.sync(&state, &Theme::dark(), 35, 1, true);
    let mut narrow = term(35, 5);
    narrow
        .draw(|f| view.render(f, f.area(), &Theme::dark(), &[], usize::MAX))
        .unwrap();
    assert!(rows(&narrow)[4].contains("answer line 3"));
}

#[test]
fn pinned_prompt_changes_with_the_visible_conversation_turn() {
    let mut state = ClientState::new("s".into());
    state.push_entry(EntryKind::User, "First question");
    for i in 0..20 {
        state.push_entry(EntryKind::Assistant, format!("first answer {i}"));
    }
    state.push_entry(EntryKind::User, "Second question");
    for i in 0..20 {
        state.push_entry(EntryKind::Assistant, format!("second answer {i}"));
    }
    let mut view = TranscriptView::default();
    view.sync(&state, &Theme::dark(), 50, 0, false);
    let mut t = term(50, 8);
    t.draw(|f| view.render(f, f.area(), &Theme::dark(), &[], 0))
        .unwrap();
    assert!(rows(&t)[0].contains("Second question"));
    t.draw(|f| view.render(f, f.area(), &Theme::dark(), &[], 23))
        .unwrap();
    assert!(rows(&t)[0].contains("First question"));
}

#[test]
fn sidebar_wraps_full_task_title_and_details_scroll_to_last_acceptance() {
    let mut state = ClientState::new("s".into());
    state.active_mission = Some("m1".into());
    let mut task = task("long", TaskStatus::Blocked, Role::Designer);
    task.title = "Inspect the application and define a responsive accessible redesign".into();
    task.description = (0..25)
        .map(|i| format!("Description paragraph {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    task.acceptance = vec!["Final acceptance criterion is readable".into()];
    task.status_reason = Some("Missing local configuration".into());
    state.tasks.insert("m1".into(), vec![task]);
    let mut t = term(40, 24);
    t.draw(|f| ui::sidebar::render(f, f.area(), &Theme::dark(), &state, 0))
        .unwrap();
    let screen = rows(&t).join("\n");
    assert!(screen.contains("accessible redesign"));
    let view = ui::tasks::TasksView {
        selected: 0,
        detail_open: true,
        detail_scroll: usize::MAX,
    };
    t.draw(|f| ui::tasks::render(f, f.area(), &Theme::dark(), &state, &view))
        .unwrap();
    let screen = rows(&t).join("\n");
    assert!(screen.contains("Final acceptance criterion is readable"));
    assert!(screen.contains("scroll"));
}

#[test]
fn originating_block_summary_wins_over_propagated_dependency_reason() {
    let mut state = ClientState::new("s".into());
    state.active_mission = Some("m1".into());
    let mut dependent = task("coder", TaskStatus::Blocked, Role::Coder);
    dependent.status_reason = Some("dependency failed or blocked".into());
    let mut designer = task("designer", TaskStatus::Blocked, Role::Designer);
    designer.summary = Some("Required framework guides are missing".into());
    state.tasks.insert("m1".into(), vec![dependent, designer]);
    assert_eq!(
        state.blocking_reason(),
        Some("Required framework guides are missing")
    );
}

#[test]
fn history_details_wrap_and_scroll_to_last_complete_viewport() {
    let mut t = term(72, 18);
    let mut view = ui::history::HistoryView::new(maestro_types::history::History {
        versions: vec![],
        current_id: None,
    });
    view.detail = Some(format!(
        "{}\n{}\nEND OF DIFF",
        "A long version description with Unicode 日本語 and readable wrapping. ".repeat(4),
        (0..40).map(|i| format!("+ line {i}\n")).collect::<String>()
    ));
    t.draw(|f| ui::history::render(f, f.area(), &Theme::dark(), &view))
        .unwrap();
    let top = rows(&t).join("\n");
    assert!(top.contains("History & restore"));
    assert!(top.contains("A long version"));
    assert!(view.scroll_max.get() > 0);
    view.scroll = usize::MAX;
    t.draw(|f| ui::history::render(f, f.area(), &Theme::dark(), &view))
        .unwrap();
    let bottom = rows(&t).join("\n");
    assert!(bottom.contains("END OF DIFF"));
    assert!(bottom.contains("line 39"));
    assert!(bottom.contains("Esc close"));
}
