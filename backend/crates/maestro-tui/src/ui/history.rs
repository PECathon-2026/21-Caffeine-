use crate::theme::Theme;
use maestro_types::history::{History, RestorePreview};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

#[derive(Debug)]
pub struct HistoryView {
    pub history: History,
    pub selected: usize,
    pub detail: Option<String>,
    pub preview: Option<RestorePreview>,
    pub undo: bool,
    pub scroll: usize,
    pub busy: bool,
    pub scroll_max: std::cell::Cell<usize>,
}

impl HistoryView {
    pub fn new(history: History) -> Self {
        Self {
            history,
            selected: 0,
            detail: None,
            preview: None,
            undo: false,
            scroll: 0,
            busy: false,
            scroll_max: std::cell::Cell::new(0),
        }
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, theme: &Theme, view: &HistoryView) {
    let rect = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" History & restore ")
        .style(Style::default().fg(theme.text).bg(theme.bg));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let mut lines = Vec::new();
    if let Some(detail) = &view.detail {
        for s in detail.lines() {
            let color = if s.starts_with('+') {
                theme.accent
            } else {
                theme.text
            };
            let mut row = String::new();
            let mut width = 0;
            for ch in s.chars() {
                let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if width + size > parts[0].width as usize && !row.is_empty() {
                    lines.push(Line::styled(
                        std::mem::take(&mut row),
                        Style::default().fg(color),
                    ));
                    width = 0;
                }
                row.push(ch);
                width += size;
            }
            lines.push(Line::styled(row, Style::default().fg(color)));
        }
        let max = lines.len().saturating_sub(parts[0].height as usize);
        view.scroll_max.set(max);
        let para = Paragraph::new(lines);
        frame.render_widget(
            para.scroll((view.scroll.min(max).min(u16::MAX as usize) as u16, 0)),
            parts[0],
        );
    } else {
        let visible = (parts[0].height as usize / 2).max(1);
        let start =
            super::overlay::window_start(view.selected, view.history.versions.len(), visible);
        for (i, v) in view
            .history
            .versions
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
        {
            let mark = if i == view.selected { "❯" } else { " " };
            let current = if Some(&v.id) == view.history.current_id.as_ref() {
                " · project baseline"
            } else {
                ""
            };
            let style = if i == view.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::styled(
                format!("{mark} {}  {}{current}", &v.id[..8], v.kind),
                style,
            ));
            lines.push(Line::styled(
                format!(
                    "  {}",
                    super::clip(&v.label, parts[0].width.saturating_sub(2) as usize)
                ),
                style,
            ));
        }
        frame.render_widget(Paragraph::new(lines), parts[0]);
    }
    let hint = if view.busy {
        "Working…"
    } else if let Some(p) = &view.preview {
        if p.conflicts.is_empty() {
            "Enter apply this restore · ↑↓/PgUp/PgDn scroll · Esc close"
        } else {
            "Conflicts: files unchanged · ↑↓/PgUp/PgDn scroll · Esc close"
        }
    } else if view.detail.is_some() {
        "↑↓/PgUp/PgDn scroll · Esc close · /history to return"
    } else {
        "↑↓ select · Enter full details & diff · r restore preview · u undo request · Esc close"
    };
    frame.render_widget(
        Paragraph::new(hint)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.dim)),
        parts[1],
    );
}
