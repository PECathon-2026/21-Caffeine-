//! Approvals tab: pending approval cards, j/k select, a approve, x reject,
//! v view diff.

use crate::state::ClientState;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct ApprovalsView {
    pub selected: usize,
}

impl ApprovalsView {
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &ClientState,
    view: &ApprovalsView,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if state.approvals.is_empty() {
        lines.push(Line::from(Span::styled(
            "no pending approvals",
            Style::default().fg(theme.dim),
        )));
    }
    for (i, approval) in state.approvals.iter().enumerate() {
        let mut card = super::cards::approval_card(approval, theme, area.width.min(90));
        if i == view.selected {
            for line in &mut card {
                *line = line
                    .clone()
                    .style(Style::default().add_modifier(Modifier::BOLD));
            }
        }
        lines.extend(card);
        lines.push(Line::default());
    }
    // Keep the selected card in view: scroll by whole cards.
    let per_card = lines.len() / state.approvals.len().max(1);
    let height = area.height as usize;
    let target = view.selected * per_card;
    let start = if target + per_card > height {
        (target + per_card).saturating_sub(height)
    } else {
        0
    };
    let visible: Vec<Line<'static>> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().bg(theme.bg)),
        area,
    );
}
