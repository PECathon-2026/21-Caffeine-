//! Semantic color tokens. All widgets read tokens only — no literal colors
//! anywhere else in the crate. Built-in dark + light themes, with an optional
//! user overlay from `~/.config/maestro/theme.json`.

use ratatui::style::Color;

/// Flat-minimalist token set (see docs/UI-BRIEF.md §Theming).
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub bg: Color,
    pub panel: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub error: Color,
    pub warn: Color,
    pub ok: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    pub border: Color,
    /// Foreground painted over `accent` for the reversed-video mode block.
    pub mode_fg: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            bg: Color::Reset,
            panel: Color::Rgb(24, 26, 32),
            text: Color::Rgb(205, 209, 217),
            dim: Color::Rgb(110, 118, 133),
            accent: Color::Rgb(122, 162, 247),
            error: Color::Rgb(224, 108, 117),
            warn: Color::Rgb(216, 166, 87),
            ok: Color::Rgb(120, 190, 130),
            diff_added_bg: Color::Rgb(22, 48, 28),
            diff_removed_bg: Color::Rgb(58, 26, 28),
            border: Color::Rgb(58, 63, 76),
            mode_fg: Color::Rgb(14, 15, 18),
        }
    }

    pub fn light() -> Self {
        Theme {
            bg: Color::Reset,
            panel: Color::Rgb(240, 240, 244),
            text: Color::Rgb(38, 42, 51),
            dim: Color::Rgb(130, 136, 148),
            accent: Color::Rgb(46, 89, 168),
            error: Color::Rgb(170, 50, 58),
            warn: Color::Rgb(146, 100, 20),
            ok: Color::Rgb(38, 120, 60),
            diff_added_bg: Color::Rgb(216, 240, 220),
            diff_removed_bg: Color::Rgb(248, 218, 220),
            border: Color::Rgb(196, 200, 210),
            mode_fg: Color::Rgb(250, 250, 252),
        }
    }

    /// Apply a flat `{token: "color"}` JSON overlay. Unknown tokens are
    /// ignored; unparseable colors leave the built-in value.
    pub fn apply_overlay(&mut self, overlay: &serde_json::Value) {
        let Some(map) = overlay.as_object() else {
            return;
        };
        for (key, val) in map {
            let Some(color) = val.as_str().and_then(parse_color) else {
                continue;
            };
            match key.as_str() {
                "bg" => self.bg = color,
                "panel" => self.panel = color,
                "text" => self.text = color,
                "dim" => self.dim = color,
                "accent" => self.accent = color,
                "error" => self.error = color,
                "warn" => self.warn = color,
                "ok" => self.ok = color,
                "diffAddedBg" | "diff_added_bg" => self.diff_added_bg = color,
                "diffRemovedBg" | "diff_removed_bg" => self.diff_removed_bg = color,
                "border" => self.border = color,
                "modeFg" | "mode_fg" => self.mode_fg = color,
                _ => {}
            }
        }
    }

    /// Built-in dark theme with the user overlay from
    /// `$XDG_CONFIG_HOME/maestro/theme.json` (missing/invalid file = defaults).
    /// Overlay key `"base": "light"` selects the light base first.
    pub fn load() -> Self {
        let mut theme = Theme::dark();
        let Some(path) = config_file("theme.json") else {
            return theme;
        };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return theme;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return theme;
        };
        if json.get("base").and_then(|v| v.as_str()) == Some("light") {
            theme = Theme::light();
        }
        theme.apply_overlay(&json);
        theme
    }
}

/// `$XDG_CONFIG_HOME/maestro/<name>` (fallback `~/.config/maestro/<name>`).
pub fn config_file(name: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("maestro").join(name))
}

/// Parse `#rrggbb`, `#rgb`, ANSI index (`"246"`), or a named color.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::Rgb(r, g, b))
            }
            3 => {
                let d = |i: usize| u8::from_str_radix(&hex[i..=i], 16).map(|v| v * 17);
                Some(Color::Rgb(d(0).ok()?, d(1).ok()?, d(2).ok()?))
            }
            _ => None,
        };
    }
    if let Ok(idx) = s.parse::<u8>() {
        return Some(Color::Indexed(idx));
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        "default" | "reset" | "none" => Some(Color::Reset),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_named_colors() {
        assert_eq!(parse_color("#7aa2f7"), Some(Color::Rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(parse_color("#fff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_color("red"), Some(Color::Red));
        assert_eq!(parse_color("246"), Some(Color::Indexed(246)));
        assert_eq!(parse_color("reset"), Some(Color::Reset));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("not-a-color"), None);
    }

    #[test]
    fn overlay_overrides_only_known_tokens() {
        let mut t = Theme::dark();
        let default_text = t.text;
        t.apply_overlay(&serde_json::json!({
            "accent": "#ff0000",
            "diff_added_bg": "green",
            "bogus": "#00ff00",
            "text": "not-a-color"
        }));
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert_eq!(t.diff_added_bg, Color::Green);
        assert_eq!(t.text, default_text); // bad value ignored
    }

    #[test]
    fn overlay_ignores_non_object() {
        let mut t = Theme::dark();
        let before = t.clone();
        t.apply_overlay(&serde_json::json!(["nope"]));
        assert_eq!(t, before);
    }
}
