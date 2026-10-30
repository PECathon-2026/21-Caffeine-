//! Right sidebar (~40 cols): team panel (one row per running task) above the
//! mission task list with status glyphs. Auto-hidden below 100 columns.

use crate::state::ClientState;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub const SIDEBAR_WIDTH: u16 = 40;
pub const AUTO_HIDE_BELOW: u16 = 100;

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &ClientState,
    spinner_frame: usize,
) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(Span::styled(
        "team",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    let running = state.running_tasks();
    let mission_status = state
        .active_mission
        .as_deref()
        .and_then(|id| state.mission(id))
        .map(|m| m.mission.status);
    if running.is_empty() {
        let message = match mission_status {
            Some(maestro_types::domain::MissionStatus::Blocked) => "  blocked — no agents running",
            Some(maestro_types::domain::MissionStatus::Paused) => "  paused",
            _ if state.waiting_chat => "  answering your question",
            _ => "  idle",
        };
        lines.push(Line::from(Span::styled(
            message,
            Style::default().fg(theme.dim),
        )));
    }
    for assignment in &state.role_models {
        let tasks: Vec<_> = state
            .active_tasks()
            .iter()
            .filter(|t| t.role.as_str() == assignment.role)
            .collect();
        let status = [
            "running",
            "failed",
            "blocked",
            "ready",
            "proposed",
            "completed",
        ]
        .into_iter()
        .find(|s| tasks.iter().any(|t| t.status.as_str() == *s))
        .unwrap_or(
            if assignment.role == "planner"
                && (state.waiting_chat
                    || mission_status == Some(maestro_types::domain::MissionStatus::Planning))
            {
                "thinking"
            } else {
                "idle"
            },
        );
        let color = match status {
            "running" | "thinking" => theme.accent,
            "failed" => theme.error,
            "blocked" => theme.warn,
            "completed" => theme.ok,
            _ => theme.dim,
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "  {} ",
                    maestro_types::domain::Role::parse(&assignment.role)
                        .map(|r| r.guild_name())
                        .unwrap_or(&assignment.role)
                ),
                Style::default().fg(theme.text),
            ),
            Span::styled(status.to_string(), Style::default().fg(color)),
        ]));
        lines.push(Line::from(Span::styled(
            format!(
                "    {}",
                super::clip(&assignment.model, width.saturating_sub(4))
            ),
            Style::default().fg(theme.dim),
        )));
    }
    for task in running {
        let model = task.model_override.clone().unwrap_or_default();
        let title = super::clip(&task.title, width.saturating_sub(8 + model.len()));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", super::role_glyph(task.role)),
                Style::default().fg(theme.accent),
            ),
            Span::styled(
                format!("{} ", super::spinner(spinner_frame)),
                Style::default().fg(theme.accent),
            ),
            Span::styled(title, Style::default().fg(theme.text)),
            Span::styled(
                if model.is_empty() {
                    String::new()
                } else {
                    format!(" {model}")
                },
                Style::default().fg(theme.dim),
            ),
        ]));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "tasks · 2 opens full details",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    if state.active_tasks().is_empty() {
        lines.push(Line::from(Span::styled(
            "  no mission",
            Style::default().fg(theme.dim),
        )));
    }
    for task in state.active_tasks() {
        lines.extend(task_rows(task, theme, width));
    }

    if let Some(reason) = state.blocking_reason() {
        lines.push(Line::default());
        lines.extend(
            super::transcript::wrap(reason, width)
                .into_iter()
                .map(|text| Line::from(Span::styled(text, Style::default().fg(theme.error)))),
        );
    }

    lines.truncate(inner.height as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme.bg))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

/// One task list row: status glyph + role glyph + title (pure, testable).
pub fn task_row(task: &maestro_types::domain::Task, theme: &Theme, width: usize) -> Line<'static> {
    use maestro_types::domain::TaskStatus;
    let glyph = super::task_glyph(task.status);
    let gstyle = match task.status {
        TaskStatus::Completed => Style::default().fg(theme.ok),
        TaskStatus::Failed => Style::default().fg(theme.error),
        TaskStatus::Running => Style::default().fg(theme.accent),
        TaskStatus::Blocked => Style::default().fg(theme.warn),
        _ => Style::default().fg(theme.dim),
    };
    let tstyle = if task.status == TaskStatus::Proposed {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::styled(format!("{glyph} "), gstyle),
        Span::styled(
            format!("{} ", super::role_glyph(task.role)),
            Style::default().fg(theme.dim),
        ),
        Span::styled(super::clip(&task.title, width.saturating_sub(4)), tstyle),
    ])
}

/// Wrapped task titles retain every word in the narrow sidebar.
pub fn task_rows(
    task: &maestro_types::domain::Task,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let wrapped = super::transcript::wrap(&task.title, width.saturating_sub(4));
    wrapped
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            if i == 0 {
                let mut row = task_row(task, theme, width);
                if let Some(title) = row.spans.last_mut() {
                    title.content = text.into();
                }
                row
            } else {
                Line::from(Span::styled(
                    format!("    {text}"),
                    Style::default().fg(theme.text),
                ))
            }
        })
        .collect()
}
