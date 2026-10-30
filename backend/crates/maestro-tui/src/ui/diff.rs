//! Diff tab: unified diff, +/- rows tinted full-width via diff_added_bg /
//! diff_removed_bg, hunk headers dim. Lazy-fetched by the app.

use crate::state::CachedDiff;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Style one unified-diff line (pure).
pub fn diff_line(raw: &str, theme: &Theme, width: usize) -> Line<'static> {
    let text = super::clip(raw, width);
    let pad = width.saturating_sub(super::dwidth(&text));
    let padded = format!("{text}{}", " ".repeat(pad));
    if raw.starts_with("+++") || raw.starts_with("---") {
        Line::from(Span::styled(padded, Style::default().fg(theme.text)))
    } else if raw.starts_with("@@") {
        Line::from(Span::styled(padded, Style::default().fg(theme.dim)))
    } else if raw.starts_with('+') {
        Line::from(Span::styled(
            padded,
            Style::default().fg(theme.text).bg(theme.diff_added_bg),
        ))
    } else if raw.starts_with('-') {
        Line::from(Span::styled(
            padded,
            Style::default().fg(theme.text).bg(theme.diff_removed_bg),
        ))
    } else if raw.starts_with("diff ") || raw.starts_with("index ") {
        Line::from(Span::styled(padded, Style::default().fg(theme.dim)))
    } else {
        Line::from(Span::styled(padded, Style::default().fg(theme.text)))
    }
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    diff: Option<&CachedDiff>,
    scroll: usize,
    loading: bool,
) {
    let width = area.width as usize;
    let lines: Vec<Line<'static>> = match diff {
        Some(d) if !d.is_empty() => {
            // Only the visible window is styled; the line index was derived
            // once when the diff settled, so this is O(height) per frame.
            let start = scroll.min(d.line_count().saturating_sub(1));
            d.lines(start, area.height as usize)
                .map(|l| diff_line(l, theme, width))
                .collect()
        }
        Some(_) => vec![Line::from(Span::styled(
            "no changes",
            Style::default().fg(theme.dim),
        ))],
        None if loading => vec![Line::from(Span::styled(
            "fetching diff…",
            Style::default().fg(theme.dim),
        ))],
        None => vec![Line::from(Span::styled(
            "no diff — select a mission first",
            Style::default().fg(theme.dim),
        ))],
    };
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tints_full_row_for_adds_and_removes() {
        let theme = Theme::dark();
        let added = diff_line("+let x = 1;", &theme, 40);
        assert_eq!(added.spans.len(), 1);
        assert_eq!(added.spans[0].style.bg, Some(theme.diff_added_bg));
        assert_eq!(crate::ui::dwidth(&added.spans[0].content), 40); // full row
        let removed = diff_line("-let x = 0;", &theme, 40);
        assert_eq!(removed.spans[0].style.bg, Some(theme.diff_removed_bg));
        let hunk = diff_line("@@ -1,3 +1,4 @@", &theme, 40);
        assert_eq!(hunk.spans[0].style.fg, Some(theme.dim));
        assert_eq!(hunk.spans[0].style.bg, None);
        // File headers are not tinted like content lines.
        let header = diff_line("+++ b/src/main.rs", &theme, 40);
        assert_eq!(header.spans[0].style.bg, None);
    }
}
