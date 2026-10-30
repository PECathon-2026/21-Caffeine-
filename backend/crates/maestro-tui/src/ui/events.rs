//! Events tab: bounded ring of the last 500 events, follow mode (scroll 0).

use crate::state::ClientState;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// One event row (pure).
pub fn event_line(
    ev: &maestro_types::event::EventRecord,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let style = if ev.kind.contains("fail")
        || ev.kind.ends_with("denied")
        || ev.kind.contains("violation")
        || ev.kind.contains("conflict")
    {
        Style::default().fg(theme.error)
    } else if ev.kind.starts_with("approval") {
        Style::default().fg(theme.warn)
    } else {
        Style::default().fg(theme.dim)
    };
    let payload = ev.payload.to_string();
    let head = format!("{:>7}  {:<22} ", ev.seq, ev.kind);
    let body = super::clip(&payload, width.saturating_sub(super::dwidth(&head)));
    Line::from(vec![
        Span::styled(head, style),
        Span::styled(body, Style::default().fg(theme.text)),
    ])
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &ClientState,
    scroll_from_bottom: usize,
) {
    let width = area.width as usize;
    let height = area.height as usize;
    let total = state.events.len();
    let end = total.saturating_sub(scroll_from_bottom);
    let start = end.saturating_sub(height);
    let lines: Vec<Line<'static>> = state
        .events
        .iter()
        .skip(start)
        .take(end - start)
        .map(|ev| event_line(ev, theme, width))
        .collect();
    let lines = if lines.is_empty() {
        vec![Line::from(Span::styled(
            "no events yet",
            Style::default().fg(theme.dim),
        ))]
    } else {
        lines
    };
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}
