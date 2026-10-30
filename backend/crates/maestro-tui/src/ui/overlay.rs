//! Overlays: command palette (Ctrl+P), model picker (/models, /model), help.
//! Escape always closes the topmost overlay.

use crate::commands::{fuzzy_rank, SlashCommand};
use crate::keymap::Keymap;
use crate::theme::Theme;
use maestro_types::domain::Role;
use maestro_types::provider::DiscoveredModel;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

#[derive(Debug)]
pub enum Overlay {
    Palette(PaletteState),
    ModelPicker(PickerState),
    SessionPicker(SessionPickerState),
    Help { scroll: usize },
    History(super::history::HistoryView),
}

/// `/sessions`: fuzzy list of persisted chat sessions; Enter resumes one.
#[derive(Debug)]
pub struct SessionPickerState {
    pub filter: String,
    pub selected: usize,
    pub sessions: Vec<maestro_types::domain::ChatSession>,
}

impl SessionPickerState {
    pub fn new(mut sessions: Vec<maestro_types::domain::ChatSession>) -> Self {
        // Most recently active first.
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        SessionPickerState {
            filter: String::new(),
            selected: 0,
            sessions,
        }
    }

    /// Indices into `sessions`, fuzzy-ranked by title (all when no filter).
    pub fn ranked(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.sessions.len()).collect();
        }
        crate::commands::fuzzy_rank(&self.filter, &self.sessions, |s| s.title.as_str())
    }
}

/// Keep `selected` inside a window of `visible` rows over `len` items.
/// Returns the window start (the same math the slash finder uses).
pub fn window_start(selected: usize, len: usize, visible: usize) -> usize {
    if len <= visible {
        return 0;
    }
    selected
        .saturating_sub(visible.saturating_sub(1))
        .min(len - visible)
}

/// Row marker inside a windowed list: `❯` on the selection, `↑`/`↓` on the
/// window edges when rows continue off-screen, blank otherwise.
pub fn row_marker(
    row: usize,
    selected: usize,
    start: usize,
    visible: usize,
    len: usize,
) -> &'static str {
    if row == selected {
        "❯ "
    } else if row == start && start > 0 {
        "↑ "
    } else if row + 1 == start + visible && row + 1 < len {
        "↓ "
    } else {
        "  "
    }
}

/// Compact relative age for session rows.
pub fn fmt_age(now_ms: i64, then_ms: i64) -> String {
    let secs = ((now_ms - then_ms) / 1000).max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[derive(Debug, Default)]
pub struct PaletteState {
    pub input: String,
    pub selected: usize,
}

impl PaletteState {
    pub fn ranked(&self) -> Vec<SlashCommand> {
        let items: Vec<(&'static str, SlashCommand)> = SlashCommand::ALL
            .into_iter()
            .map(|c| (c.name(), c))
            .collect();
        fuzzy_rank(&self.input, &items, |(name, _)| name)
            .into_iter()
            .map(|i| items[i].1)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickStage {
    Model,
    Role,
}

#[derive(Debug)]
pub struct PickerState {
    pub filter: String,
    pub selected: usize,
    pub models: Vec<DiscoveredModel>,
    pub stage: PickStage,
    /// Preselected via `/model <role>`, or chosen in the Role stage.
    pub role: Option<Role>,
    /// Model chosen in the first stage ("provider/model").
    pub chosen: Option<(String, String)>,
    /// Browse-only (/models): Enter closes instead of assigning.
    pub browse_only: bool,
}

impl PickerState {
    pub fn new(models: Vec<DiscoveredModel>, role: Option<Role>, browse_only: bool) -> Self {
        PickerState {
            filter: String::new(),
            selected: 0,
            models,
            stage: PickStage::Model,
            role,
            chosen: None,
            browse_only,
        }
    }

    pub fn ranked_models(&self) -> Vec<usize> {
        let keys: Vec<String> = self
            .models
            .iter()
            .map(|m| format!("{}/{}", m.provider_id, m.model))
            .collect();
        fuzzy_rank(&self.filter, &keys, |s| s.as_str())
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn overlay_block(theme: &Theme, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.panel))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(theme.accent),
        ))
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    overlay: &Overlay,
    keymap: &Keymap,
) {
    match overlay {
        Overlay::Palette(p) => render_palette(frame, area, theme, p),
        Overlay::ModelPicker(p) => render_picker(frame, area, theme, p),
        Overlay::SessionPicker(p) => render_sessions(frame, area, theme, p),
        Overlay::Help { scroll } => render_help(frame, area, theme, keymap, *scroll),
        Overlay::History(view) => super::history::render(frame, area, theme, view),
    }
}

fn render_palette(frame: &mut Frame<'_>, area: Rect, theme: &Theme, p: &PaletteState) {
    let rect = centered(area, 60, 16);
    frame.render_widget(Clear, rect);
    let block = overlay_block(theme, "commands");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![Line::from(vec![
        Span::styled("❯ ", Style::default().fg(theme.accent)),
        Span::styled(p.input.clone(), Style::default().fg(theme.text)),
    ])];
    let ranked = p.ranked();
    let visible = (inner.height.saturating_sub(1) as usize).max(1);
    let len = ranked.len();
    let sel = p.selected.min(len.saturating_sub(1));
    let start = window_start(sel, len, visible);
    for (i, cmd) in ranked.into_iter().enumerate().skip(start).take(visible) {
        let is_sel = i == sel;
        let mark = row_marker(i, sel, start, visible, len);
        let name_style = if is_sel {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent)
        };
        lines.push(Line::from(vec![
            Span::styled(mark, Style::default().fg(theme.accent)),
            Span::styled(format!("/{:<10}", cmd.name()), name_style),
            Span::styled(cmd.describe().to_string(), Style::default().fg(theme.dim)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_sessions(frame: &mut Frame<'_>, area: Rect, theme: &Theme, p: &SessionPickerState) {
    let rect = centered(area, 70, 16);
    frame.render_widget(Clear, rect);
    let block = overlay_block(theme, "sessions");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![Line::from(vec![
        Span::styled("❯ ", Style::default().fg(theme.accent)),
        Span::styled(p.filter.clone(), Style::default().fg(theme.text)),
    ])];
    let ranked = p.ranked();
    if ranked.is_empty() {
        lines.push(Line::from(Span::styled(
            " no sessions yet — say something first",
            Style::default().fg(theme.dim),
        )));
    }
    let now = maestro_types::now_ms();
    let visible = (inner.height.saturating_sub(2) as usize).max(1);
    let len = ranked.len();
    let selected = p.selected.min(len.saturating_sub(1));
    let start = window_start(selected, len, visible);
    for (i, idx) in ranked.into_iter().enumerate().skip(start).take(visible) {
        let session = &p.sessions[idx];
        let sel = i == selected;
        let marker = row_marker(i, selected, start, visible, len);
        let title_style = if sel {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let title = crate::ui::clip(&session.title, (inner.width as usize).saturating_sub(20));
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(theme.accent)),
            Span::styled(format!("{title:<44}"), title_style),
            Span::styled(
                fmt_age(now, session.updated_at),
                Style::default().fg(theme.dim),
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(
        " ↑↓ select · ↵ resume · esc close",
        Style::default().fg(theme.dim),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_picker(frame: &mut Frame<'_>, area: Rect, theme: &Theme, p: &PickerState) {
    let rect = centered(area, 70, 18);
    frame.render_widget(Clear, rect);
    let title = match p.stage {
        PickStage::Model => "models",
        PickStage::Role => "assign to role",
    };
    let block = overlay_block(theme, title);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line<'static>> = Vec::new();
    match p.stage {
        PickStage::Model => {
            lines.push(Line::from(vec![
                Span::styled("❯ ", Style::default().fg(theme.accent)),
                Span::styled(p.filter.clone(), Style::default().fg(theme.text)),
            ]));
            let ranked = p.ranked_models();
            if ranked.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!(" no models match '{}'", p.filter),
                    Style::default().fg(theme.dim),
                )));
            }
            // Filter row + footer hint take 2 rows; the rest is the window,
            // kept over the selection so ↓ scrolls instead of walking off.
            let visible = (inner.height.saturating_sub(2) as usize).max(1);
            let len = ranked.len();
            let sel = p.selected.min(len.saturating_sub(1));
            let start = window_start(sel, len, visible);
            for (i, idx) in ranked.into_iter().enumerate().skip(start).take(visible) {
                let m = &p.models[idx];
                let is_sel = i == sel;
                let mark = row_marker(i, sel, start, visible, len);
                let ctx = m
                    .context_window
                    .map(|c| format!("  {}k ctx", c / 1000))
                    .unwrap_or_default();
                let style = if is_sel {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                lines.push(Line::from(vec![
                    Span::styled(mark, Style::default().fg(theme.accent)),
                    Span::styled(format!("{}/{}", m.provider_id, m.model), style),
                    Span::styled(ctx, Style::default().fg(theme.dim)),
                ]));
            }
            lines.push(Line::from(Span::styled(
                if p.browse_only {
                    format!(" {len} models · ↑↓ select · esc close")
                } else {
                    format!(" {len} models · ↑↓ select · ↵ assign to role · esc close")
                },
                Style::default().fg(theme.dim),
            )));
        }
        PickStage::Role => {
            lines.push(Line::from(Span::styled(
                format!(
                    "assign {} to:",
                    p.chosen
                        .as_ref()
                        .map(|(pr, m)| format!("{pr}/{m}"))
                        .unwrap_or_default()
                ),
                Style::default().fg(theme.dim),
            )));
            for (i, role) in Role::ALL.into_iter().enumerate() {
                let sel = i == p.selected;
                let mark = if sel { "▸ " } else { "  " };
                let style = if sel {
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                lines.push(Line::from(vec![
                    Span::styled(mark, Style::default().fg(theme.accent)),
                    Span::styled(role.as_str().to_string(), style),
                ]));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_help(frame: &mut Frame<'_>, area: Rect, theme: &Theme, keymap: &Keymap, scroll: usize) {
    let rect = centered(area, 64, area.height.saturating_sub(4).min(24));
    frame.render_widget(Clear, rect);
    let block = overlay_block(theme, "help");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "keybindings",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    for (ctx, chord, action) in keymap.table() {
        lines.push(Line::from(vec![
            Span::styled(format!(" {ctx:<10}"), Style::default().fg(theme.dim)),
            Span::styled(format!("{chord:<14}"), Style::default().fg(theme.accent)),
            Span::styled(action, Style::default().fg(theme.text)),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "slash commands",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    for cmd in SlashCommand::ALL {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" /{:<10}", cmd.name()),
                Style::default().fg(theme.accent),
            ),
            Span::styled(cmd.describe().to_string(), Style::default().fg(theme.text)),
        ]));
    }
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll)
        .take(inner.height as usize)
        .collect();
    frame.render_widget(Paragraph::new(visible), inner);
}
