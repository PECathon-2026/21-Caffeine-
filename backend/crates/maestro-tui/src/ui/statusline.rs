//! Status line (1 row): reversed mode block + contextual key hints on the
//! left; approvals badge, daemon dot, tokens, cost, elapsed on the right.
//! Segments shed by priority — the line never wraps.

use crate::state::Connectivity;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Mission,
    Approval,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Chat => " CHAT ",
            Mode::Mission => " MISSION ",
            Mode::Approval => " APPROVAL ",
        }
    }
}

/// Semantic tags so render can style without the builder knowing colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Mode,
    Hint,
    Warn,
    Ok,
    Err,
    Dim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub tag: Tag,
    /// Lower number = dropped later. Mode/daemon are priority 0 (never shed).
    pub priority: u8,
}

pub struct StatusInput<'a> {
    pub mode: Mode,
    pub hints: &'a [(&'a str, &'a str)],
    pub pending_approvals: usize,
    pub daemon: Connectivity,
    pub tokens: i64,
    pub cost_usd: f64,
    pub elapsed_secs: Option<i64>,
    pub toast: Option<&'a str>,
}

/// Pure builder + shedder. Returns (left, right) segment lists that fit in
/// `width` columns; sheds the highest `priority` value first.
pub fn build(input: &StatusInput<'_>, width: u16) -> (Vec<Seg>, Vec<Seg>) {
    let mut left: Vec<Seg> = vec![Seg {
        text: input.mode.label().to_string(),
        tag: Tag::Mode,
        priority: 0,
    }];
    if let Some(toast) = input.toast {
        left.push(Seg {
            text: format!(" {toast} "),
            tag: Tag::Warn,
            priority: 1,
        });
    } else {
        for (key, what) in input.hints {
            left.push(Seg {
                text: format!(" {key} {what} "),
                tag: Tag::Hint,
                priority: 5,
            });
        }
    }

    let mut right: Vec<Seg> = Vec::new();
    if input.pending_approvals > 0 {
        right.push(Seg {
            text: format!("⚠ {} ", input.pending_approvals),
            tag: Tag::Warn,
            priority: 1,
        });
    }
    if input.tokens > 0 {
        right.push(Seg {
            text: format!("{} tok ", format_tokens(input.tokens)),
            tag: Tag::Dim,
            priority: 3,
        });
    }
    if input.cost_usd > 0.0 {
        right.push(Seg {
            text: format!("${:.2} ", input.cost_usd),
            tag: Tag::Dim,
            priority: 3,
        });
    }
    if let Some(secs) = input.elapsed_secs {
        right.push(Seg {
            text: format!("{} ", format_elapsed(secs)),
            tag: Tag::Dim,
            priority: 4,
        });
    }
    right.push(Seg {
        text: "● ".to_string(),
        tag: match input.daemon {
            Connectivity::Connected => Tag::Ok,
            Connectivity::Connecting | Connectivity::StartingDaemon => Tag::Warn,
            Connectivity::Offline => Tag::Err,
        },
        priority: 0,
    });

    // Shed until it fits: repeatedly remove one segment with the highest
    // priority value (>0).
    let fits = |l: &[Seg], r: &[Seg]| -> bool {
        let w: usize = l
            .iter()
            .chain(r.iter())
            .map(|s| super::dwidth(&s.text))
            .sum();
        w <= width as usize
    };
    while !fits(&left, &right) {
        let candidate = left
            .iter()
            .enumerate()
            .map(|(i, s)| (s.priority, 0usize, i))
            .chain(
                right
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (s.priority, 1usize, i)),
            )
            .filter(|(p, _, _)| *p > 0)
            .max_by_key(|(p, side, i)| (*p, *side, usize::MAX - *i));
        match candidate {
            // Drop later (rightmost) segments first within a priority class.
            Some((_, 0, i)) => {
                // Within left hints drop from the right end.
                let last = left
                    .iter()
                    .rposition(|s| s.priority == left[i].priority)
                    .unwrap_or(i);
                left.remove(last);
            }
            Some((_, _, i)) => {
                right.remove(i);
            }
            None => break,
        }
    }
    (left, right)
}

pub fn format_tokens(t: i64) -> String {
    if t >= 1_000_000 {
        format!("{:.1}M", t as f64 / 1_000_000.0)
    } else if t >= 1_000 {
        format!("{:.1}k", t as f64 / 1_000.0)
    } else {
        t.to_string()
    }
}

pub fn format_elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn seg_style(theme: &Theme, tag: Tag) -> Style {
    match tag {
        Tag::Mode => Style::default()
            .fg(theme.mode_fg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD),
        Tag::Hint => Style::default().fg(theme.dim),
        Tag::Warn => Style::default().fg(theme.warn),
        Tag::Ok => Style::default().fg(theme.ok),
        Tag::Err => Style::default().fg(theme.error),
        Tag::Dim => Style::default().fg(theme.dim),
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, theme: &Theme, input: &StatusInput<'_>) {
    let (left, right) = build(input, area.width);
    let left_w: usize = left.iter().map(|s| super::dwidth(&s.text)).sum();
    let right_w: usize = right.iter().map(|s| super::dwidth(&s.text)).sum();
    let pad = (area.width as usize).saturating_sub(left_w + right_w);

    let mut spans: Vec<Span<'static>> = Vec::new();
    for seg in &left {
        spans.push(Span::styled(seg.text.clone(), seg_style(theme, seg.tag)));
    }
    spans.push(Span::raw(" ".repeat(pad)));
    for seg in &right {
        spans.push(Span::styled(seg.text.clone(), seg_style(theme, seg.tag)));
    }
    frame.render_widget(
        ratatui::widgets::Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.bg)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>() -> StatusInput<'a> {
        StatusInput {
            mode: Mode::Mission,
            hints: &[("^p", "palette"), ("1-5", "tabs"), ("esc", "close")],
            pending_approvals: 2,
            daemon: Connectivity::Connected,
            tokens: 152_000,
            cost_usd: 1.27,
            elapsed_secs: Some(93),
            toast: None,
        }
    }

    fn total_width(l: &[Seg], r: &[Seg]) -> usize {
        l.iter()
            .chain(r.iter())
            .map(|s| crate::ui::dwidth(&s.text))
            .sum()
    }

    #[test]
    fn wide_keeps_everything() {
        let i = input();
        let (l, r) = build(&i, 120);
        assert!(l.iter().any(|s| s.text.contains("palette")));
        assert!(r.iter().any(|s| s.text.contains("⚠ 2")));
        assert!(r.iter().any(|s| s.text.contains("tok")));
        assert!(r.iter().any(|s| s.text.contains("$1.27")));
        assert!(r.iter().any(|s| s.text.contains("1m33s")));
        assert!(total_width(&l, &r) <= 120);
    }

    #[test]
    fn narrow_sheds_hints_before_badges() {
        let i = input();
        let (l, r) = build(&i, 40);
        assert!(total_width(&l, &r) <= 40);
        // Mode block and daemon dot always survive.
        assert_eq!(l[0].tag, Tag::Mode);
        assert!(r.iter().any(|s| s.tag == Tag::Ok));
        // Approvals badge survives at 40 while hints are shed first.
        assert!(r.iter().any(|s| s.text.contains("⚠ 2")));
        assert!(!l.iter().any(|s| s.text.contains("palette")));
    }

    #[test]
    fn tiny_width_never_wraps() {
        let i = input();
        let (l, r) = build(&i, 14);
        assert!(total_width(&l, &r) <= 14);
        assert_eq!(l[0].tag, Tag::Mode);
    }

    #[test]
    fn toast_replaces_hints() {
        let mut i = input();
        i.toast = Some("press again to quit");
        let (l, _) = build(&i, 120);
        assert!(l.iter().any(|s| s.text.contains("press again to quit")));
        assert!(!l.iter().any(|s| s.text.contains("palette")));
    }

    #[test]
    fn formats() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(152_000), "152.0k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
        assert_eq!(format_elapsed(45), "45s");
        assert_eq!(format_elapsed(93), "1m33s");
        assert_eq!(format_elapsed(3720), "1h02m");
    }
}
