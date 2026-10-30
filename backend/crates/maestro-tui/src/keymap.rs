//! Single source of truth for keybindings: action enum, default bindings per
//! context, optional user overlay (`~/.config/maestro/keybindings.json`), and
//! the live table `/help` renders.

use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Help,
    Palette,
    ToggleSidebar,
    ToggleToolDetail,
    TabChat,
    TabTasks,
    TabDiff,
    TabApprovals,
    TabEvents,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollBottom,
    Up,
    Down,
    Select,
    Escape,
    Approve,
    Reject,
    ViewDiff,
    Retry,
    Interrupt,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Help => "help",
            Action::Palette => "palette",
            Action::ToggleSidebar => "toggle-sidebar",
            Action::ToggleToolDetail => "toggle-tool-detail",
            Action::TabChat => "tab-chat",
            Action::TabTasks => "tab-tasks",
            Action::TabDiff => "tab-diff",
            Action::TabApprovals => "tab-approvals",
            Action::TabEvents => "tab-events",
            Action::ScrollUp => "scroll-up",
            Action::ScrollDown => "scroll-down",
            Action::PageUp => "page-up",
            Action::PageDown => "page-down",
            Action::ScrollBottom => "scroll-bottom",
            Action::Up => "up",
            Action::Down => "down",
            Action::Select => "select",
            Action::Escape => "escape",
            Action::Approve => "approve",
            Action::Reject => "reject",
            Action::ViewDiff => "view-diff",
            Action::Retry => "retry",
            Action::Interrupt => "interrupt",
        }
    }

    pub fn parse(s: &str) -> Option<Action> {
        Some(match s {
            "quit" => Action::Quit,
            "help" => Action::Help,
            "palette" => Action::Palette,
            "toggle-sidebar" => Action::ToggleSidebar,
            "toggle-tool-detail" => Action::ToggleToolDetail,
            "tab-chat" => Action::TabChat,
            "tab-tasks" => Action::TabTasks,
            "tab-diff" => Action::TabDiff,
            "tab-approvals" => Action::TabApprovals,
            "tab-events" => Action::TabEvents,
            "scroll-up" => Action::ScrollUp,
            "scroll-down" => Action::ScrollDown,
            "page-up" => Action::PageUp,
            "page-down" => Action::PageDown,
            "scroll-bottom" => Action::ScrollBottom,
            "up" => Action::Up,
            "down" => Action::Down,
            "select" => Action::Select,
            "escape" => Action::Escape,
            "approve" => Action::Approve,
            "reject" => Action::Reject,
            "view-diff" => Action::ViewDiff,
            "retry" => Action::Retry,
            "interrupt" => Action::Interrupt,
            _ => return None,
        })
    }
}

/// Binding context: `Global` applies everywhere; view contexts win over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    Global,
    Tasks,
    Approvals,
    Events,
    Diff,
    /// Plan-approval card focused in the chat flow.
    PlanCard,
}

impl Ctx {
    fn parse(s: &str) -> Option<Ctx> {
        Some(match s {
            "global" => Ctx::Global,
            "tasks" => Ctx::Tasks,
            "approvals" => Ctx::Approvals,
            "events" => Ctx::Events,
            "diff" => Ctx::Diff,
            "plan" | "plan-card" => Ctx::PlanCard,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Ctx::Global => "global",
            Ctx::Tasks => "tasks",
            Ctx::Approvals => "approvals",
            Ctx::Events => "events",
            Ctx::Diff => "diff",
            Ctx::PlanCard => "plan",
        }
    }
}

/// A physical key chord: code + modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Chord { code, mods }
    }

    /// Parse "ctrl+p", "alt+enter", "pageup", "j", "shift+tab", "f5"…
    pub fn parse(s: &str) -> Option<Chord> {
        let mut mods = KeyModifiers::NONE;
        let mut code: Option<KeyCode> = None;
        for part in s.split('+') {
            let part = part.trim().to_ascii_lowercase();
            match part.as_str() {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                key => {
                    code = Some(match key {
                        "enter" | "return" => KeyCode::Enter,
                        "esc" | "escape" => KeyCode::Esc,
                        "tab" => KeyCode::Tab,
                        "backspace" => KeyCode::Backspace,
                        "space" => KeyCode::Char(' '),
                        "up" => KeyCode::Up,
                        "down" => KeyCode::Down,
                        "left" => KeyCode::Left,
                        "right" => KeyCode::Right,
                        "home" => KeyCode::Home,
                        "end" => KeyCode::End,
                        "pageup" | "pgup" => KeyCode::PageUp,
                        "pagedown" | "pgdn" => KeyCode::PageDown,
                        "delete" | "del" => KeyCode::Delete,
                        k if k.len() == 1 => KeyCode::Char(k.chars().next()?),
                        k if k.starts_with('f') => KeyCode::F(k[1..].parse().ok()?),
                        _ => return None,
                    });
                }
            }
        }
        Some(Chord { code: code?, mods })
    }

    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_string());
        }
        if self.mods.contains(KeyModifiers::ALT) {
            parts.push("alt".to_string());
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_string());
        }
        parts.push(match self.code {
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_ascii_lowercase(),
        });
        parts.join("+")
    }
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub ctx: Ctx,
    pub chord: Chord,
    pub action: Action,
}

/// All bindings, defaults first, user overlay bindings appended (an overlay
/// binding wins because lookup scans back-to-front).
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::default_map()
    }
}

impl Keymap {
    pub fn default_map() -> Keymap {
        use Action as A;
        use Ctx::*;
        use KeyModifiers as M;
        let b = |ctx, chord: &str, action| Binding {
            ctx,
            chord: Chord::parse(chord).unwrap_or(Chord::new(KeyCode::Null, M::NONE)),
            action,
        };
        Keymap {
            bindings: vec![
                b(Global, "ctrl+p", A::Palette),
                b(Global, "ctrl+b", A::ToggleSidebar),
                b(Global, "ctrl+o", A::ToggleToolDetail),
                b(Global, "esc", A::Escape),
                b(Global, "pageup", A::PageUp),
                b(Global, "pagedown", A::PageDown),
                b(Global, "end", A::ScrollBottom),
                // Number tabs apply only while the composer is empty (app-gated).
                b(Global, "1", A::TabChat),
                b(Global, "2", A::TabTasks),
                b(Global, "3", A::TabDiff),
                b(Global, "4", A::TabApprovals),
                b(Global, "5", A::TabEvents),
                b(Tasks, "j", A::Down),
                b(Tasks, "k", A::Up),
                b(Tasks, "enter", A::Select),
                b(Tasks, "r", A::Retry),
                b(Tasks, "d", A::ViewDiff),
                b(Approvals, "j", A::Down),
                b(Approvals, "k", A::Up),
                b(Approvals, "a", A::Approve),
                b(Approvals, "x", A::Reject),
                b(Approvals, "v", A::ViewDiff),
                b(Events, "j", A::Down),
                b(Events, "k", A::Up),
                b(Events, "end", A::ScrollBottom),
                b(Diff, "j", A::Down),
                b(Diff, "k", A::Up),
                b(PlanCard, "a", A::Approve),
                b(PlanCard, "d", A::Reject),
            ],
        }
    }

    /// Apply a user overlay: `{"global": {"ctrl+p": "palette"}, "tasks": {...}}`.
    /// Also accepts a flat map, treated as `global`. Unknown keys/actions are
    /// skipped; a `null`/"none" action unbinds the chord in that context.
    pub fn apply_overlay(&mut self, overlay: &serde_json::Value) {
        let Some(map) = overlay.as_object() else {
            return;
        };
        // Flat form: values are strings → treat as global.
        let flat = map.values().all(|v| v.is_string() || v.is_null());
        if flat {
            self.apply_ctx_overlay(Ctx::Global, map);
            return;
        }
        for (ctx_name, section) in map {
            let (Some(ctx), Some(obj)) = (Ctx::parse(ctx_name), section.as_object()) else {
                continue;
            };
            self.apply_ctx_overlay(ctx, obj);
        }
    }

    fn apply_ctx_overlay(&mut self, ctx: Ctx, map: &serde_json::Map<String, serde_json::Value>) {
        for (chord_s, action_v) in map {
            let Some(chord) = Chord::parse(chord_s) else {
                continue;
            };
            let unbind = action_v.is_null() || action_v.as_str() == Some("none");
            if unbind {
                self.bindings
                    .retain(|b| !(b.ctx == ctx && b.chord == chord));
                continue;
            }
            let Some(action) = action_v.as_str().and_then(Action::parse) else {
                continue;
            };
            self.bindings.push(Binding { ctx, chord, action });
        }
    }

    /// Load defaults + `~/.config/maestro/keybindings.json` overlay.
    pub fn load() -> Keymap {
        let mut km = Keymap::default_map();
        if let Some(path) = crate::theme::config_file("keybindings.json") {
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                    km.apply_overlay(&json);
                }
            }
        }
        km
    }

    /// Context binding wins over Global; later (overlay) bindings win over
    /// earlier (default) ones.
    pub fn lookup(&self, ctx: Ctx, code: KeyCode, mods: KeyModifiers) -> Option<Action> {
        let hit = |want: Ctx| {
            self.bindings
                .iter()
                .rev()
                .find(|b| b.ctx == want && b.chord.code == code && b.chord.mods == mods)
                .map(|b| b.action)
        };
        hit(ctx).or_else(|| {
            if ctx != Ctx::Global {
                hit(Ctx::Global)
            } else {
                None
            }
        })
    }

    /// Live table for `/help`: (context, chord, action) rows.
    pub fn table(&self) -> Vec<(String, String, String)> {
        // Effective bindings only: last binding per (ctx, chord) wins.
        let mut rows: Vec<(String, String, String)> = Vec::new();
        let mut seen: Vec<(Ctx, Chord)> = Vec::new();
        for b in self.bindings.iter().rev() {
            if seen.contains(&(b.ctx, b.chord)) {
                continue;
            }
            seen.push((b.ctx, b.chord));
            rows.push((
                b.ctx.name().to_string(),
                b.chord.display(),
                b.action.name().to_string(),
            ));
        }
        rows.reverse();
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chords() {
        assert_eq!(
            Chord::parse("ctrl+p"),
            Some(Chord::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            Chord::parse("alt+enter"),
            Some(Chord::new(KeyCode::Enter, KeyModifiers::ALT))
        );
        assert_eq!(
            Chord::parse("pageup"),
            Some(Chord::new(KeyCode::PageUp, KeyModifiers::NONE))
        );
        assert_eq!(
            Chord::parse("f5"),
            Some(Chord::new(KeyCode::F(5), KeyModifiers::NONE))
        );
        assert_eq!(Chord::parse("ctrl+"), None);
        assert_eq!(Chord::parse("bogus"), None);
    }

    #[test]
    fn default_lookup_context_beats_global() {
        let km = Keymap::default_map();
        // 'a' approves in Approvals context…
        assert_eq!(
            km.lookup(Ctx::Approvals, KeyCode::Char('a'), KeyModifiers::NONE),
            Some(Action::Approve)
        );
        // …and is unbound globally.
        assert_eq!(
            km.lookup(Ctx::Global, KeyCode::Char('a'), KeyModifiers::NONE),
            None
        );
        // Global fallback still works from a view context.
        assert_eq!(
            km.lookup(Ctx::Tasks, KeyCode::Char('p'), KeyModifiers::CONTROL),
            Some(Action::Palette)
        );
    }

    #[test]
    fn overlay_rebinds_and_unbinds() {
        let mut km = Keymap::default_map();
        km.apply_overlay(&serde_json::json!({
            "global": { "ctrl+k": "palette", "ctrl+p": null },
            "tasks": { "x": "retry" },
            "nonsense-ctx": { "q": "quit" }
        }));
        assert_eq!(
            km.lookup(Ctx::Global, KeyCode::Char('k'), KeyModifiers::CONTROL),
            Some(Action::Palette)
        );
        assert_eq!(
            km.lookup(Ctx::Global, KeyCode::Char('p'), KeyModifiers::CONTROL),
            None
        );
        assert_eq!(
            km.lookup(Ctx::Tasks, KeyCode::Char('x'), KeyModifiers::NONE),
            Some(Action::Retry)
        );
    }

    #[test]
    fn flat_overlay_treated_as_global() {
        let mut km = Keymap::default_map();
        km.apply_overlay(&serde_json::json!({ "ctrl+q": "quit" }));
        assert_eq!(
            km.lookup(Ctx::Global, KeyCode::Char('q'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn table_lists_effective_bindings_once() {
        let mut km = Keymap::default_map();
        km.apply_overlay(&serde_json::json!({ "global": { "ctrl+p": "help" } }));
        let rows = km.table();
        let palette_rows: Vec<_> = rows
            .iter()
            .filter(|(c, k, _)| c == "global" && k == "ctrl+p")
            .collect();
        assert_eq!(palette_rows.len(), 1);
        assert_eq!(palette_rows[0].2, "help");
    }
}
