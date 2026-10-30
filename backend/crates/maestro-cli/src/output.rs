//! Human-readable output helpers: minimal manual ANSI (no color crates),
//! aligned tables, event-line formatting, and small format utilities.
//!
//! Convention: dim for secondary text, one accent color (cyan), green/red
//! only for diff +/- tinting and doctor verdict glyphs. Everything degrades
//! to plain text when stdout is not a TTY, `--no-color` is set, `NO_COLOR`
//! is present, or `--json` is active.

use maestro_types::event::{kind, EventRecord};
use maestro_types::now_ms;

/// ANSI on/off switch threaded through all human-readable rendering.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    pub fn plain() -> Style {
        Style { color: false }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }

    /// The single accent color (cyan).
    pub fn accent(&self, s: &str) -> String {
        self.wrap("36", s)
    }

    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }

    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
}

/// Render an aligned two-space-separated table. The header row is dimmed.
/// Pure function (unit-tested); ANSI is applied after width computation so
/// alignment never counts escape bytes.
pub fn render_table(headers: &[&str], rows: &[Vec<String>], style: &Style) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            let w = cell.chars().count();
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }
    let mut out = String::new();
    let mut header_line = String::new();
    for (i, h) in headers.iter().enumerate() {
        if i > 0 {
            header_line.push_str("  ");
        }
        header_line.push_str(&pad(h, widths[i], i + 1 == cols));
    }
    out.push_str(&style.dim(header_line.trim_end()));
    out.push('\n');
    for row in rows {
        let mut line = String::new();
        for (i, width) in widths.iter().enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            line.push_str(&pad(cell, *width, i + 1 == cols));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn pad(s: &str, width: usize, last: bool) -> String {
    if last {
        return s.to_string();
    }
    let len = s.chars().count();
    let mut out = String::with_capacity(width + 2);
    out.push_str(s);
    for _ in len..width {
        out.push(' ');
    }
    out
}

/// First `n` chars of an id (ids are 32-char hex; 12 is unambiguous enough).
pub fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Clip a one-line summary for table cells.
pub fn clip(s: &str, max: usize) -> String {
    let single = s.replace('\n', " ");
    if single.chars().count() <= max {
        return single;
    }
    let cut: String = single.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Micro-USD to a dollar string.
pub fn fmt_cost(micro_usd: i64) -> String {
    format!("${:.4}", micro_usd as f64 / 1_000_000.0)
}

/// Human "time ago" for unix-ms timestamps.
pub fn ago(ts_ms: i64) -> String {
    let delta = (now_ms() - ts_ms).max(0) / 1000;
    match delta {
        0..=59 => format!("{delta}s ago"),
        60..=3599 => format!("{}m ago", delta / 60),
        3600..=86_399 => format!("{}h ago", delta / 3600),
        _ => format!("{}d ago", delta / 86_400),
    }
}

fn payload_str(ev: &EventRecord, key: &str) -> String {
    ev.payload
        .get(key)
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

/// First non-empty payload value among `keys` (for renamed payload keys:
/// new name first, legacy fallback for events persisted by older builds).
fn payload_str_any(ev: &EventRecord, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| payload_str(ev, k))
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// One progress line per interesting event kind; `None` filters noise
/// (raw agent turns, tool results, heartbeats). Used by `run` and the
/// daemon-follow paths.
pub fn event_line(ev: &EventRecord, style: &Style) -> Option<String> {
    let task = ev
        .task_id
        .as_deref()
        .map(|t| format!("[{}] ", &short(t)[..8.min(t.len())]))
        .unwrap_or_default();
    let line = match ev.kind.as_str() {
        kind::MISSION_STATUS => {
            let status = payload_str(ev, "status");
            let reason = payload_str(ev, "reason");
            let mut s = format!("mission → {}", style.bold(&status));
            if !reason.is_empty() && reason != "null" {
                s.push_str(&style.dim(&format!(" ({reason})")));
            }
            s
        }
        kind::PLAN_PROPOSED => format!(
            "plan proposed: {} tasks — {}",
            payload_str(ev, "tasks"),
            clip(&payload_str(ev, "summary"), 80)
        ),
        kind::PLAN_APPROVED => "plan approved".to_string(),
        kind::PLAN_REJECTED => "plan rejected".to_string(),
        kind::TASK_LEASED => style.dim(&format!(
            "{task}leased (attempt {})",
            payload_str(ev, "attempt")
        )),
        kind::TASK_STATUS => {
            let status = payload_str(ev, "status");
            let summary = payload_str(ev, "summary");
            let mut s = format!("{task}task → {}", style.bold(&status));
            if !summary.is_empty() && summary != "null" {
                s.push_str(&style.dim(&format!(" — {}", clip(&summary, 80))));
            }
            s
        }
        kind::TASK_RETRY => format!(
            "{task}retry (attempt {}): {}",
            payload_str(ev, "attempt"),
            clip(&payload_str(ev, "error"), 80)
        ),
        kind::TOOL_CALL => style.dim(&format!(
            "{task}  {} {}",
            payload_str(ev, "tool"),
            // The orchestrator emits "arguments"; "args" is the legacy key.
            clip(&payload_str_any(ev, &["arguments", "args"]), 100)
        )),
        kind::TOOL_DENIED => format!(
            "{task}tool denied: {} — {}",
            payload_str(ev, "tool"),
            clip(&payload_str(ev, "error"), 100)
        ),
        kind::LOOP_DETECTED => format!(
            "{task}loop detected: {} × {}",
            payload_str(ev, "repeats"),
            payload_str(ev, "tool")
        ),
        kind::SCOPE_VIOLATION => {
            format!(
                "{task}scope violation: {}",
                clip(&payload_str(ev, "error"), 100)
            )
        }
        kind::BUDGET_EXHAUSTED => format!(
            "budget exhausted ({}): {} tokens, {} µ$",
            payload_str(ev, "budget"),
            payload_str(ev, "totalTokens"),
            payload_str(ev, "costMicroUsd")
        ),
        kind::WORKTREE_CREATED => style.dim(&format!(
            "{task}worktree {} ({})",
            payload_str(ev, "path"),
            payload_str(ev, "branch")
        )),
        kind::GATE_RECORDED => format!(
            "{task}gate: {} → {} by {}",
            payload_str(ev, "coderTask"),
            payload_str(ev, "verdict"),
            payload_str(ev, "role")
        ),
        kind::MERGE_ENQUEUED => format!("{task}merge enqueued"),
        kind::MERGE_STARTED => style.dim(&format!("{task}merge started")),
        kind::MERGE_COMPLETED => format!("{task}merged ({})", payload_str(ev, "commit")),
        kind::MERGE_CONFLICT => format!("{task}merge conflict"),
        kind::MERGE_ROLLED_BACK => format!(
            "{task}merge rolled back: {}",
            clip(&payload_str(ev, "reason"), 80)
        ),
        kind::CHECK_STARTED => style.dim(&format!("{task}check {}", payload_str(ev, "name"))),
        kind::CHECK_FINISHED => style.dim(&format!(
            "{task}check {} → {}",
            payload_str(ev, "name"),
            payload_str(ev, "ok")
        )),
        kind::APPROVAL_REQUESTED => format!(
            "approval required: {} {} {}  (maestro approve {})",
            payload_str(ev, "action"),
            payload_str(ev, "resource"),
            style.dim(&payload_str(ev, "approvalId")),
            payload_str(ev, "approvalId")
        ),
        kind::APPROVAL_DECIDED => format!(
            "approval {} → {}",
            short(&payload_str(ev, "approvalId")),
            payload_str(ev, "status")
        ),
        kind::DIRECTIVE_ADDED => format!("steer: {}", clip(&payload_str(ev, "text"), 80)),
        kind::PROVIDER_RETRY => style.dim(&format!(
            "{task}provider retry {} (wait {}ms)",
            payload_str(ev, "model"),
            payload_str(ev, "waitMs")
        )),
        kind::PROVIDER_FALLBACK => style.dim(&format!(
            "{task}provider fallback {} → {}",
            payload_str(ev, "from"),
            payload_str(ev, "to")
        )),
        kind::SANDBOX_FALLBACK => format!("{task}sandbox unavailable; ran on host"),
        kind::INTEGRATION_FINALIZED => format!(
            "integration finalized: {} on {}",
            payload_str(ev, "commit"),
            payload_str(ev, "branch")
        ),
        _ => return None,
    };
    Some(line)
}

/// Raw audit line for `maestro events`: every kind, compact payload.
pub fn raw_event_line(ev: &EventRecord, style: &Style) -> String {
    let task = ev.task_id.as_deref().map(short).unwrap_or_default();
    format!(
        "{:>6}  {:<24} {:<12} {}",
        ev.seq,
        ev.kind,
        style.dim(&task),
        clip(&ev.payload.to_string(), 160)
    )
}

/// Print a unified diff with +/- tint when color is on.
pub fn tint_diff(unified: &str, style: &Style) -> String {
    if !style.color {
        return unified.to_string();
    }
    let mut out = String::with_capacity(unified.len() + 64);
    for line in unified.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            out.push_str(&style.green(line));
        } else if line.starts_with('-') && !line.starts_with("---") {
            out.push_str(&style.red(line));
        } else if line.starts_with("@@") {
            out.push_str(&style.accent(line));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::event::kind;

    #[test]
    fn table_aligns_columns_plain() {
        let style = Style::plain();
        let rows = vec![
            vec!["a".to_string(), "long-cell".to_string(), "x".to_string()],
            vec!["bbbb".to_string(), "c".to_string(), "y".to_string()],
        ];
        let table = render_table(&["ID", "NAME", "S"], &rows, &style);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "ID    NAME       S");
        assert_eq!(lines[1], "a     long-cell  x");
        assert_eq!(lines[2], "bbbb  c          y");
    }

    #[test]
    fn table_tolerates_short_rows() {
        let style = Style::plain();
        let rows = vec![vec!["only".to_string()]];
        let table = render_table(&["A", "B"], &rows, &style);
        assert!(table.contains("only"));
    }

    #[test]
    fn ansi_disabled_when_color_off() {
        let style = Style::plain();
        assert_eq!(style.dim("x"), "x");
        assert_eq!(style.accent("x"), "x");
        let on = Style { color: true };
        assert_eq!(on.dim("x"), "\x1b[2mx\x1b[0m");
    }

    #[test]
    fn clip_and_short_and_cost() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
        assert_eq!(short("0123456789abcdef0123456789abcdef"), "0123456789ab");
        assert_eq!(fmt_cost(1_234_500), "$1.2345");
        assert_eq!(fmt_cost(0), "$0.0000");
    }

    fn ev(kind: &str, payload: serde_json::Value, task: Option<&str>) -> EventRecord {
        EventRecord {
            seq: 1,
            mission_id: "m".into(),
            task_id: task.map(str::to_string),
            kind: kind.to_string(),
            payload,
            created_at: 0,
        }
    }

    #[test]
    fn event_lines_cover_key_kinds_and_skip_noise() {
        let style = Style::plain();
        let line = event_line(
            &ev(
                kind::TASK_STATUS,
                serde_json::json!({"status": "completed", "summary": "did it"}),
                Some("0123456789abcdef"),
            ),
            &style,
        )
        .expect("task.status renders");
        assert!(line.contains("completed"));
        assert!(line.contains("[01234567]"));

        let tool = event_line(
            &ev(
                kind::TOOL_CALL,
                serde_json::json!({"tool": "read_file", "arguments": {"path": "x.rs"}}),
                None,
            ),
            &style,
        )
        .expect("tool.call renders");
        assert!(tool.contains("read_file"));
        assert!(tool.contains("x.rs"), "arguments payload rendered: {tool}");

        // Legacy events persisted with the old "args" key still render.
        let legacy = event_line(
            &ev(
                kind::TOOL_CALL,
                serde_json::json!({"tool": "read_file", "args": {"path": "old.rs"}}),
                None,
            ),
            &style,
        )
        .expect("legacy tool.call renders");
        assert!(legacy.contains("old.rs"), "legacy args rendered: {legacy}");

        assert!(event_line(
            &ev(kind::AGENT_TURN, serde_json::json!({"raw": "…"}), None),
            &style
        )
        .is_none());
        assert!(event_line(&ev(kind::TOOL_RESULT, serde_json::json!({}), None), &style).is_none());
    }

    #[test]
    fn diff_tint_only_when_color() {
        let plain = Style::plain();
        let diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(tint_diff(diff, &plain), diff);
        let on = Style { color: true };
        let tinted = tint_diff(diff, &on);
        assert!(tinted.contains("\x1b[32m+new\x1b[0m"));
        assert!(tinted.contains("\x1b[31m-old\x1b[0m"));
        assert!(tinted.contains("--- a\n"));
    }
}
