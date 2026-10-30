//! Deterministic, network-free mock provider. Powers every E2E test.
//!
//! Behavior is selected by `ModelRequest::model`:
//!
//! * `planner` — returns a canonical 4-task JSON plan (raw text, camelCase
//!   keys, serialized from [`maestro_types::plan::Plan`] so it always parses).
//! * contains `coder-writer` — stateless turn sequence driven by the prompt:
//!   until the compiled transcript echoes a successful `write_file` result
//!   ("bytesWritten"), it asks to write `src/generated.txt`; after that it
//!   returns a completed final. Statelessness keeps retried attempts (which
//!   resend identical first prompts) behaving like fresh attempts.
//! * contains `slow` — waits 250 ms (cancellable: cancel yields
//!   [`MaestroError::Cancelled`] promptly), then behaves like the remaining
//!   role default (e.g. `slow-designer` → designer).
//! * contains `reviewer` or `security` — final with verdict `approved`,
//!   reasons `["mock verdict"]`.
//! * `designer` / anything else — completed final with a short summary.
//! * If the system prompt contains `conversational software architect` the
//!   reply is a plain-text canned architect answer (not JSON), regardless of
//!   model name.
//!
//! Usage accounting: `input = prompt chars / 4`, `output = text chars / 4`.
//! Streaming splits the response into ~3 `Delta` chunks then one `Done`.

use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use maestro_types::domain::{RiskLevel, Role, Verdict};
use maestro_types::plan::{Plan, PlanTask};
use maestro_types::protocol::{AgentTurn, FinalReport, FinalStatus, ToolCall};
use maestro_types::provider::{MessageRole, ModelRequest, ModelResponse, StreamEvent, Usage};
use maestro_types::{MaestroError, Result};
use tokio_util::sync::CancellationToken;

use crate::provider::Provider;

const ARCHITECT_MARKER: &str = "conversational software architect";
const SLOW_DELAY: Duration = Duration::from_millis(250);

/// Deterministic mock provider (no network, instant except `slow` models,
/// and fully stateless so retried attempts behave identically).
#[derive(Default)]
pub struct MockProvider;

impl MockProvider {
    pub fn new() -> Self {
        Self
    }

    fn system_prompt(req: &ModelRequest) -> String {
        req.messages
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Compute the canned response text for a request.
    fn response_text(&self, req: &ModelRequest) -> String {
        if Self::system_prompt(req).contains(ARCHITECT_MARKER) {
            // Scripted tool round-trip: a question naming ARCHITECT-TOOL-PROBE
            // first reads probe.txt; once a tool result is in the transcript,
            // the "answer" echoes it so tests can assert real file content
            // flowed through.
            if let Some(result) = req
                .messages
                .iter()
                .rev()
                .find(|m| m.content.starts_with("tool result:"))
            {
                let head: String = result.content.chars().take(400).collect();
                return format!("architect read: {head}");
            }
            if req
                .messages
                .iter()
                .any(|m| m.content.contains("ARCHITECT-TOOL-PROBE"))
            {
                return r#"{"tool":"read_file","args":{"path":"probe.txt"}}"#.to_string();
            }
            if req
                .messages
                .iter()
                .any(|m| m.content.contains("ARCHITECT-BACKUP-PROBE"))
            {
                return r#"{"tool":"save_version","args":{"label":"Before experiment"}}"#.into();
            }
            return architect_answer();
        }
        let model = req.model.as_str();
        if model.contains("planner") {
            return canonical_plan_json();
        }
        if model == "designer-protocol-recover" {
            let prompt = req
                .messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !prompt.contains("[read_file]") {
                return r#"{"kind":"tool","tool":"read_file","arguments":{"path":"src/lib.rs"}}"#
                    .into();
            }
            if !prompt.contains("Invalid response format:") {
                return "{}".into();
            }
            return r#"{"kind":"final","status":"completed","summary":"Recovered using the existing file evidence","evidence":["src/lib.rs"],"risks":[]}"#.into();
        }
        if model == "designer-scope-block" || model == "designer-genuine-block" {
            let clarified = req.messages.iter().any(|m| {
                m.content
                    .contains("Before stopping, reassess only YOUR assigned task")
            });
            if model == "designer-scope-block" && clarified {
                return r#"{"kind":"final","status":"completed","summary":"Design handoff ready; coder implements it","evidence":[],"risks":[]}"#.into();
            }
            return r#"{"kind":"final","status":"blocked","summary":"Required design input is unavailable","evidence":[],"risks":[]}"#.into();
        }
        if model == "reviewer-receipts" {
            let prompt = req
                .messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if prompt.contains("Runtime patch receipt")
                && prompt.contains("changedPaths")
                && prompt.contains("src/generated.txt")
                && prompt.contains("Runtime check receipt")
                && prompt.contains("exitCode")
                && prompt.contains("Model final report")
            {
                return verdict_final();
            }
            return r#"{"kind":"final","status":"blocked","summary":"Missing runtime evidence handoff","evidence":[],"risks":[],"verdict":"changes_requested","reasons":["missing receipts"]}"#.into();
        }
        if model.contains("coder-writer") {
            if model.contains("backup")
                && !req
                    .messages
                    .iter()
                    .filter(|m| m.role == MessageRole::User)
                    .any(|m| m.content.contains("versionId"))
            {
                return r#"{"kind":"tool","tool":"save_version","arguments":{"label":"Before coder edits"}}"#.into();
            }
            // Stateless sequencing: the orchestrator compiles the transcript
            // into the user prompt, so once the write's successful tool
            // result ("bytesWritten") is echoed back the mock declares
            // success; until then it keeps asking to write. Statelessness
            // matters: retried attempts resend byte-identical first prompts,
            // and a counter would wrongly skip the write on attempt two.
            let prompt_shows_write = req
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::User)
                .any(|m| m.content.contains("write_file") && m.content.contains("bytesWritten"));
            if model.contains("checked")
                && prompt_shows_write
                && !req.messages.iter().any(|m| m.content.contains("exitCode"))
            {
                return r#"{"kind":"tool","tool":"run_check","arguments":{"index":0}}"#.into();
            }
            return if prompt_shows_write {
                coder_final()
            } else {
                write_file_turn()
            };
        }
        if model.contains("coder-delegator") {
            // The task whose prompt names the delegated key IS the delegated
            // task: it behaves like coder-writer. Any other coder task
            // delegates once (until the delegation tool result is echoed
            // back) and then finishes without writing.
            let prompt = |pred: &dyn Fn(&str) -> bool| {
                req.messages
                    .iter()
                    .filter(|m| m.role == MessageRole::User)
                    .any(|m| pred(&m.content))
            };
            // Order matters: the delegation tool result echoes the delegated
            // key, so check the write result and the delegation marker
            // before the key itself.
            if prompt(&|c| c.contains("bytesWritten")) {
                return coder_final();
            }
            if prompt(&|c| c.contains("\"delegated\"")) {
                return completed_final();
            }
            if prompt(&|c| c.contains("delegated-fix")) {
                return write_file_turn();
            }
            return delegate_turn();
        }
        if model.contains("reviewer") || model.contains("security") {
            return verdict_final();
        }
        // designer / coder / default
        completed_final()
    }

    /// The `slow` behavior: a 250 ms cancellable delay.
    async fn maybe_delay(&self, req: &ModelRequest, cancel: &CancellationToken) -> Result<()> {
        if !req.model.contains("slow") {
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MaestroError::Cancelled),
            _ = tokio::time::sleep(SLOW_DELAY) => Ok(()),
        }
    }

    fn usage(req: &ModelRequest, text: &str) -> Usage {
        let prompt_chars: usize = req.messages.iter().map(|m| m.content.chars().count()).sum();
        Usage {
            input_tokens: (prompt_chars / 4) as i64,
            output_tokens: (text.chars().count() / 4) as i64,
        }
    }
}

/// The canonical planner output: design → implement (high risk, so a
/// security gate is required) → review + security audit.
fn canonical_plan_json() -> String {
    let plan = Plan {
        summary: "Design, implement, review and security-audit the feature".to_string(),
        tasks: vec![
            PlanTask {
                key: "design-api".into(),
                role: Role::Designer,
                title: "Design the API".into(),
                description: "Produce the API design for the requested feature.".into(),
                dependencies: vec![],
                acceptance: vec!["design is documented".into()],
                risk: RiskLevel::Low,
                write_scopes: vec![],
            },
            PlanTask {
                key: "implement-feature".into(),
                role: Role::Coder,
                title: "Implement the feature".into(),
                description: "Implement the feature following the approved design.".into(),
                dependencies: vec!["design-api".into()],
                acceptance: vec!["code compiles".into(), "feature works".into()],
                risk: RiskLevel::High,
                write_scopes: vec!["src/**".into()],
            },
            PlanTask {
                key: "review-changes".into(),
                role: Role::Reviewer,
                title: "Review the changes".into(),
                description: "Review the implementation diff for correctness.".into(),
                dependencies: vec!["implement-feature".into()],
                acceptance: vec!["verdict recorded".into()],
                risk: RiskLevel::Low,
                write_scopes: vec![],
            },
            PlanTask {
                key: "security-audit".into(),
                role: Role::Security,
                title: "Security audit".into(),
                description: "Audit the implementation diff for security issues.".into(),
                dependencies: vec!["implement-feature".into()],
                acceptance: vec!["verdict recorded".into()],
                risk: RiskLevel::Low,
                write_scopes: vec![],
            },
        ],
    };
    // Serializing the real Plan type guarantees camelCase wire casing.
    serde_json::to_string(&plan).unwrap_or_else(|_| "{}".to_string())
}

fn write_file_turn() -> String {
    let turn = AgentTurn::Tool(ToolCall {
        tool: "write_file".into(),
        arguments: serde_json::json!({
            "path": "src/generated.txt",
            "content": "hello from maestro\n",
        }),
    });
    serde_json::to_string(&turn).unwrap_or_else(|_| "{}".to_string())
}

fn delegate_turn() -> String {
    let turn = AgentTurn::Tool(ToolCall {
        tool: "delegate_task".into(),
        arguments: serde_json::json!({
            "role": "coder",
            "key": "delegated-fix",
            "title": "Delegated fix",
            "description": "delegated-fix: write the generated file",
            "writeScopes": ["src/**"],
        }),
    });
    serde_json::to_string(&turn).unwrap_or_else(|_| "{}".to_string())
}

fn coder_final() -> String {
    let turn = AgentTurn::Final(FinalReport {
        status: FinalStatus::Completed,
        summary: "Wrote src/generated.txt".into(),
        evidence: vec!["file written".into()],
        risks: vec![],
        verdict: None,
        reasons: vec![],
    });
    serde_json::to_string(&turn).unwrap_or_else(|_| "{}".to_string())
}

fn verdict_final() -> String {
    let turn = AgentTurn::Final(FinalReport {
        status: FinalStatus::Completed,
        summary: "Mock review complete".into(),
        evidence: vec!["mock inspection".into()],
        risks: vec![],
        verdict: Some(Verdict::Approved),
        reasons: vec!["mock verdict".into()],
    });
    serde_json::to_string(&turn).unwrap_or_else(|_| "{}".to_string())
}

fn completed_final() -> String {
    let turn = AgentTurn::Final(FinalReport {
        status: FinalStatus::Completed,
        summary: "Mock task completed successfully".into(),
        evidence: vec!["mock execution".into()],
        risks: vec![],
        verdict: None,
        reasons: vec![],
    });
    serde_json::to_string(&turn).unwrap_or_else(|_| "{}".to_string())
}

fn architect_answer() -> String {
    "This is the Maestro mock architect. A reasonable approach: keep the change \
small, add a test first, and route any file writes through a coder task with \
explicit write scopes."
        .to_string()
}

/// Split text into at most `n` chunks on char boundaries.
fn split_chunks(text: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    let per = chars.len().div_ceil(n.max(1));
    chars
        .chunks(per)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn generate(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<ModelResponse> {
        let started = std::time::Instant::now();
        self.maybe_delay(req, cancel).await?;
        let text = self.response_text(req);
        let usage = MockProvider::usage(req, &text);
        Ok(ModelResponse {
            text,
            usage,
            model: req.model.clone(),
            latency_ms: started.elapsed().as_millis() as i64,
        })
    }

    async fn stream(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let started = std::time::Instant::now();
        self.maybe_delay(req, cancel).await?;
        let text = self.response_text(req);
        let usage = MockProvider::usage(req, &text);
        let mut events: Vec<Result<StreamEvent>> = split_chunks(&text, 3)
            .into_iter()
            .map(|chunk| Ok(StreamEvent::Delta { text: chunk }))
            .collect();
        events.push(Ok(StreamEvent::Done {
            usage,
            model: req.model.clone(),
            latency_ms: started.elapsed().as_millis() as i64,
        }));
        Ok(futures::stream::iter(events).boxed())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        Ok(vec![
            "planner".into(),
            "designer".into(),
            "coder".into(),
            "coder-writer".into(),
            "reviewer".into(),
            "security".into(),
            "slow-designer".into(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::provider::ModelMessage;

    fn req(model: &str, user: &str) -> ModelRequest {
        ModelRequest {
            model: model.into(),
            messages: vec![ModelMessage::user(user)],
            max_tokens: None,
            temperature: None,
            json: false,
        }
    }

    #[tokio::test]
    async fn planner_returns_parseable_plan() {
        let p = MockProvider::new();
        let resp = p
            .generate(&req("planner", "build it"), &CancellationToken::new())
            .await
            .unwrap();
        let plan: Plan = serde_json::from_str(&resp.text).unwrap();
        assert_eq!(plan.tasks.len(), 4);
        assert_eq!(plan.tasks[0].key, "design-api");
        let implement = &plan.tasks[1];
        assert_eq!(implement.key, "implement-feature");
        assert_eq!(implement.role, Role::Coder);
        assert_eq!(implement.dependencies, vec!["design-api"]);
        assert_eq!(implement.write_scopes, vec!["src/**"]);
        assert_eq!(implement.risk, RiskLevel::High);
        assert_eq!(plan.tasks[2].dependencies, vec!["implement-feature"]);
        assert_eq!(plan.tasks[3].role, Role::Security);
        assert_eq!(plan.tasks[3].risk, RiskLevel::Low);
        // camelCase on the wire
        assert!(resp.text.contains("writeScopes"));
    }

    #[tokio::test]
    async fn coder_writer_sequence_and_isolation() {
        let p = MockProvider::new();
        let cancel = CancellationToken::new();
        // conversation A: tool then final
        let a1 = p
            .generate(&req("mock-coder-writer", "task A"), &cancel)
            .await
            .unwrap();
        match maestro_types::protocol::parse_agent_turn(&a1.text).unwrap() {
            AgentTurn::Tool(t) => {
                assert_eq!(t.tool, "write_file");
                assert_eq!(t.arguments["path"], "src/generated.txt");
                assert_eq!(t.arguments["content"], "hello from maestro\n");
            }
            other => panic!("expected tool turn, got {other:?}"),
        }
        // a concurrent conversation B still gets a first-turn tool call
        let b1 = p
            .generate(&req("mock-coder-writer", "task B"), &cancel)
            .await
            .unwrap();
        assert!(matches!(
            maestro_types::protocol::parse_agent_turn(&b1.text).unwrap(),
            AgentTurn::Tool(_)
        ));
        // A retried attempt resends the same prompt and must write again —
        // sequencing is driven purely by the transcript echoed in the prompt.
        let a1_retry = p
            .generate(&req("mock-coder-writer", "task A"), &cancel)
            .await
            .unwrap();
        assert!(matches!(
            maestro_types::protocol::parse_agent_turn(&a1_retry.text).unwrap(),
            AgentTurn::Tool(_)
        ));
        // Once the prompt shows the successful write result, the mock finals.
        let a2 = p
            .generate(
                &req(
                    "mock-coder-writer",
                    "task A\n# Transcript\n[write_file] {\"bytesWritten\":19,\"path\":\"src/generated.txt\"}",
                ),
                &cancel,
            )
            .await
            .unwrap();
        match maestro_types::protocol::parse_agent_turn(&a2.text).unwrap() {
            AgentTurn::Final(f) => {
                assert_eq!(f.status, FinalStatus::Completed);
                assert_eq!(f.summary, "Wrote src/generated.txt");
                assert_eq!(f.evidence, vec!["file written"]);
            }
            other => panic!("expected final, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reviewer_and_security_return_approved_verdict() {
        let p = MockProvider::new();
        for model in ["reviewer", "mock-security-model"] {
            let resp = p
                .generate(&req(model, "review"), &CancellationToken::new())
                .await
                .unwrap();
            match maestro_types::protocol::parse_agent_turn(&resp.text).unwrap() {
                AgentTurn::Final(f) => {
                    assert_eq!(f.verdict, Some(Verdict::Approved));
                    assert_eq!(f.reasons, vec!["mock verdict"]);
                }
                other => panic!("expected final, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn slow_model_cancel_returns_cancelled_promptly() {
        let p = MockProvider::new();
        let cancel = CancellationToken::new();
        let started = std::time::Instant::now();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            c2.cancel();
        });
        let err = p
            .generate(&req("slow-designer", "x"), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, MaestroError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "cancel was not prompt"
        );
    }

    #[tokio::test]
    async fn slow_model_completes_after_delay() {
        let p = MockProvider::new();
        let resp = p
            .generate(&req("slow-designer", "x"), &CancellationToken::new())
            .await
            .unwrap();
        assert!(resp.latency_ms >= 250);
        assert!(matches!(
            maestro_types::protocol::parse_agent_turn(&resp.text).unwrap(),
            AgentTurn::Final(_)
        ));
    }

    #[tokio::test]
    async fn architect_system_prompt_returns_plain_text() {
        let p = MockProvider::new();
        let mut r = req("designer", "how should I structure this?");
        r.messages.insert(
            0,
            ModelMessage::system("You are a conversational software architect for this repo."),
        );
        let resp = p.generate(&r, &CancellationToken::new()).await.unwrap();
        assert!(!resp.text.trim_start().starts_with('{'));
        assert!(resp.text.contains("architect"));
    }

    #[tokio::test]
    async fn stream_yields_deltas_then_exactly_one_done() {
        let p = MockProvider::new();
        let r = req("designer", "go");
        let mut s = p.stream(&r, &CancellationToken::new()).await.unwrap();
        let mut text = String::new();
        let mut deltas = 0;
        let mut dones = 0;
        while let Some(ev) = s.next().await {
            match ev.unwrap() {
                StreamEvent::Delta { text: t } => {
                    deltas += 1;
                    text.push_str(&t);
                }
                StreamEvent::Done { usage, .. } => {
                    dones += 1;
                    assert_eq!(usage.output_tokens, (text.chars().count() / 4) as i64);
                }
            }
        }
        assert_eq!(deltas, 3);
        assert_eq!(dones, 1);
        assert!(maestro_types::protocol::parse_agent_turn(&text).is_ok());
    }

    #[tokio::test]
    async fn usage_is_chars_over_four() {
        let p = MockProvider::new();
        let r = req("designer", "abcdefgh"); // 8 chars → 2 input tokens
        let resp = p.generate(&r, &CancellationToken::new()).await.unwrap();
        assert_eq!(resp.usage.input_tokens, 2);
        assert_eq!(
            resp.usage.output_tokens,
            (resp.text.chars().count() / 4) as i64
        );
    }
}
