//! Chat transcript: flat Vec of pre-styled lines. Settled entries render once
//! and are cached; only the streaming tail re-renders per flush. Viewport
//! slice drawing only.

use crate::state::{ClientState, EntryKind, TranscriptEntry};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use std::collections::VecDeque;

/// Collapsed tool output cap (UI-BRIEF: "cap ~16 lines with +N more").
pub const TOOL_RESULT_COLLAPSED: usize = 4;
pub const TOOL_RESULT_EXPANDED: usize = 16;

pub fn gutter(kind: &EntryKind) -> (&'static str, bool) {
    match kind {
        EntryKind::User => (super::GUTTER_USER, false),
        EntryKind::Assistant => (super::GUTTER_ASSISTANT, false),
        EntryKind::Tool => (super::GUTTER_TOOL, true),
        EntryKind::ToolResult => (super::GUTTER_RESULT, true),
        EntryKind::System | EntryKind::Error => (super::GUTTER_SYSTEM, true),
        EntryKind::ApprovalCard { .. } | EntryKind::PlanCard { .. } => ("", true),
    }
}

/// Word-aware hard wrap to `width` display columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for word in raw_line.split(' ') {
            let ww = super::dwidth(word);
            if cur_w > 0 && cur_w + 1 + ww > width {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            if ww > width {
                // Hard-break an overlong word.
                if cur_w > 0 {
                    cur.push(' ');
                    cur_w += 1;
                }
                for ch in word.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if cur_w + cw > width {
                        out.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                    cur.push(ch);
                    cur_w += cw;
                }
                continue;
            }
            if cur_w > 0 {
                cur.push(' ');
                cur_w += 1;
            }
            cur.push_str(word);
            cur_w += ww;
        }
        out.push(cur);
    }
    out
}

fn kind_style(kind: &EntryKind, theme: &Theme) -> (Style, Style) {
    // (gutter style, text style)
    match kind {
        EntryKind::User => (
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme.text)
                .bg(theme.panel)
                .add_modifier(Modifier::BOLD),
        ),
        EntryKind::Assistant => (
            Style::default().fg(theme.accent),
            Style::default().fg(theme.text),
        ),
        EntryKind::Tool => (
            Style::default().fg(theme.dim),
            Style::default().fg(theme.dim),
        ),
        EntryKind::ToolResult => (
            Style::default().fg(theme.dim),
            Style::default().fg(theme.dim),
        ),
        EntryKind::System => (
            Style::default().fg(theme.dim),
            Style::default().fg(theme.dim),
        ),
        EntryKind::Error => (
            Style::default().fg(theme.error),
            Style::default().fg(theme.error),
        ),
        _ => (Style::default(), Style::default()),
    }
}

/// Render one settled entry to styled lines (pure).
pub fn entry_lines(
    entry: &TranscriptEntry,
    state: &ClientState,
    theme: &Theme,
    width: u16,
    tool_expanded: bool,
) -> Vec<Line<'static>> {
    match &entry.kind {
        EntryKind::ApprovalCard { approval_id } => {
            match state.approvals.iter().find(|a| a.id == *approval_id) {
                Some(a) => super::cards::approval_card(a, theme, width),
                None => vec![Line::from(Span::styled(
                    "· approval decided",
                    Style::default().fg(theme.dim),
                ))],
            }
        }
        EntryKind::PlanCard { mission_id } => {
            let pending = state.pending_plan.as_deref() == Some(mission_id.as_str());
            match (pending, state.plans.get(mission_id)) {
                (true, Some(plan)) => super::cards::plan_card(plan, theme, width),
                _ => vec![Line::from(Span::styled(
                    "· plan decided",
                    Style::default().fg(theme.dim),
                ))],
            }
        }
        // Assistant prose renders as markdown (bold/italic/code, bullets,
        // headings, fences); everything else stays verbatim.
        EntryKind::Assistant => {
            let (gstyle, _) = kind_style(&EntryKind::Assistant, theme);
            let body_w = (width as usize).saturating_sub(2);
            super::md::markdown_lines(&display_text(&entry.text), body_w, theme)
                .into_iter()
                .enumerate()
                .map(|(i, line)| {
                    let prefix = if i == 0 {
                        Span::styled(format!("{} ", super::GUTTER_ASSISTANT), gstyle)
                    } else {
                        Span::raw("  ")
                    };
                    let mut spans = vec![prefix];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect()
        }
        kind => {
            let (glyph, _) = gutter(kind);
            let (gstyle, tstyle) = kind_style(kind, theme);
            let body_w = (width as usize).saturating_sub(2);
            let mut wrapped = wrap(&display_text(&entry.text), body_w);
            if matches!(kind, EntryKind::ToolResult) {
                let cap = if tool_expanded {
                    TOOL_RESULT_EXPANDED
                } else {
                    TOOL_RESULT_COLLAPSED
                };
                if wrapped.len() > cap {
                    let extra = wrapped.len() - cap;
                    wrapped.truncate(cap);
                    wrapped.push(format!("+{extra} more (ctrl+o expands)"));
                }
            }
            wrapped
                .into_iter()
                .enumerate()
                .map(|(i, text)| {
                    let prefix = if i == 0 {
                        Span::styled(format!("{glyph} "), gstyle)
                    } else {
                        Span::raw("  ")
                    };
                    let padding = if matches!(kind, EntryKind::User) {
                        " ".repeat((width as usize).saturating_sub(2 + super::dwidth(&text)))
                    } else {
                        String::new()
                    };
                    Line::from(vec![
                        prefix,
                        Span::styled(format!("{text}{padding}"), tstyle),
                    ])
                    .style(tstyle)
                })
                .collect()
        }
    }
}

/// Streaming tail: re-rendered every flush (never cached).
pub fn tail_lines(
    state: &ClientState,
    theme: &Theme,
    width: u16,
    spinner_frame: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let body_w = (width as usize).saturating_sub(2);
    for (task_id, buf) in &state.streaming {
        let title = state
            .active_tasks()
            .iter()
            .find(|t| t.id == *task_id)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| task_id.chars().take(8).collect());
        out.push(Line::from(vec![
            Span::styled(
                format!("{} ", super::spinner(spinner_frame)),
                Style::default().fg(theme.accent),
            ),
            Span::styled(title, Style::default().fg(theme.dim)),
        ]));
        let wrapped = wrap(buf, body_w);
        let start = wrapped.len().saturating_sub(6);
        for text in &wrapped[start..] {
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(text.clone(), Style::default().fg(theme.text)),
            ]));
        }
    }
    if state.waiting_chat {
        out.push(Line::from(vec![
            Span::styled(
                format!("{} ", super::GUTTER_ASSISTANT),
                Style::default().fg(theme.accent),
            ),
            Span::styled(
                format!("{} thinking…", super::spinner(spinner_frame)),
                Style::default().fg(theme.dim),
            ),
        ]));
    }
    out
}

/// Settled-line cache. Rebuilds fully only on width/theme/epoch changes;
/// appends incrementally as entries arrive and, once the transcript is at
/// cap, drops exactly the wrapped lines of front-popped entries instead of
/// re-wrapping everything.
#[derive(Default)]
pub struct TranscriptView {
    width: u16,
    epoch: u64,
    tool_expanded: bool,
    /// `ClientState::transcript_dropped` value this cache has caught up to.
    dropped: u64,
    /// Number of transcript entries currently rendered into `lines`.
    synced: usize,
    /// (entry id, wrapped-line count) per rendered entry (parallels `lines`).
    entry_line_counts: VecDeque<(String, usize, bool)>,
    lines: VecDeque<Line<'static>>,
    /// Counts full cache rebuilds (regression guard for cap thrash).
    #[cfg(test)]
    pub rebuilds: u64,
}

impl TranscriptView {
    /// Bring the cache up to date with `state`. `epoch` bumps force a full
    /// re-render (theme change, approval decided, plan decided…).
    pub fn sync(
        &mut self,
        state: &ClientState,
        theme: &Theme,
        width: u16,
        epoch: u64,
        tool_expanded: bool,
    ) {
        let newly_dropped = state.transcript_dropped.saturating_sub(self.dropped);
        let mut stale = self.width != width
            || self.epoch != epoch
            || self.tool_expanded != tool_expanded
            // A reset state (e.g. a fresh `ClientState`) rewinds the counter.
            || state.transcript_dropped < self.dropped
            // More entries vanished than we ever rendered.
            || newly_dropped as usize > self.synced.min(self.entry_line_counts.len())
            || self.synced.saturating_sub(newly_dropped as usize) > state.transcript.len();
        if !stale {
            // Shed the entries the state popped off the front: we stored how
            // many wrapped lines each produced, so no re-wrap is needed.
            for _ in 0..newly_dropped {
                let (_, n, _) = self.entry_line_counts.pop_front().unwrap_or_default();
                self.lines.drain(..n.min(self.lines.len()));
                self.synced -= 1;
            }
            // Identity check: the cache front must still be the state front
            // (catches a cleared-and-refilled transcript of the same length,
            // e.g. /new without an epoch bump).
            let cache_front = self.entry_line_counts.front().map(|(id, _, _)| id.as_str());
            let state_front = state.transcript.front().map(|e| e.id.as_str());
            if self.synced > 0 && cache_front != state_front {
                stale = true;
            }
        }
        if stale {
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
            self.lines.clear();
            self.entry_line_counts.clear();
            self.synced = 0;
            self.width = width;
            self.epoch = epoch;
            self.tool_expanded = tool_expanded;
        }
        self.dropped = state.transcript_dropped;
        for entry in state.transcript.iter().skip(self.synced) {
            let rendered = entry_lines(entry, state, theme, width, tool_expanded);
            self.entry_line_counts.push_back((
                entry.id.clone(),
                rendered.len(),
                matches!(entry.kind, EntryKind::User),
            ));
            self.lines.extend(rendered);
        }
        self.synced = state.transcript.len();
    }

    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    /// Visible slice: `scroll` counts lines from the bottom (0 = follow).
    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        tail: &[Line<'static>],
        scroll: usize,
    ) {
        let height = area.height as usize;
        let total = self.lines.len() + tail.len();
        // Clamp to the first FULL viewport, not the first remaining line.
        // Also clamp here for resize, history reload, and tool-collapse changes.
        let scroll = scroll.min(total.saturating_sub(height));
        let end = total.saturating_sub(scroll);
        let mut start = end.saturating_sub(height);
        let mut offset = 0;
        let mut pinned = None;
        let mut latest_user = None;
        for (_, count, is_user) in &self.entry_line_counts {
            if *is_user {
                latest_user = Some((offset, *count));
                if offset < start {
                    pinned = Some((offset, *count));
                }
            }
            offset += count;
        }
        if scroll == 0 {
            // Follow the current turn. Short responses start below their own
            // request instead of pinning an unrelated earlier question.
            if let Some((offset, count)) = latest_user {
                if offset >= start {
                    start = offset;
                    pinned = None;
                } else {
                    pinned = Some((offset, count));
                }
            }
        }
        let mut visible: Vec<Line<'static>> = Vec::with_capacity(height);
        if let Some((offset, count)) = pinned {
            let count = count.min(height / 3);
            visible.extend(self.lines.iter().skip(offset).take(count).cloned());
            start = end.saturating_sub(height.saturating_sub(count));
        }
        for idx in start..end {
            if idx < self.lines.len() {
                visible.push(self.lines[idx].clone());
            } else {
                visible.push(tail[idx - self.lines.len()].clone());
            }
        }
        frame.render_widget(
            ratatui::widgets::Paragraph::new(visible).style(Style::default().bg(theme.bg)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_respects_width_and_newlines() {
        let lines = wrap("hello world this is maestro", 12);
        assert!(lines.iter().all(|l| crate::ui::dwidth(l) <= 12));
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
        // Overlong word is hard-broken.
        let broken = wrap("abcdefghijklmnop", 6);
        assert!(broken.len() > 1);
        assert!(broken.iter().all(|l| crate::ui::dwidth(l) <= 6));
    }

    #[test]
    fn tool_result_collapses_with_more_marker() {
        let state = ClientState::new("s".into());
        let entry = TranscriptEntry {
            id: "e1".into(),
            kind: EntryKind::ToolResult,
            text: (0..12)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let lines = entry_lines(&entry, &state, &Theme::dark(), 80, false);
        assert_eq!(lines.len(), TOOL_RESULT_COLLAPSED + 1);
        let last: String = lines
            .last()
            .unwrap()
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(last.contains("+8 more"));
        // Expanded shows everything (12 < 16).
        let lines = entry_lines(&entry, &state, &Theme::dark(), 80, true);
        assert_eq!(lines.len(), 12);
    }

    /// Flatten cached lines to plain text for content comparison.
    fn texts(view: &TranscriptView) -> Vec<String> {
        view.lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    #[test]
    fn cache_at_cap_drops_front_without_rebuilding() {
        use crate::state::TRANSCRIPT_CAP;
        let theme = Theme::dark();
        let mut state = ClientState::new("s".into());
        // Fill past the cap so entries are already being popped off the front.
        for i in 0..(TRANSCRIPT_CAP + 5) {
            state.push_entry(EntryKind::System, format!("entry {i} with some words"));
        }
        let mut view = TranscriptView::default();
        view.sync(&state, &theme, 80, 0, false);
        let rebuilds_after_fill = view.rebuilds;

        // One more entry pops one off the front: must NOT trigger a rebuild.
        state.push_entry(EntryKind::System, "the straw entry");
        view.sync(&state, &theme, 80, 0, false);
        assert_eq!(
            view.rebuilds, rebuilds_after_fill,
            "adding one entry at cap must not rebuild the whole cache"
        );

        // Content must match a from-scratch rebuild exactly.
        let mut fresh = TranscriptView::default();
        fresh.sync(&state, &theme, 80, 0, false);
        assert_eq!(texts(&view), texts(&fresh));
        assert_eq!(view.total_lines(), fresh.total_lines());

        // Several more pushes (multi-line entries too) stay incremental.
        for i in 0..3 {
            state.push_entry(EntryKind::User, format!("multi\nline\nentry {i}"));
        }
        view.sync(&state, &theme, 80, 0, false);
        assert_eq!(view.rebuilds, rebuilds_after_fill);
        let mut fresh = TranscriptView::default();
        fresh.sync(&state, &theme, 80, 0, false);
        assert_eq!(texts(&view), texts(&fresh));

        // Width change still forces a full rebuild.
        view.sync(&state, &theme, 40, 0, false);
        assert_eq!(view.rebuilds, rebuilds_after_fill + 1);
        let mut fresh = TranscriptView::default();
        fresh.sync(&state, &theme, 40, 0, false);
        assert_eq!(texts(&view), texts(&fresh));
    }

    #[test]
    fn cache_rebuilds_when_transcript_cleared() {
        let theme = Theme::dark();
        let mut state = ClientState::new("s".into());
        state.push_entry(EntryKind::User, "hello");
        let mut view = TranscriptView::default();
        view.sync(&state, &theme, 80, 0, false);
        assert_eq!(view.total_lines(), 1);
        // /new clears the transcript without bumping the drop counter.
        state.transcript.clear();
        state.push_entry(EntryKind::System, "new session");
        view.sync(&state, &theme, 80, 0, false);
        let mut fresh = TranscriptView::default();
        fresh.sync(&state, &theme, 80, 0, false);
        assert_eq!(texts(&view), texts(&fresh));
    }

    #[test]
    fn cache_appends_and_invalidates_on_width_change() {
        let mut state = ClientState::new("s".into());
        state.push_entry(EntryKind::User, "hi");
        let theme = Theme::dark();
        let mut view = TranscriptView::default();
        view.sync(&state, &theme, 80, 0, false);
        assert_eq!(view.total_lines(), 1);
        state.push_entry(EntryKind::Assistant, "hello there");
        view.sync(&state, &theme, 80, 0, false);
        assert_eq!(view.total_lines(), 2);
        // Width change → full rebuild, same content.
        view.sync(&state, &theme, 40, 0, false);
        assert_eq!(view.total_lines(), 2);
    }
}

/// Keep private content-address identifiers out of the rendered conversation.
fn display_text(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[image:") {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find("]]") {
            let id = &rest[8..end];
            if id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()) {
                output.push_str("[image attached]");
                rest = &rest[end + 2..];
                continue;
            }
        }
        output.push_str("[[image:");
        rest = &rest[8..];
    }
    output.push_str(rest);
    output
}
