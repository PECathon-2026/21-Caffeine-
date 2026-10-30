//! Tasks tab: mission task list with status glyphs, j/k selection, Enter for
//! a detail pane, r retry, d diff.

use crate::state::ClientState;
use crate::theme::Theme;
use maestro_types::domain::Task;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct TasksView {
    pub selected: usize,
    pub detail_open: bool,
    pub detail_scroll: usize,
}

impl TasksView {
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    pub fn selected_task<'a>(&self, state: &'a ClientState) -> Option<&'a Task> {
        state.active_tasks().get(self.selected)
    }
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &ClientState,
    view: &TasksView,
) {
    let tasks = state.active_tasks();
    if view.detail_open {
        if let Some(task) = tasks.get(view.selected) {
            render_detail(frame, area, theme, task, view.detail_scroll);
            return;
        }
    }
    let list_area = area;

    let mut lines: Vec<Line<'static>> = Vec::new();
    if tasks.is_empty() {
        lines.push(Line::from(Span::styled(
            "no active mission — /run <objective> to start one",
            Style::default().fg(theme.dim),
        )));
    }
    let height = list_area.height as usize;
    let start = view.selected.saturating_sub(height.saturating_sub(1));
    for (i, task) in tasks.iter().enumerate().skip(start).take(height) {
        let mut line = super::sidebar::task_row(task, theme, list_area.width as usize);
        let attempts = if task.attempts > 1 {
            format!("  (attempt {})", task.attempts)
        } else {
            String::new()
        };
        line.spans
            .push(Span::styled(attempts, Style::default().fg(theme.dim)));
        if i == view.selected {
            line = line.style(
                Style::default()
                    .bg(theme.panel)
                    .add_modifier(Modifier::BOLD),
            );
        }
        lines.push(line);
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        list_area,
    );
}

pub fn detail_lines(task: &Task, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let fields = [
        ("Task", task.title.clone()),
        (
            "Status",
            format!(
                "{} {}",
                super::task_glyph(task.status),
                task.status.as_str()
            ),
        ),
        ("Reason", task.status_reason.clone().unwrap_or_default()),
        ("Role", task.role.guild_name().to_string()),
        (
            "Model override",
            task.model_override.clone().unwrap_or_default(),
        ),
        ("Write scopes", task.write_scopes.join(", ")),
        ("Dependencies", task.dependencies.join(", ")),
        ("Description", task.description.clone()),
        (
            "Acceptance criteria",
            task.acceptance
                .iter()
                .map(|s| format!("• {s}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ("Summary", task.summary.clone().unwrap_or_default()),
    ];
    for (label, content) in fields {
        if content.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            label,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        lines.extend(
            super::transcript::wrap(&content, width)
                .into_iter()
                .map(|text| Line::from(Span::styled(text, Style::default().fg(theme.text)))),
        );
        lines.push(Line::default());
    }
    lines
}

fn render_detail(frame: &mut Frame<'_>, area: Rect, theme: &Theme, task: &Task, scroll: usize) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border))
        .title(Span::styled(
            format!(" {} ", task.task_key),
            Style::default().fg(theme.accent),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = inner.height.saturating_sub(1) as usize;
    let lines = detail_lines(task, theme, inner.width as usize);
    let start = scroll.min(lines.len().saturating_sub(height));
    let visible: Vec<_> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(
        Paragraph::new(visible),
        Rect {
            height: height as u16,
            ..inner
        },
    );
    if inner.height > 0 {
        frame.render_widget(
            Paragraph::new("↑/↓ or j/k scroll · PgUp/PgDn page · r retry · Esc back")
                .style(Style::default().fg(theme.dim)),
            Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            },
        );
    }
}
