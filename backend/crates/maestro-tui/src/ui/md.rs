//! Minimal markdown renderer for assistant turns: bold / italic / inline
//! code, headings, bullets, and fenced code blocks — pre-styled Lines wrapped
//! to width, so entries render once and cache like any other transcript line.
//! Unmatched markers stay literal: never eat a user-visible character.

use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Inline style classes; resolved to a ratatui Style per theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Bold,
    Italic,
    Code,
}

fn tone_style(tone: Tone, theme: &Theme) -> Style {
    match tone {
        Tone::Plain => Style::default().fg(theme.text),
        Tone::Bold => Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        Tone::Italic => Style::default()
            .fg(theme.text)
            .add_modifier(Modifier::ITALIC),
        Tone::Code => Style::default().fg(theme.accent),
    }
}

/// Parse one paragraph's inline markup into (text, tone) runs. Flat, no
/// nesting: `**a `b` c**` bolds the whole run and leaves backticks literal
/// inside. A marker without a closer is emitted literally.
fn inline_runs(text: &str) -> Vec<(String, Tone)> {
    let chars: Vec<char> = text.chars().collect();
    let mut runs: Vec<(String, Tone)> = Vec::new();
    let mut cur = String::new();
    let mut i = 0usize;

    let push_plain = |cur: &mut String, runs: &mut Vec<(String, Tone)>| {
        if !cur.is_empty() {
            runs.push((std::mem::take(cur), Tone::Plain));
        }
    };

    while i < chars.len() {
        // `code`
        if chars[i] == '`' {
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == '`') {
                push_plain(&mut cur, &mut runs);
                let inner: String = chars[i + 1..i + 1 + close].iter().collect();
                runs.push((inner, Tone::Code));
                i += close + 2;
                continue;
            }
        }
        // **bold**
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            let rest = &chars[i + 2..];
            if let Some(close) = find_double_star(rest) {
                if close > 0 {
                    push_plain(&mut cur, &mut runs);
                    let inner: String = rest[..close].iter().collect();
                    runs.push((inner, Tone::Bold));
                    i += close + 4;
                    continue;
                }
            }
        }
        // *italic* / _italic_ — the opener must hug a non-space char AND sit
        // at a word boundary (CommonMark: no intraword `_`; we apply the
        // same rule to `*` so snake_case and a*b stay literal).
        let at_boundary = i == 0
            || chars
                .get(i - 1)
                .is_some_and(|c| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '—' | '-'));
        if (chars[i] == '*' || chars[i] == '_')
            && at_boundary
            && chars.get(i + 1).is_some_and(|c| !c.is_whitespace())
        {
            let marker = chars[i];
            let rest = &chars[i + 1..];
            if let Some(close) = rest.iter().position(|&c| c == marker) {
                let inner: String = rest[..close].iter().collect();
                if !inner.ends_with(char::is_whitespace) && !inner.is_empty() {
                    push_plain(&mut cur, &mut runs);
                    runs.push((inner, Tone::Italic));
                    i += close + 2;
                    continue;
                }
            }
        }
        cur.push(chars[i]);
        i += 1;
    }
    push_plain(&mut cur, &mut runs);
    runs
}

fn find_double_star(chars: &[char]) -> Option<usize> {
    (0..chars.len().saturating_sub(1)).find(|&j| chars[j] == '*' && chars[j + 1] == '*')
}

/// Word-wrap styled runs to `width`, with `indent` columns of hanging indent
/// on continuation lines. Reuses the transcript's wrap discipline (word
/// break, hard-break overlong words) while carrying a tone per character.
fn wrap_runs(
    runs: &[(String, Tone)],
    width: usize,
    indent: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let width = width.max(4);
    // Flatten to (char, tone), then split into words on spaces.
    let flat: Vec<(char, Tone)> = runs
        .iter()
        .flat_map(|(s, tone)| s.chars().map(move |c| (c, *tone)))
        .collect();
    let mut lines: Vec<Vec<(char, Tone)>> = Vec::new();
    let mut cur: Vec<(char, Tone)> = Vec::new();
    let mut cur_w = 0usize;
    let hang: Vec<(char, Tone)> = std::iter::repeat_n((' ', Tone::Plain), indent).collect();

    for word in flat.split(|(c, _)| *c == ' ') {
        let ww: usize = word
            .iter()
            .map(|(c, _)| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
            .sum();
        let base = if lines.is_empty() { 0 } else { indent };
        if cur_w > 0 && base + cur_w + 1 + ww > width {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if ww > width.saturating_sub(base) {
            // Hard-break an overlong word.
            for &(c, tone) in word {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                let base = if lines.is_empty() { 0 } else { indent };
                if base + cur_w + cw > width {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push((c, tone));
                cur_w += cw;
            }
            continue;
        }
        if cur_w > 0 {
            cur.push((' ', Tone::Plain));
            cur_w += 1;
        }
        cur.extend_from_slice(word);
        cur_w += ww;
    }
    lines.push(cur);

    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut chars)| {
            if i > 0 && indent > 0 {
                let mut with_hang = hang.clone();
                with_hang.append(&mut chars);
                chars = with_hang;
            }
            // Group consecutive same-tone chars back into spans.
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut buf = String::new();
            let mut tone = Tone::Plain;
            for (c, t) in chars {
                if t != tone && !buf.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut buf),
                        tone_style(tone, theme),
                    ));
                }
                tone = t;
                buf.push(c);
            }
            if !buf.is_empty() {
                spans.push(Span::styled(buf, tone_style(tone, theme)));
            }
            Line::from(spans)
        })
        .collect()
}

/// Render markdown-ish assistant text to styled lines at `width` columns.
pub fn markdown_lines(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    for raw in text.split('\n') {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            // Fence markers themselves render as a dim rule; language tag kept.
            in_fence = !in_fence;
            let lang = trimmed.trim_start_matches('`').trim();
            let label = if in_fence && !lang.is_empty() {
                format!("┌ {lang}")
            } else if in_fence {
                "┌".to_string()
            } else {
                "└".to_string()
            };
            out.push(Line::from(Span::styled(
                label,
                Style::default().fg(theme.dim),
            )));
            continue;
        }
        if in_fence {
            // Code lines: verbatim (clipped, not wrapped — code loses meaning
            // re-flowed), with a dim gutter.
            let clipped = super::clip(raw, width.saturating_sub(2));
            out.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(theme.dim)),
                Span::styled(clipped, Style::default().fg(theme.text)),
            ]));
            continue;
        }
        if raw.trim().is_empty() {
            out.push(Line::default());
            continue;
        }
        // Headings: strip the hashes, render bold (accent for h1).
        if let Some(rest) = trimmed
            .strip_prefix("###")
            .or_else(|| trimmed.strip_prefix("##"))
        {
            let rest = rest.trim_start_matches('#').trim();
            out.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                rest.trim().to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        // Bullets: glyph + hanging indent.
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let runs = inline_runs(rest);
            let body = wrap_runs(&runs, width.saturating_sub(2), 0, theme);
            for (i, line) in body.into_iter().enumerate() {
                let mut spans = vec![Span::styled(
                    if i == 0 { "• " } else { "  " }.to_string(),
                    Style::default().fg(theme.dim),
                )];
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
            continue;
        }
        // Plain paragraph line.
        out.extend(wrap_runs(&inline_runs(raw), width, 0, theme));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn bold_italic_code_markers_are_consumed() {
        let theme = Theme::dark();
        let lines = markdown_lines("**Bold** then *italic* and `code` end", 80, &theme);
        let text = flat(&lines);
        assert!(!text.contains('*'), "markers consumed: {text}");
        assert!(!text.contains('`'));
        assert!(text.contains("Bold then italic and code end"));
        // Styles actually differ across the runs.
        let styles: Vec<_> = lines[0].iter().map(|s| s.style).collect();
        assert!(
            styles.windows(2).any(|w| w[0] != w[1]),
            "spans styled distinctly"
        );
    }

    #[test]
    fn unmatched_markers_stay_literal() {
        let theme = Theme::dark();
        assert_eq!(flat(&markdown_lines("2 * 3 * 4", 80, &theme)), "2 * 3 * 4");
        assert_eq!(flat(&markdown_lines("a ** b", 80, &theme)), "a ** b");
        assert_eq!(
            flat(&markdown_lines("tick ` alone", 80, &theme)),
            "tick ` alone"
        );
        assert_eq!(
            flat(&markdown_lines("snake_case_name", 80, &theme)),
            "snake_case_name"
        );
    }

    #[test]
    fn bullets_get_glyphs_and_headings_lose_hashes() {
        let theme = Theme::dark();
        let text = "## Plan\n- **first** step\n- second";
        let out = flat(&markdown_lines(text, 80, &theme));
        assert!(out.contains("Plan"));
        assert!(!out.contains('#'));
        assert!(out.contains("• first step"));
        assert!(out.contains("• second"));
    }

    #[test]
    fn fenced_code_is_verbatim_with_gutter() {
        let theme = Theme::dark();
        let text = "```rust\nlet x = a * b; // *not* italic\n```";
        let lines = markdown_lines(text, 80, &theme);
        let out = flat(&lines);
        assert!(out.contains("┌ rust"));
        assert!(out.contains("│ let x = a * b; // *not* italic"));
        assert!(out.contains('└'));
    }

    #[test]
    fn wrapping_respects_width() {
        let theme = Theme::dark();
        let lines = markdown_lines(
            "**A rather long bold sentence** that must wrap cleanly across lines",
            24,
            &theme,
        );
        for l in &lines {
            let w: usize = l
                .iter()
                .map(|s| crate::ui::dwidth(s.content.as_ref()))
                .sum();
            assert!(w <= 24, "line too wide: {w}");
        }
    }
}
