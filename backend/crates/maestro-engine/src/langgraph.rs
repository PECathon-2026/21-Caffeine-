//! LangGraph worker bridge. IPC conveys requests, never grants filesystem
//! authority. Rust owns quota admission, receipts, approvals and final gates.
use crate::{
    prompts,
    tools::{ToolCtx, ToolExec},
    worktrees::TaskWorkspace,
    Orchestrator,
};
use maestro_types::{
    config::{ModelRef, ProviderKind},
    domain::{InvocationStatus, Task},
    event::kind,
    now_ms,
    protocol::{FinalReport, FinalStatus},
    provider::{ModelRequest, Usage},
    MaestroError, Result,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

fn tool_schema(name: &str) -> Result<Value> {
    let string = || json!({"type":"string"});
    let (description, properties) = match name {
        "read_file" => ("Read a file by 1-based startLine and maxLines. Use nextLine when truncated; null selects defaults.", json!({"path":string(),"startLine":{"type":["integer","null"],"minimum":1},"maxLines":{"type":["integer","null"],"minimum":1,"maximum":1000}})),
        "view_image" => ("View a repository PNG/JPEG/WebP image. Pixels are delivered to the model with the result.",json!({"path":string()})),
        "browser_inspect" => ("Inspect a localhost page: screenshot pixels, visible text, HTTP status, console/page errors. Same-origin read requests only; external assets and mutations are blocked. width/height may be null.",json!({"url":string(),"width":{"type":["integer","null"]},"height":{"type":["integer","null"]}})),
        "git_diff" => ("Read the Git diff for one policy-checked repository file against the task starting revision (HEAD in chat), including untracked files; also reports all changed paths.",json!({"path":string()})),
        "preview_start" => ("Start the current workspace Next.js/Vite dev script on a loopback port (null: automatically select a free port). Retains workspace while running. Use browser_inspect to verify it.",json!({"port":{"type":["integer","null"],"minimum":1024,"maximum":65535}})),
        "preview_status" => ("Get current workspace preview process status, URL and recent logs",json!({})),
        "preview_stop" => ("Stop the current workspace managed preview and its child processes",json!({})),
        "run_project_task" => ("Run an existing package.json lint/build/test/typecheck script through the check sandbox. Does not install packages.",json!({"name":{"type":"string","enum":["lint","build","test","typecheck"]}})),
        "save_version" => ("Save a named, restorable code checkpoint of your current task workspace. Use when the user asks for a backup or saved version, in the requested order (before edits if requested). Does not modify source files or back up databases/services. Report success only after receiving versionId.", json!({"label":{"type":"string","minLength":1,"maxLength":500}})),
        "list_files" => ("List a repository directory; use . for root", json!({"path":string()})),
        "search" => ("Search text or regex in a repository path", json!({"path":string(),"pattern":string()})),
        "write_file" => ("Create or replace a file within declared write scopes", json!({"path":string(),"content":string()})),
        "replace_text" => ("Replace exact text occurring once within a scoped file", json!({"path":string(),"old_text":string(),"new_text":string()})),
        "run_check" => ("Run a preconfigured project check by index; never arbitrary shell", json!({"index":{"type":"integer","minimum":0}})),
        "delegate_task" => ("Request a bounded child task; Rust validates role and scopes", json!({"role":{"type":"string","enum":["designer","coder","reviewer","security"]},"key":string(),"title":string(),"description":string(),"writeScopes":{"type":"array","items":string()}})),
        _ => return Err(MaestroError::Config(format!("No native schema for tool {name}"))),
    };
    let required: Vec<_> = properties.as_object().unwrap().keys().cloned().collect();
    Ok(
        json!({"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}),
    )
}

fn checkpoint_fingerprint(
    task: &Task,
    retry_seq: i64,
    base_ref: &str,
    models: &[Value],
    dependencies: &[Value],
) -> Value {
    json!([
        task.id,
        retry_seq,
        base_ref,
        task.description,
        task.acceptance,
        models,
        dependencies
    ])
}

impl Orchestrator {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_langgraph(
        &self,
        task: &Task,
        owner: &str,
        workspace: &TaskWorkspace,
        gate_targets: &[Task],
        tools: &[Arc<dyn ToolExec>],
        ctx: &ToolCtx,
        prompt: String,
        cancel: &CancellationToken,
    ) -> Result<FinalStatus> {
        let runtime = &self.inner.config.runtime;
        let command = &runtime.langgraph_command;
        if command.is_empty() {
            return Err(MaestroError::Config(
                "LangGraph worker command is missing".into(),
            ));
        }
        let assignment = self.assignment(task.role)?;
        let candidates = self.candidates(task.role, task.model_override.as_deref())?;
        let models = candidates.iter().map(|model| {
            let provider = self.inner.config.providers.get(&model.provider).ok_or_else(|| MaestroError::Config("unknown LangGraph provider".into()))?;
            if provider.kind != ProviderKind::OpenaiResponses { return Err(MaestroError::Config("LangGraph currently requires an OpenAI Responses provider".into())); }
            Ok(json!({"model":model.model,"baseUrl":provider.base_url,"apiKeyEnv":provider.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY"),"reasoningEffort":provider.reasoning_effort,"headers":provider.headers}))
        }).collect::<Result<Vec<_>>>()?;
        let schemas = tools
            .iter()
            .map(|tool| tool_schema(tool.name()))
            .collect::<Result<Vec<_>>>()?;
        let mut cursor = 0;
        let mut retry_seq = 0;
        let mut receipts = BTreeMap::<String, Value>::new();
        loop {
            let events = self
                .inner
                .store
                .events_page(cursor, Some(&task.mission_id), 1000)?;
            if events.is_empty() {
                break;
            }
            for ev in events {
                cursor = ev.seq;
                if ev.task_id.as_deref() != Some(&task.id) {
                    continue;
                }
                if ev.kind == kind::TASK_RETRY {
                    retry_seq = ev.seq;
                }
                if ev.kind == "langgraph.tool" {
                    if let Some(key) = ev.payload["key"].as_str() {
                        receipts.insert(key.into(), ev.payload);
                    }
                }
            }
        }
        // Explicit retries start a new thread; approval/restart recovery retains
        // the same thread. A moved workspace or changed task cannot reuse it.
        // A cascade retry can reopen this task without its own TASK_RETRY
        // event. Bind the checkpoint to completed dependency revisions too,
        // otherwise an old final verdict is replayed after new verification.
        let dependencies = self.inner.store.list_tasks(&task.mission_id)?;
        let dependency_revisions: Vec<_> = dependencies
            .iter()
            .filter(|dependency| task.dependencies.contains(&dependency.task_key))
            .map(|dependency| json!([dependency.id, dependency.updated_at, dependency.patch_hash]))
            .collect();
        let fingerprint = checkpoint_fingerprint(
            task,
            retry_seq,
            &workspace.base_ref,
            &models,
            &dependency_revisions,
        );
        let thread = hex::encode(Sha256::digest(fingerprint.to_string().as_bytes()));
        let state_dir = self.inner.root.join(".maestro/agent-state");
        std::fs::create_dir_all(&state_dir)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700))?;
        let mut system = format!("{}\n\nUse native tool calls to inspect and complete YOUR assigned task. The mission objective includes work assigned to other roles. Tools execute only through Maestro. Never simulate a tool result. Finish using submit_report with factual evidence; only reviewers/security issue verdicts. Continue available inspection before declaring missing knowledge a blocker. Never fabricate checks or bypass restrictions. {}", prompts::role_mandate(task.role), prompts::INJECTION_GUARD);
        if tools.is_empty() {
            system.push_str("\nOnly submit_report is available.");
        }
        let prompt = format!(
            "{prompt}\n# Configured checks (run_check indexes)\n{}",
            serde_json::to_string(&ctx.checks)?
        );
        let init = json!({"role":task.role.as_str(),"system":system,"prompt":prompt,"tools":schemas,"models":models,
            "maxTurns":runtime.max_turns_per_attempt.max(1),"maxContextChars":assignment.max_context_chars,
            "maxOutputTokens":assignment.max_output_tokens.unwrap_or(8192),"requestTimeoutMs":runtime.request_timeout_ms,
            "providerRetries":runtime.provider_retries,"loopDetectionWindow":runtime.loop_detection_window.max(2),
            "checkpointPath":state_dir.join(format!("{}.sqlite",task.id)).to_string_lossy(),"threadId":thread});
        let mut child = tokio::process::Command::new(&command[0])
            .args(&command[1..])
            .current_dir(&workspace.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("LANGCHAIN_TRACING_V2", "false")
            .env("LANGSMITH_TRACING", "false")
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                MaestroError::Config(format!(
                    "Cannot start LangGraph worker: {e}; run scripts/install-langgraph.py"
                ))
            })?;
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        input.write_all(format!("{init}\n").as_bytes()).await?;
        input.flush().await?;
        let mut active: Option<(ModelRef, i64)> = None;
        let store = &self.inner.store;
        let outcome: Result<FinalReport> = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MaestroError::Cancelled),
            result = async {
                loop {
                    let mut bytes = Vec::new();
                    let n = tokio::time::timeout(Duration::from_millis(runtime.request_timeout_ms.max(1000) as u64 + 30_000),
                        (&mut output).take(16_777_217).read_until(b'\n', &mut bytes)).await
                        .map_err(|_| MaestroError::Other("LangGraph worker timed out".into()))??;
                    if n == 0 || n > 16_777_216 { return Err(MaestroError::Protocol("LangGraph worker disconnected or exceeded IPC frame limit".into())); }
                    let request: Value = serde_json::from_slice(&bytes)?;
                    let mut final_report = None;
                    let response = match request["op"].as_str().unwrap_or("") {
                        "image" => json!({"url":crate::media::data_url(&self.inner.root, request["imageId"].as_str().unwrap_or(""))?}),
                        "context" => json!({"directives":self.directive_texts(&task.mission_id, Some(&task.id))?}),
                        "before_model" => {
                            if active.is_some() { return Err(MaestroError::Protocol("overlapping model calls are forbidden".into())); }
                            self.check_budget(&task.mission_id)?;
                            let index = request["candidate"].as_u64().ok_or_else(|| MaestroError::Protocol("missing model candidate".into()))? as usize;
                            let model = candidates.get(index).ok_or_else(|| MaestroError::Protocol("unconfigured model candidate".into()))?;
                            let req = ModelRequest { model:model.model.clone(), messages:vec![crate::media::message(&self.inner.root, request["context"].as_str().unwrap_or(""))?], max_tokens:assignment.max_output_tokens.or(Some(8192)), temperature:None, json:false };
                            match self.inner.registry.acquire_external(model, &req, cancel).await {
                                Ok(()) => {
                                    active = Some((model.clone(), now_ms()));
                                    json!({"allowed":true})
                                }
                                Err(MaestroError::Cancelled) => return Err(MaestroError::Cancelled),
                                Err(error) => json!({"allowed":false,"error":error.to_string()}),
                            }
                        }
                        "after_model" => {
                            let (model, started) = active.take().ok_or_else(|| MaestroError::Protocol("unadmitted model response".into()))?;
                            let success = request["success"].as_bool().unwrap_or(false);
                            let usage: Usage = serde_json::from_value(request["usage"].clone()).unwrap_or_default();
                            let cost = maestro_providers::pricing::cost_micro_usd(&model.model, &usage).0;
                            self.inner.registry.finish_external(&model, Some(success))?;
                            self.record_invocation(&task.mission_id, Some(task), task.role, &model,
                                if success { InvocationStatus::Succeeded } else { InvocationStatus::Failed },
                                usage.input_tokens, usage.output_tokens, cost, now_ms()-started, request["error"].as_str())?;
                            store.append_event(&task.mission_id, Some(&task.id), kind::AGENT_TURN, json!({"runtime":"langgraph","model":model.to_string(),"toolCalls":request["toolCalls"]}))?;
                            json!({"recorded":true})
                        }
                        "tool" => {
                            if active.is_some() { return Err(MaestroError::Protocol("tool call before model receipt".into())); }
                            let name = request["name"].as_str().ok_or_else(|| MaestroError::Protocol("missing tool name".into()))?;
                            let call_id = request["callId"].as_str().ok_or_else(|| MaestroError::Protocol("missing native tool call ID".into()))?;
                            let args = request["arguments"].clone();
                            let key = format!("{thread}:{call_id}");
                            if let Some(previous) = receipts.get(&key) {
                                if previous["name"] != name || previous["arguments"] != args { return Err(MaestroError::Protocol("native tool call ID reused with different arguments".into())); }
                                if previous["state"] == "done" {
                                    input.write_all(format!("{}\n",json!({"result":previous["result"]})).as_bytes()).await?;
                                    input.flush().await?;
                                    continue;
                                }
                                if previous["state"] == "pending" && !matches!(name, "read_file"|"list_files"|"search"|"view_image"|"git_diff"|"browser_inspect"|"preview_status") {
                                    return Err(MaestroError::Conflict(format!("Interrupted {name} has an uncertain outcome; inspect the task workspace before retrying")));
                                }
                            }
                            let pending = json!({"key":key,"state":"pending","name":name,"arguments":args});
                            store.append_event(&task.mission_id, Some(&task.id), "langgraph.tool", pending.clone())?;
                            receipts.insert(key.clone(),pending);
                            store.append_event(&task.mission_id, Some(&task.id), kind::TOOL_CALL,json!({"tool":name,"callId":call_id,"arguments":crate::orchestrator::clip(&args.to_string(),1000)}))?;
                            let result = match self.execute_tool(tools,ctx,task,name,&args).await {
                                Ok(result) => result,
                                Err(e) => {
                                    if matches!(e, MaestroError::ApprovalRequired{..}) {
                                        store.append_event(&task.mission_id,Some(&task.id),"langgraph.tool",json!({"key":key,"state":"parked","name":name,"arguments":args}))?;
                                    }
                                    return Err(e);
                                }
                            };
                            if matches!(name, "run_check" | "run_project_task") {
                                let command = if name=="run_project_task" {Some(vec!["npm".into(),"run".into(),args["name"].as_str().unwrap_or("").into()])}else{args["index"].as_u64().and_then(|i|ctx.checks.get(i as usize)).map(|c| c.command.clone())};
                                store.add_artifact(&task.mission_id,Some(&task.id),"check_result",json!({"command":command,"workspace":ctx.workspace.to_string_lossy(),"result":result}))?;
                            }
                            let receipt = json!({"key":key,"state":"done","name":name,"arguments":args,"result":result});
                            store.append_event(&task.mission_id,Some(&task.id),"langgraph.tool",receipt.clone())?;
                            receipts.insert(key,receipt);
                            store.append_event(&task.mission_id,Some(&task.id),kind::TOOL_RESULT,json!({"tool":name,"callId":call_id,"output":crate::orchestrator::clip(&result.to_string(),2000)}))?;
                            result
                        }
                        "result" => {
                            if active.is_some() { return Err(MaestroError::Protocol("final report before model accounting".into())); }
                            final_report = Some(serde_json::from_value::<FinalReport>(request["report"].clone())?);
                            json!({"accepted":true})
                        }
                        "fatal" => return Err(MaestroError::Other(request["error"].as_str().unwrap_or("LangGraph worker failed").into())),
                        other => return Err(MaestroError::Protocol(format!("unknown LangGraph operation {other}"))),
                    };
                    input.write_all(format!("{}\n",json!({"result":response})).as_bytes()).await?;
                    input.flush().await?;
                    if let Some(report) = final_report { return Ok(report); }
                }
            } => result,
        };
        if let Some((model, started)) = active.take() {
            self.inner.registry.finish_external(&model, None)?;
            self.record_invocation(
                &task.mission_id,
                Some(task),
                task.role,
                &model,
                InvocationStatus::Cancelled,
                0,
                0,
                0,
                now_ms() - started,
                Some("worker interrupted before usage receipt"),
            )?;
        }
        drop(input);
        let _ = child.kill().await;
        let _ = child.wait().await;
        let report = outcome?;
        self.handle_final(task, workspace, owner, gate_targets, report)
            .await
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn cascade_review_retry_changes_checkpoint_but_lease_recovery_does_not() {
        let mut task: Task = serde_json::from_value(json!({
            "id":"shadow","missionId":"mission","taskKey":"review-security",
            "role":"security","title":"Review","description":"Review the patch and Sentinel evidence",
            "status":"running","dependencies":["implement","verify"],"acceptance":["Independent verdict"],
            "risk":"low","attempts":0,"createdAt":1,"updatedAt":2
        })).unwrap();
        let inputs = vec![
            json!(["coder", 10, "same-patch"]),
            json!(["sentinel", 20, null]),
        ];
        let original = checkpoint_fingerprint(&task, 0, "base", &[], &inputs);
        task.updated_at = 99;
        task.lease_generation = Some("replacement-lease".into());
        assert_eq!(
            original,
            checkpoint_fingerprint(&task, 0, "base", &[], &inputs),
            "approval/restart recovery must retain tool receipts"
        );
        let reverified = vec![
            json!(["coder", 10, "same-patch"]),
            json!(["sentinel", 30, null]),
        ];
        assert_ne!(original,checkpoint_fingerprint(&task,0,"base",&[],&reverified),"new verification evidence must not replay the old final verdict, even when the code patch did not change");
        assert_ne!(
            original,
            checkpoint_fingerprint(&task, 55, "base", &[], &inputs),
            "an explicit retry must start fresh too"
        );
    }
}
