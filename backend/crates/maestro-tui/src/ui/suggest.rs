//! Slash-command autocomplete: a finder anchored above the composer while
//! the input is a bare `/token`, and a one-line ghost hint for a completed
//! command's arguments. Pure presentation — selection state lives in App.
//!
//! Row grammar (shared with every picker): `❯` marks the row Enter runs;
//! `↑`/`↓` on the window edges mean more rows off-screen. The finder stays
//! open on zero matches ("no matches" is the answer — collapsing mid-word
//! would let keystrokes leak meaning).

use crate::commands::SlashCommand;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Longest command name, for column alignment.
fn name_col() -> usize {
    SlashCommand::ALL
        .iter()
        .map(|c| c.name().len())
        .max()
        .unwrap_or(8)
        + 1
}

pub const MAX_VISIBLE: usize = 8;

/// Rows the popup needs (list or the one no-match row, + footer + border).
pub fn popup_height(n: usize) -> u16 {
    (n.clamp(1, MAX_VISIBLE) as u16) + 2
}

/// Render the finder in `area` (sized via [`popup_height`], anchored right
/// above the composer). `token` is the text after the slash, for the
/// no-match row.
pub fn render_popup(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    items: &[SlashCommand],
    selected: usize,
    token: &str,
) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  no matches for /{token}"),
            Style::default().fg(theme.dim),
        )));
    } else {
        // Keep the selection in-window when the list is longer than it.
        let visible = items.len().min(MAX_VISIBLE);
        let max_start = items.len() - visible;
        let start = selected.saturating_sub(visible - 1).min(max_start);
        let col = name_col();
        for (row, cmd) in items.iter().enumerate().skip(start).take(visible) {
            let is_sel = row == selected;
            // Edge markers replace a scrollbar: ↑/↓ mean more off-window.
            let marker = if is_sel {
                "❯ "
            } else if row == start && start > 0 {
                "↑ "
            } else if row == start + visible - 1 && row + 1 < items.len() {
                "↓ "
            } else {
                "  "
            };
            let marker_style = if is_sel {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim)
            };
            let name = format!("/{:<col$}", cmd.name());
            let name_style = if is_sel {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            let mut spans = vec![
                Span::styled(marker.to_string(), marker_style),
                Span::styled(name, name_style),
                Span::styled(cmd.describe().to_string(), Style::default().fg(theme.dim)),
            ];
            if is_sel {
                if let Some(hint) = cmd.arg_hint() {
                    spans.push(Span::styled(
                        format!("  {hint}"),
                        Style::default()
                            .fg(theme.dim)
                            .add_modifier(Modifier::ITALIC),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
    }
    lines.push(Line::from(Span::styled(
        "  ↑↓ select · tab complete · ↵ run · esc dismiss".to_string(),
        Style::default().fg(theme.dim),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One-line ghost hint above the composer once a command with arguments has
/// been completed: `/steer <directive> — steer the active mission`.
pub fn render_arg_hint(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    cmd: SlashCommand,
    hint: &str,
) {
    frame.render_widget(Clear, area);
    let line = Line::from(vec![
        Span::styled(
            format!("  /{} ", cmd.name()),
            Style::default().fg(theme.accent),
        ),
        Span::styled(
            hint.to_string(),
            Style::default()
                .fg(theme.dim)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::styled(
            format!(" — {}", cmd.describe()),
            Style::default().fg(theme.dim),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
