//! First-run welcome: a static, content-only block shown in the chat area
//! until the first real turn commits. Zero animation, zero timers — it costs
//! nothing while idle, and it teaches the composer instead of describing it.

use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Startup phase: a quiet centered block while the daemon comes up. Shown
/// instead of the welcome until the control plane answers — the UI arrives,
/// it doesn't pop.
pub fn render_loading(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    spinner: &str,
    phase: &str,
) {
    let mid = area.height / 2;
    let lines: Vec<Line<'static>> = (0..area.height)
        .map(|row| {
            if row + 1 == mid {
                Line::from(vec![
                    Span::styled(
                        "maestro",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "  multi-model mission control",
                        Style::default().fg(theme.dim),
                    ),
                ])
                .centered()
            } else if row == mid {
                Line::from(Span::styled(
                    format!("{spinner} {phase}"),
                    Style::default().fg(theme.dim),
                ))
                .centered()
            } else {
                Line::default()
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Everything the welcome screen shows (mirrors the StatusInput pattern).
/// Brand, cwd, and branch live in the header row — never repeated here.
pub struct WelcomeInput<'a> {
    pub mock_roles: &'a [String],
    pub sessions: &'a [maestro_types::domain::ChatSession],
    pub session_sel: Option<usize>,
}

pub fn render(frame: &mut Frame<'_>, area: Rect, theme: &Theme, input: &WelcomeInput<'_>) {
    let WelcomeInput {
        mock_roles,
        sessions,
        session_sel,
    } = *input;
    let accent = Style::default().fg(theme.accent);
    let text = Style::default().fg(theme.text);
    let dim = Style::default().fg(theme.dim);
    let cmd = |name: &str, args: &str, desc: &str| {
        let invocation = if args.is_empty() {
            name.to_string()
        } else {
            format!("{name} {args}")
        };
        Line::from(vec![
            Span::styled(format!("    {invocation:<22}"), accent),
            Span::styled(desc.to_string(), text),
        ])
    };

    let mut lines: Vec<Line<'static>> = vec![
        Line::default(),
        Line::from(Span::styled("  multi-model mission control", dim)),
        Line::default(),
    ];
    // Mock mode is a state the user must see before typing, not discover
    // from a canned reply.
    if !mock_roles.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  ⚠ mock providers", Style::default().fg(theme.warn)),
            Span::styled(
                format!(
                    " ({}) — replies are canned, no model is wired.",
                    mock_roles.join(", ")
                ),
                dim,
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("    ", dim),
            Span::styled("edit maestro.toml", text),
            Span::styled(" to add a provider, then ", dim),
            Span::styled("/connect", accent),
            Span::styled(" to store its key", dim),
        ]));
        lines.push(Line::default());
    }
    // Recent sessions: an arrow-navigable strip (a fresh session is the
    // default; resuming is a deliberate ↓ + ↵ away).
    if !sessions.is_empty() {
        lines.push(Line::from(Span::styled("  recent sessions", dim)));
        let now = maestro_types::now_ms();
        for (i, session) in sessions.iter().enumerate() {
            let sel = session_sel == Some(i);
            let marker = if sel { "  ❯ " } else { "    " };
            let title_style = if sel {
                accent.add_modifier(Modifier::BOLD)
            } else {
                text
            };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), accent),
                Span::styled(
                    format!("{:<38}", super::clip(&session.title, 36)),
                    title_style,
                ),
                Span::styled(super::overlay::fmt_age(now, session.updated_at), dim),
            ]));
        }
        lines.push(Line::from(Span::styled(
            "    ↑↓ select · ↵ resume · /sessions for all",
            dim,
        )));
        lines.push(Line::default());
    }
    lines.extend(vec![
        Line::from(Span::styled("  start", dim)),
        Line::from(vec![
            Span::styled("    ❯ ", accent),
            Span::styled(
                "type what to build — plain words become a mission".to_string(),
                text,
            ),
        ]),
        Line::from(vec![
            Span::styled("    ❯ ", accent),
            Span::styled(
                "ask a question — the architect answers inline".to_string(),
                text,
            ),
        ]),
        Line::default(),
        Line::from(Span::styled("  commands", dim)),
        cmd("/run", "<objective>", "start a mission directly"),
        cmd("/models", "", "browse wired models"),
        cmd("/plan", "", "review the pending plan"),
        cmd("/help", "", "all keys and commands"),
    ]);
    frame.render_widget(Paragraph::new(lines), area);
}
