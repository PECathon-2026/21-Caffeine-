//! Header (1 row): brand · cwd tail · git branch · mission status · model.
//! Sheds segments right-to-left when narrow; never wraps.

use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

pub const SEP: &str = " · ";

/// Pure shedding function: keeps `brand` always, drops the rightmost of the
/// remaining segments until the joined line fits in `width` columns.
pub fn shed_segments(segments: &[String], width: u16) -> Vec<String> {
    let width = width as usize;
    let sep_w = super::dwidth(SEP);
    let mut kept: Vec<String> = segments.to_vec();
    while kept.len() > 1 {
        let total: usize =
            kept.iter().map(|s| super::dwidth(s)).sum::<usize>() + sep_w * (kept.len() - 1);
        if total <= width {
            break;
        }
        kept.pop();
    }
    // Even the brand alone may overflow a sliver-thin terminal: clip it.
    if kept.len() == 1 && super::dwidth(&kept[0]) > width {
        kept[0] = super::clip(&kept[0], width);
    }
    kept
}

/// Build the segment list from current state (None segments skipped).
pub fn build_segments(
    brand: &str,
    cwd_tail: &str,
    branch: Option<&str>,
    mission_status: Option<&str>,
    model: Option<&str>,
) -> Vec<String> {
    let mut out = vec![brand.to_string(), cwd_tail.to_string()];
    if let Some(b) = branch {
        out.push(format!("⎇ {b}"));
    }
    if let Some(m) = mission_status {
        out.push(m.to_string());
    }
    if let Some(m) = model {
        out.push(m.to_string());
    }
    out
}

pub fn render(frame: &mut Frame<'_>, area: Rect, theme: &Theme, segments: &[String]) {
    let kept = shed_segments(segments, area.width);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, seg) in kept.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(SEP, Style::default().fg(theme.dim)));
        }
        let style = if i == 0 {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.dim)
        };
        spans.push(Span::styled(seg.clone(), style));
    }
    frame.render_widget(
        ratatui::widgets::Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.bg)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs() -> Vec<String> {
        build_segments(
            "maestro",
            "~/projects/maestro-rs",
            Some("main*"),
            Some("running"),
            Some("anthropic/claude-sonnet-4-20250514"),
        )
    }

    #[test]
    fn wide_terminal_keeps_all_segments() {
        let kept = shed_segments(&segs(), 120);
        assert_eq!(kept.len(), 5);
    }

    #[test]
    fn medium_terminal_drops_rightmost_first() {
        let kept = shed_segments(&segs(), 80);
        // model (24 cols) must be the first to go at 80 cols.
        assert!(kept.len() < 5);
        assert_eq!(kept[0], "maestro");
        assert!(!kept.iter().any(|s| s.contains("claude")));
        // The remaining prefix is preserved in order.
        assert_eq!(kept[1], "~/projects/maestro-rs");
    }

    #[test]
    fn narrow_terminal_keeps_brand() {
        let kept = shed_segments(&segs(), 40);
        assert!(!kept.is_empty());
        assert_eq!(kept[0], "maestro");
        let total: usize =
            kept.iter().map(|s| super::super::dwidth(s)).sum::<usize>() + 3 * (kept.len() - 1);
        assert!(total <= 40);
    }

    #[test]
    fn sliver_terminal_clips_brand() {
        let kept = shed_segments(&segs(), 4);
        assert_eq!(kept.len(), 1);
        assert!(super::super::dwidth(&kept[0]) <= 4);
    }
}
