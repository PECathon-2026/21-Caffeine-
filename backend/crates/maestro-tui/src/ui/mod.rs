//! Widgets. Everything reads semantic theme tokens only (docs/UI-BRIEF.md).

pub mod approvals;
pub mod cards;
pub mod composer;
pub mod diff;
pub mod events;
pub mod header;
pub mod history;
pub mod md;
pub mod overlay;
pub mod sidebar;
pub mod statusline;
pub mod suggest;
pub mod tasks;
pub mod transcript;
pub mod welcome;

use maestro_types::domain::{Role, TaskStatus};
use unicode_width::UnicodeWidthStr;

/// Gutter grammar (UI-BRIEF): column-0 glyph tells the line kind.
pub const GUTTER_USER: &str = "❯";
pub const GUTTER_ASSISTANT: &str = "◆";
pub const GUTTER_TOOL: &str = "⏵";
pub const GUTTER_RESULT: &str = "⎿";
pub const GUTTER_SYSTEM: &str = "·";

/// Braille spinner (restrained; the one animation in the UI).
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}

/// Task status glyphs: ✓ ✗ ▸ ○ ⊘ ⏸.
pub fn task_glyph(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Completed => "✓",
        TaskStatus::Failed => "✗",
        TaskStatus::Running => "▸",
        TaskStatus::Proposed | TaskStatus::Ready => "○",
        TaskStatus::Blocked => "⊘",
    }
}

/// Single-letter role glyph for team rows and plan cards.
pub fn role_glyph(role: Role) -> &'static str {
    match role {
        Role::Planner => "将",
        Role::Designer => "将",
        Role::Coder => "侍",
        Role::Reviewer => "守",
        Role::Security => "影",
    }
}

/// Clip `s` to `max` display columns, appending `…` when clipped.
pub fn clip(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Display width helper.
pub fn dwidth(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}
