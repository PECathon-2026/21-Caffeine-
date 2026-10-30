//! The agent turn protocol: models reply with exactly one JSON object per turn.

use crate::domain::Verdict;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentTurn {
    Tool(ToolCall),
    Final(FinalReport),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalStatus {
    Completed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalReport {
    pub status: FinalStatus,
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    /// REQUIRED for reviewer/security finals; forbidden for other roles.
    #[serde(default)]
    pub verdict: Option<Verdict>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// Tool names, single source of truth.
pub mod tool {
    pub const READ_FILE: &str = "read_file";
    pub const LIST_FILES: &str = "list_files";
    pub const SEARCH: &str = "search";
    pub const WRITE_FILE: &str = "write_file";
    pub const REPLACE_TEXT: &str = "replace_text";
    pub const RUN_CHECK: &str = "run_check";
    pub const DELEGATE_TASK: &str = "delegate_task";
    pub const SAVE_VERSION: &str = "save_version";
}

/// Extract the outermost JSON object from raw model text: strips ``` fences,
/// then falls back to the first `{` .. last `}` slice.
pub fn extract_json_object(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let unfenced = if trimmed.starts_with("```") {
        let inner = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```");
        inner.trim_end_matches("```").trim()
    } else {
        trimmed
    };
    if unfenced.starts_with('{') && unfenced.ends_with('}') {
        return Some(unfenced);
    }
    let start = unfenced.find('{')?;
    let end = unfenced.rfind('}')?;
    if end > start {
        Some(&unfenced[start..=end])
    } else {
        None
    }
}

/// Parse a raw model reply into an AgentTurn: the FIRST complete, valid turn
/// object wins and anything after it (commentary, a second fence, stray
/// braces) is ignored. Models routinely append prose after the JSON; the
/// protocol absorbs that rather than failing the attempt. A reply with no
/// parseable turn is still an error — never interpreted loosely.
pub fn parse_agent_turn(raw: &str) -> Result<AgentTurn, String> {
    let text = extract_json_object(raw).ok_or("no JSON object in model output")?;
    // Try each '{' as a candidate start (bounded): leading prose may itself
    // contain braces. StreamDeserializer stops at the end of the first
    // complete value, so trailing text never poisons the parse.
    let mut last_err = None;
    for (count, (idx, _)) in text.match_indices('{').enumerate() {
        if count >= 8 {
            break;
        }
        let mut stream = serde_json::Deserializer::from_str(&text[idx..]).into_iter::<AgentTurn>();
        match stream.next() {
            Some(Ok(turn)) => return Ok(turn),
            Some(Err(e)) => last_err = Some(e),
            None => {}
        }
    }
    match last_err {
        Some(e) => Err(format!("invalid agent turn: {e}")),
        None => Err("no JSON object in model output".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_turn() {
        let turn = parse_agent_turn(
            r#"{"kind":"tool","tool":"read_file","arguments":{"path":"src/main.rs"}}"#,
        )
        .unwrap();
        match turn {
            AgentTurn::Tool(t) => assert_eq!(t.tool, "read_file"),
            _ => panic!("expected tool"),
        }
    }

    #[test]
    fn parses_fenced_final() {
        let raw =
            "```json\n{\"kind\":\"final\",\"status\":\"completed\",\"summary\":\"done\"}\n```";
        let turn = parse_agent_turn(raw).unwrap();
        match turn {
            AgentTurn::Final(f) => {
                assert_eq!(f.status, FinalStatus::Completed);
                assert!(f.verdict.is_none());
            }
            _ => panic!("expected final"),
        }
    }

    #[test]
    fn extracts_embedded_object() {
        let raw = "Sure, here you go: {\"kind\":\"final\",\"status\":\"failed\",\"summary\":\"x\"} hope that helps";
        assert!(parse_agent_turn(raw).is_ok());
    }

    #[test]
    fn rejects_prose() {
        assert!(parse_agent_turn("I could not decide what to do.").is_err());
    }
}

#[cfg(test)]
mod trailing_tests {
    use super::*;

    #[test]
    fn trailing_commentary_is_ignored() {
        let raw = "{\"kind\":\"tool\",\"tool\":\"write_file\",\"arguments\":{\"path\":\"a.css\",\"content\":\"x\"}}\n\nThis writes the tokens. Next I will { wire } the layout.";
        match parse_agent_turn(raw).unwrap() {
            AgentTurn::Tool(t) => assert_eq!(t.tool, "write_file"),
            other => panic!("expected tool, got {other:?}"),
        }
    }

    #[test]
    fn fenced_turn_with_trailing_prose_parses() {
        let raw = "```json\n{\"kind\":\"final\",\"status\":\"completed\",\"summary\":\"done\"}\n```\nLet me know if you need anything else — e.g. `.card { color: red }`.";
        assert!(matches!(
            parse_agent_turn(raw).unwrap(),
            AgentTurn::Final(_)
        ));
    }

    #[test]
    fn leading_prose_with_braces_still_finds_the_turn() {
        let raw = "Note {important}: proceeding.\n{\"kind\":\"final\",\"status\":\"completed\",\"summary\":\"ok\"}";
        assert!(matches!(
            parse_agent_turn(raw).unwrap(),
            AgentTurn::Final(_)
        ));
    }

    #[test]
    fn garbage_is_still_an_error() {
        assert!(parse_agent_turn("no json here").is_err());
        assert!(parse_agent_turn("{\"kind\":\"tool\"").is_err());
    }
}
