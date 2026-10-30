//! Inline bordered cards rendered into the chat flow: approval prompts and
//! plan-approval. Pure functions returning styled lines (testable).

use crate::theme::Theme;
use maestro_types::domain::Approval;
use maestro_types::plan::Plan;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

fn card_border(theme: &Theme) -> Style {
    Style::default().fg(theme.border)
}

fn framed(theme: &Theme, width: u16, rows: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let inner = (width as usize).saturating_sub(2).max(8);
    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(Line::from(Span::styled(
        format!("┌{}┐", "─".repeat(inner)),
        card_border(theme),
    )));
    for row in rows {
        let used: usize = row.iter().map(|s| super::dwidth(&s.content)).sum();
        let pad = inner.saturating_sub(used + 1);
        let mut spans = vec![Span::styled("│ ", card_border(theme))];
        spans.extend(row.spans);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled("│", card_border(theme)));
        out.push(Line::from(spans));
    }
    out.push(Line::from(Span::styled(
        format!("└{}┘", "─".repeat(inner)),
        card_border(theme),
    )));
    out
}

/// `◆ Approval <action>` / collapsed resource / reason / key hints.
pub fn approval_card(approval: &Approval, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    let inner = (width as usize).saturating_sub(4).max(8);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::from(vec![
        Span::styled(
            "◆ Approval  ",
            Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
        ),
        Span::styled(approval.action.clone(), Style::default().fg(theme.text)),
    ]));
    rows.push(Line::from(Span::styled(
        super::clip(&approval.resource, inner),
        Style::default().fg(theme.text),
    )));
    let reason = approval
        .policy_snapshot
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("policy requires a decision");
    rows.push(Line::from(Span::styled(
        super::clip(reason, inner),
        Style::default().fg(theme.dim),
    )));
    rows.push(Line::from(vec![
        Span::styled("[a]", Style::default().fg(theme.accent)),
        Span::styled(" approve · ", Style::default().fg(theme.dim)),
        Span::styled("[x]", Style::default().fg(theme.accent)),
        Span::styled(" reject · ", Style::default().fg(theme.dim)),
        Span::styled("[v]", Style::default().fg(theme.accent)),
        Span::styled(" view diff", Style::default().fg(theme.dim)),
    ]));
    framed(theme, width, rows)
}

/// Plan approval card: summary + one row per task (role glyph, key, title,
/// scopes) + decision hints.
pub fn plan_card(plan: &Plan, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    let inner = (width as usize).saturating_sub(4).max(8);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::from(Span::styled(
        "◆ Plan approval",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    rows.push(Line::from(Span::styled(
        super::clip(&plan.summary, inner),
        Style::default().fg(theme.text),
    )));
    for task in &plan.tasks {
        let scopes = if task.write_scopes.is_empty() {
            String::new()
        } else {
            format!("  [{}]", task.write_scopes.join(", "))
        };
        let text = format!("{}  {}{}", task.key, task.title, scopes);
        rows.push(Line::from(vec![
            Span::styled(
                format!("{} ", super::role_glyph(task.role)),
                Style::default().fg(theme.accent),
            ),
            Span::styled(
                super::clip(&text, inner.saturating_sub(2)),
                Style::default().fg(theme.text),
            ),
        ]));
    }
    rows.push(Line::from(vec![
        Span::styled("[a]", Style::default().fg(theme.accent)),
        Span::styled(" approve · ", Style::default().fg(theme.dim)),
        Span::styled("[d]", Style::default().fg(theme.accent)),
        Span::styled(" reject", Style::default().fg(theme.dim)),
    ]));
    framed(theme, width, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::domain::{ApprovalStatus, RiskLevel, Role};
    use maestro_types::plan::PlanTask;

    fn approval() -> Approval {
        Approval {
            id: "ap1".into(),
            mission_id: "m1".into(),
            task_id: None,
            action: "edit".into(),
            resource: "src/main.rs".into(),
            patch_hash: None,
            policy_snapshot: serde_json::json!({"reason": "outside declared scopes"}),
            status: ApprovalStatus::Pending,
            expires_at: None,
            decided_at: None,
            decided_by: None,
            note: None,
            created_at: 1,
        }
    }

    #[test]
    fn approval_card_has_action_resource_reason_hints() {
        let lines = approval_card(&approval(), &Theme::dark(), 60);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text[1].contains("◆ Approval"));
        assert!(text[1].contains("edit"));
        assert!(text[2].contains("src/main.rs"));
        assert!(text[3].contains("outside declared scopes"));
        assert!(text[4].contains("[a] approve"));
        assert!(text[4].contains("[x] reject"));
        assert!(text[4].contains("[v] view diff"));
        // Framed top and bottom.
        assert!(text.first().unwrap().starts_with('┌'));
        assert!(text.last().unwrap().starts_with('└'));
    }

    #[test]
    fn plan_card_lists_tasks_with_role_glyphs_and_scopes() {
        let plan = Plan {
            summary: "Build the widget".into(),
            tasks: vec![PlanTask {
                key: "implement-widget".into(),
                role: Role::Coder,
                title: "Implement it".into(),
                description: String::new(),
                dependencies: vec![],
                acceptance: vec![],
                risk: RiskLevel::Low,
                write_scopes: vec!["src/**".into()],
            }],
        };
        let lines = plan_card(&plan, &Theme::dark(), 80);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text[1].contains("Plan approval"));
        assert!(text[2].contains("Build the widget"));
        assert!(text[3].contains("侍 "));
        assert!(text[3].contains("implement-widget"));
        assert!(text[3].contains("src/**"));
        assert!(text[4].contains("[a] approve"));
        assert!(text[4].contains("[d] reject"));
    }
}
