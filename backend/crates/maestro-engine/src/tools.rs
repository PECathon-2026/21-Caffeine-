//! The tool bus: every repository tool implements [`ToolExec`] and is looked
//! up through [`registry_for_role`] — no hardcoded tool-name matches outside
//! the registry. This is the seam where MCP tools plug in later.

use crate::policy::PolicyEngine;
use crate::sandbox::{self, SandboxSpec};
use crate::scopes;
use futures::future::BoxFuture;
use futures::FutureExt;
use globset::GlobSet;
use maestro_types::config::{CheckConfig, PolicyAction, PolicyEffect};
use maestro_types::domain::Role;
use maestro_types::protocol::tool;
use maestro_types::{MaestroError, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Maximum bytes a single `write_file` may create.
pub const MAX_WRITE_BYTES: usize = 1_048_576;
/// Maximum entries returned by `list_files`.
pub const MAX_LIST_ENTRIES: usize = 500;
/// Maximum characters of search output.
pub const MAX_SEARCH_CHARS: usize = 20_000;
/// Maximum matches reported by `search`.
pub const MAX_SEARCH_MATCHES: usize = 50;

/// Directory names tools never traverse or list.
const SKIP_DIRS: [&str; 4] = [".git", ".maestro", "node_modules", "target"];

/// Injected by the orchestrator: called when policy evaluates to `Ask`.
/// The production decider creates a durable approval and returns
/// [`MaestroError::ApprovalRequired`]; the default test decider denies.
pub type PolicyDecider = Box<dyn Fn(PolicyAction, &str) -> Result<()> + Send + Sync>;

/// A decider that maps every `Ask` to a denial (no approver available).
pub fn deny_ask_decider() -> PolicyDecider {
    Box::new(|action, resource| {
        Err(MaestroError::PolicyDenied {
            action: action.as_str().to_string(),
            resource: resource.to_string(),
            reason: "approval required but no approver is configured".into(),
        })
    })
}

/// Everything a tool execution may see or touch.
pub struct ToolCtx {
    /// Project root (the primary checkout).
    pub root: PathBuf,
    /// The task's workspace; all paths resolve inside it.
    pub workspace: PathBuf,
    /// Immutable task starting revision; chat without a task uses HEAD.
    pub base_ref: Option<String>,
    pub role: Role,
    /// Compiled write scopes for this task.
    pub scopes: GlobSet,
    pub policy: PolicyEngine,
    /// Pre-approved project checks, runnable by index.
    pub checks: Vec<CheckConfig>,
    pub sandbox: SandboxSpec,
    /// Clip limit for `read_file` content.
    pub max_read_chars: usize,
    /// Maps policy `Ask` to a side-effect-free decision (see [`PolicyDecider`]).
    pub decider: PolicyDecider,
}

impl ToolCtx {
    /// Evaluate policy for `action` on `resource`; `Ask` defers to the
    /// decider, which must create no filesystem side effects.
    pub fn decide(&self, action: PolicyAction, resource: &str) -> Result<()> {
        match self.policy.evaluate(action, resource) {
            PolicyEffect::Allow => Ok(()),
            PolicyEffect::Deny => Err(MaestroError::PolicyDenied {
                action: action.as_str().to_string(),
                resource: resource.to_string(),
                reason: "denied by policy".into(),
            }),
            PolicyEffect::Ask => (self.decider)(action, resource),
        }
    }
}

/// What a tool produced.
#[derive(Debug, Clone)]
pub enum ToolOutcome {
    /// JSON payload fed back to the model as the tool result.
    Result(Value),
    /// A validated delegation request the orchestrator materializes.
    Delegate(DelegateRequest),
    /// Validated intent; only the orchestrator can access the history store.
    SaveVersion { label: String },
}

/// A validated `delegate_task` call.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegateRequest {
    pub role: Role,
    pub key: String,
    pub title: String,
    pub description: String,
    pub write_scopes: Vec<String>,
}

fn read_path(ctx: &ToolCtx, rel: &str) -> Result<SafePath> {
    if (rel == "node_modules/next/dist/docs" || rel.starts_with("node_modules/next/dist/docs/"))
        && ctx.workspace != ctx.root
        && !ctx.workspace.join(rel).exists()
    {
        return safe_path(&ctx.root, rel);
    }
    safe_path(&ctx.workspace, rel)
}

/// One executable tool. Implementations must be side-effect-free until every
/// policy, scope, and path check has passed.
pub trait ToolExec: Send + Sync {
    /// The wire name (`read_file`, …).
    fn name(&self) -> &'static str;
    /// Execute against `ctx` with raw JSON `args`.
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>>;
}

/// The tool set a role is allowed to use. Planner has none; designer reads
/// and delegates; coder additionally writes and runs checks; reviewer and
/// security read, run checks, and delegate — never write source files.
/// Working roles can request history checkpoints of their own workspace.
pub fn registry_for_role(role: Role) -> Vec<Arc<dyn ToolExec>> {
    let read: Arc<dyn ToolExec> = Arc::new(ReadFileTool);
    let list: Arc<dyn ToolExec> = Arc::new(ListFilesTool);
    let search: Arc<dyn ToolExec> = Arc::new(SearchTool);
    let write: Arc<dyn ToolExec> = Arc::new(WriteFileTool);
    let replace: Arc<dyn ToolExec> = Arc::new(ReplaceTextTool);
    let check: Arc<dyn ToolExec> = Arc::new(RunCheckTool);
    let delegate: Arc<dyn ToolExec> = Arc::new(DelegateTaskTool);
    let save: Arc<dyn ToolExec> = Arc::new(SaveVersionTool);
    let mut tools = match role {
        Role::Planner => vec![],
        Role::Designer => vec![read, list, search, delegate, save],
        Role::Coder => vec![read, list, search, write, replace, check, delegate, save],
        Role::Reviewer | Role::Security => vec![read, list, search, check, delegate, save],
    };
    if role != Role::Planner {
        tools.extend(
            ["view_image", "browser_inspect", "git_diff"]
                .map(|name| Arc::new(UtilityTool(name)) as Arc<dyn ToolExec>),
        );
        if role != Role::Designer {
            tools.extend(
                [
                    "preview_start",
                    "preview_status",
                    "preview_stop",
                    "run_project_task",
                ]
                .map(|name| Arc::new(UtilityTool(name)) as Arc<dyn ToolExec>),
            );
        }
    }
    tools
}

/// Save code through history, never by creating backup copies in the source tree.
pub struct SaveVersionTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveVersionArgs {
    label: String,
}

impl ToolExec for SaveVersionTool {
    fn name(&self) -> &'static str {
        tool::SAVE_VERSION
    }
    fn execute<'a>(
        &'a self,
        _ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: SaveVersionArgs = parse_args(args)?;
            let label = args.label.trim();
            if label.is_empty() || label.len() > 500 {
                return Err(MaestroError::Config(
                    "version label must be 1–500 bytes of non-blank text".into(),
                ));
            }
            Ok(ToolOutcome::SaveVersion {
                label: label.into(),
            })
        }
        .boxed()
    }
}

/// A workspace-confined path. `abs` is the joined, non-canonicalized
/// location (so not-yet-existing files can be created); `resolved_rel` is
/// the workspace-relative path the request actually lands on after
/// following symlinks. Policy and scope checks must evaluate
/// `resolved_rel`: an in-workspace symlink would otherwise alias `.env`
/// (or any glob-denied or out-of-scope path) under a clean name.
#[derive(Debug)]
pub struct SafePath {
    pub abs: PathBuf,
    pub resolved_rel: String,
}

/// Resolve `rel` inside `workspace`, rejecting absolute paths, `..`
/// components, and symlink escapes (the nearest existing ancestor is
/// canonicalized and must stay under the workspace).
pub fn safe_path(workspace: &Path, rel: &str) -> Result<SafePath> {
    if rel.is_empty() {
        return Err(MaestroError::Protocol("empty path".into()));
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(MaestroError::PolicyDenied {
            action: "read".into(),
            resource: rel.to_string(),
            reason: "absolute paths are not allowed".into(),
        });
    }
    for component in rel_path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(MaestroError::PolicyDenied {
                    action: "read".into(),
                    resource: rel.to_string(),
                    reason: "path traversal ('..' or root components) is not allowed".into(),
                })
            }
        }
    }

    let workspace_canon = workspace
        .canonicalize()
        .map_err(|e| MaestroError::Io(format!("workspace unavailable: {e}")))?;
    let joined = workspace_canon.join(rel_path);

    // Find the nearest existing ancestor and make sure it resolves inside
    // the workspace — this catches symlinks pointing outside.
    let mut probe: &Path = &joined;
    let resolved_ancestor = loop {
        if probe.symlink_metadata().is_ok() {
            break probe
                .canonicalize()
                .map_err(|e| MaestroError::Io(format!("cannot resolve {}: {e}", rel)))?;
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => break workspace_canon.clone(),
        }
    };
    if !resolved_ancestor.starts_with(&workspace_canon) {
        return Err(MaestroError::PolicyDenied {
            action: "read".into(),
            resource: rel.to_string(),
            reason: "path escapes the workspace".into(),
        });
    }

    // Re-attach the components below the existing ancestor, then express the
    // fully resolved target relative to the workspace.
    let resolved_full = match joined.strip_prefix(probe) {
        Ok(remainder) => resolved_ancestor.join(remainder),
        Err(_) => resolved_ancestor,
    };
    let resolved_rel = match resolved_full.strip_prefix(&workspace_canon) {
        Ok(r) if r.as_os_str().is_empty() => ".".to_string(),
        Ok(r) => r.to_string_lossy().into_owned(),
        Err(_) => rel.to_string(),
    };
    Ok(SafePath {
        abs: joined,
        resolved_rel,
    })
}

fn parse_args<T: DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone())
        .map_err(|e| MaestroError::Protocol(format!("invalid tool arguments: {e}")))
}

fn clip_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let clipped: String = text.chars().take(max).collect();
    (format!("{clipped}\n…[truncated]"), true)
}

/// Shared guard for `write_file` / `replace_text`.
fn guard_write(ctx: &ToolCtx, rel: &str) -> Result<PathBuf> {
    // Hard invariant, before anything else: read-only roles never write.
    if ctx.role.is_read_only() {
        return Err(MaestroError::PolicyDenied {
            action: "edit".into(),
            resource: rel.to_string(),
            reason: format!("role '{}' is read-only", ctx.role),
        });
    }
    let safe = safe_path(&ctx.workspace, rel)?;
    ctx.decide(PolicyAction::Edit, rel)?;
    // Also decide on the symlink-resolved target when it differs, so an
    // in-workspace alias cannot dodge secret-file or glob-deny rules.
    if safe.resolved_rel != rel {
        ctx.decide(PolicyAction::Edit, &safe.resolved_rel)?;
    }
    for candidate in [rel, safe.resolved_rel.as_str()] {
        if !scopes::path_in_scopes(&ctx.scopes, candidate) {
            return Err(MaestroError::PolicyDenied {
                action: "edit".into(),
                resource: candidate.to_string(),
                reason: "path is outside the task's declared write scopes".into(),
            });
        }
    }
    Ok(safe.abs)
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

/// `read_file {path}` — policy-checked, clipped file read.
pub struct ReadFileTool;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default, rename = "startLine")]
    start_line: Option<usize>,
    #[serde(default, rename = "maxLines")]
    max_lines: Option<usize>,
}

impl ToolExec for ReadFileTool {
    fn name(&self) -> &'static str {
        tool::READ_FILE
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: ReadFileArgs = parse_args(args)?;
            let safe = read_path(ctx, &args.path)?;
            ctx.decide(PolicyAction::Read, &args.path)?;
            if safe.resolved_rel != args.path {
                ctx.decide(PolicyAction::Read, &safe.resolved_rel)?;
            }
            let bytes = std::fs::read(&safe.abs)
                .map_err(|e| MaestroError::NotFound(format!("{}: {e}", args.path)))?;
            let text = String::from_utf8_lossy(&bytes);
            let start=args.start_line.unwrap_or(1);let limit=args.max_lines.unwrap_or(300).min(1000);
            if start==0 || limit==0 {return Err(MaestroError::Config("startLine and maxLines must be positive".into()));}
            let total=text.lines().count();
            let selected=text.split_inclusive('\n').skip(start-1).take(limit).collect::<String>();
            let (content, clipped) = clip_chars(&selected, ctx.max_read_chars);
            let truncated=clipped || start.saturating_sub(1)+limit<total;
            let next_line=if clipped {start+content.lines().count().saturating_sub(1)} else {start+content.lines().count()};
            Ok(ToolOutcome::Result(json!({
                "path": args.path,
                "content": content,
                "truncated": truncated,
                "startLine":start,"totalLines":total,"nextLine":if truncated {Some(next_line)} else {None},
            })))
        }
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// list_files
// ---------------------------------------------------------------------------

/// `list_files {path?}` — bounded, sorted directory listing.
pub struct ListFilesTool;

#[derive(Deserialize)]
struct ListFilesArgs {
    #[serde(default)]
    path: Option<String>,
}

impl ToolExec for ListFilesTool {
    fn name(&self) -> &'static str {
        tool::LIST_FILES
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: ListFilesArgs = parse_args(args)?;
            let rel = args.path.as_deref().unwrap_or(".");
            let safe = read_path(ctx, rel)?;
            ctx.decide(PolicyAction::Read, rel)?;
            if safe.resolved_rel != rel {
                ctx.decide(PolicyAction::Read, &safe.resolved_rel)?;
            }
            let read = std::fs::read_dir(&safe.abs)
                .map_err(|e| MaestroError::NotFound(format!("{rel}: {e}")))?;
            let mut entries: Vec<String> = Vec::new();
            for entry in read.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir && SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                entries.push(if is_dir { format!("{name}/") } else { name });
            }
            entries.sort();
            let truncated = entries.len() > MAX_LIST_ENTRIES;
            entries.truncate(MAX_LIST_ENTRIES);
            Ok(ToolOutcome::Result(json!({
                "path": rel,
                "entries": entries,
                "truncated": truncated,
            })))
        }
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

/// `search {pattern, path?}` — ripgrep when available, bounded naive
/// substring scan otherwise.
pub struct SearchTool;

#[derive(Deserialize)]
struct SearchArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

impl ToolExec for SearchTool {
    fn name(&self) -> &'static str {
        tool::SEARCH
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: SearchArgs = parse_args(args)?;
            if args.pattern.is_empty() {
                return Err(MaestroError::Protocol("empty search pattern".into()));
            }
            let rel = args.path.as_deref().unwrap_or(".");
            let safe = read_path(ctx, rel)?;
            ctx.decide(PolicyAction::Read, rel)?;
            if safe.resolved_rel != rel {
                ctx.decide(PolicyAction::Read, &safe.resolved_rel)?;
            }

            let output = match rg_search(&ctx.workspace, &args.pattern, rel).await {
                Some(out) => out,
                None => naive_search(&safe.abs, &ctx.workspace, &args.pattern),
            };
            let (results, truncated) = clip_chars(&output, MAX_SEARCH_CHARS);
            Ok(ToolOutcome::Result(json!({
                "pattern": args.pattern,
                "path": rel,
                "results": results,
                "truncated": truncated,
            })))
        }
        .boxed()
    }
}

/// Run ripgrep; `None` when the binary is missing (spawn NotFound).
async fn rg_search(workspace: &Path, pattern: &str, rel: &str) -> Option<String> {
    let result = tokio::process::Command::new("rg")
        .args([
            "--max-count",
            "50",
            "-n",
            "--no-heading",
            "--color",
            "never",
            "--",
        ])
        .arg(pattern)
        .arg(rel)
        .current_dir(workspace)
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    match result {
        Ok(output) => {
            // rg exits 1 on "no matches" — that is still a valid, empty result.
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            if output.status.code() == Some(2) {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Some(format!("search error: {}", stderr.trim()))
            } else {
                Some(stdout)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!("search error: {e}")),
    }
}

/// Substring fallback scan (documented: not regex), bounded to
/// [`MAX_SEARCH_MATCHES`] matches and skipping large/binary-ish files.
fn naive_search(start: &Path, workspace: &Path, pattern: &str) -> String {
    let mut out = String::new();
    let mut matches = 0usize;
    let mut stack = vec![start.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if matches >= MAX_SEARCH_MATCHES {
            break;
        }
        if dir.is_file() {
            naive_search_file(&dir, workspace, pattern, &mut out, &mut matches);
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if child.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(child);
                }
            } else {
                naive_search_file(&child, workspace, pattern, &mut out, &mut matches);
                if matches >= MAX_SEARCH_MATCHES {
                    break;
                }
            }
        }
    }
    out
}

fn naive_search_file(
    file: &Path,
    workspace: &Path,
    pattern: &str,
    out: &mut String,
    matches: &mut usize,
) {
    const MAX_FILE_BYTES: u64 = 524_288;
    if std::fs::metadata(file).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(file) else {
        return;
    };
    if bytes.contains(&0) {
        return; // binary
    }
    let text = String::from_utf8_lossy(&bytes);
    let rel = file.strip_prefix(workspace).unwrap_or(file);
    for (idx, line) in text.lines().enumerate() {
        if *matches >= MAX_SEARCH_MATCHES {
            return;
        }
        if line.contains(pattern) {
            out.push_str(&format!("{}:{}:{}\n", rel.display(), idx + 1, line));
            *matches += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

/// `write_file {path, content}` — scope- and policy-checked write, capped at
/// 1 MB; parent directories are created inside the workspace only.
pub struct WriteFileTool;

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

impl ToolExec for WriteFileTool {
    fn name(&self) -> &'static str {
        tool::WRITE_FILE
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: WriteFileArgs = parse_args(args)?;
            if args.content.len() > MAX_WRITE_BYTES {
                return Err(MaestroError::Protocol(format!(
                    "write_file content exceeds {MAX_WRITE_BYTES} bytes"
                )));
            }
            let abs = guard_write(ctx, &args.path)?;
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, args.content.as_bytes())?;
            Ok(ToolOutcome::Result(json!({
                "path": args.path,
                "bytesWritten": args.content.len(),
            })))
        }
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// replace_text
// ---------------------------------------------------------------------------

/// `replace_text {path, old_text, new_text}` — surgical edit; `old_text`
/// must occur exactly once.
pub struct ReplaceTextTool;

#[derive(Deserialize)]
struct ReplaceTextArgs {
    path: String,
    old_text: String,
    new_text: String,
}

impl ToolExec for ReplaceTextTool {
    fn name(&self) -> &'static str {
        tool::REPLACE_TEXT
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: ReplaceTextArgs = parse_args(args)?;
            if args.old_text.is_empty() {
                return Err(MaestroError::Protocol("old_text must not be empty".into()));
            }
            let abs = guard_write(ctx, &args.path)?;
            let content = std::fs::read_to_string(&abs)
                .map_err(|e| MaestroError::NotFound(format!("{}: {e}", args.path)))?;
            let occurrences = content.matches(&args.old_text).count();
            if occurrences != 1 {
                return Err(MaestroError::Protocol(format!(
                    "old_text must occur exactly once in {}, found {occurrences}",
                    args.path
                )));
            }
            let updated = content.replacen(&args.old_text, &args.new_text, 1);
            if updated.len() > MAX_WRITE_BYTES {
                return Err(MaestroError::Protocol(format!(
                    "resulting file exceeds {MAX_WRITE_BYTES} bytes"
                )));
            }
            std::fs::write(&abs, updated.as_bytes())?;
            Ok(ToolOutcome::Result(json!({
                "path": args.path,
                "replaced": true,
            })))
        }
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// run_check
// ---------------------------------------------------------------------------

/// `run_check {index}` — run one pre-configured project check inside the
/// sandbox. Never arbitrary shell.
pub struct RunCheckTool;

#[derive(Deserialize)]
struct RunCheckArgs {
    index: usize,
}

impl ToolExec for RunCheckTool {
    fn name(&self) -> &'static str {
        tool::RUN_CHECK
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: RunCheckArgs = parse_args(args)?;
            let check = ctx.checks.get(args.index).ok_or_else(|| {
                MaestroError::Protocol(format!(
                    "no check at index {} ({} configured)",
                    args.index,
                    ctx.checks.len()
                ))
            })?;
            ctx.decide(PolicyAction::Check, &check.name)?;
            let mut spec = ctx.sandbox.clone();
            spec.cwd = ctx.workspace.clone();
            let result = sandbox::run_check(&spec, &check.command).await?;
            let mut payload = serde_json::to_value(&result)?;
            if let Some(object) = payload.as_object_mut() {
                object.insert("name".into(), json!(check.name));
                object.insert("index".into(), json!(args.index));
            }
            Ok(ToolOutcome::Result(payload))
        }
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// delegate_task
// ---------------------------------------------------------------------------

/// `delegate_task {role, key, title, description, writeScopes?}` — validate
/// and hand a [`DelegateRequest`] to the orchestrator (which enforces depth
/// and mission caps and materializes the task).
pub struct DelegateTaskTool;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DelegateTaskArgs {
    role: String,
    key: String,
    title: String,
    description: String,
    #[serde(default)]
    write_scopes: Vec<String>,
}

fn is_kebab_key(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

impl ToolExec for DelegateTaskTool {
    fn name(&self) -> &'static str {
        tool::DELEGATE_TASK
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move {
            let args: DelegateTaskArgs = parse_args(args)?;
            let role = Role::parse(&args.role).ok_or_else(|| {
                MaestroError::Protocol(format!("unknown delegate role '{}'", args.role))
            })?;
            if role == Role::Planner {
                return Err(MaestroError::Protocol(
                    "cannot delegate to the planner role".into(),
                ));
            }
            if !is_kebab_key(&args.key) {
                return Err(MaestroError::Protocol(format!(
                    "delegate key '{}' must be kebab-case",
                    args.key
                )));
            }
            if args.title.trim().is_empty() || args.description.trim().is_empty() {
                return Err(MaestroError::Protocol(
                    "delegate title and description must be non-empty".into(),
                ));
            }
            if role == Role::Coder {
                if args.write_scopes.is_empty() {
                    return Err(MaestroError::Protocol(
                        "coder delegations require non-empty writeScopes".into(),
                    ));
                }
                for scope in &args.write_scopes {
                    scopes::validate_scope(scope)
                        .map_err(|e| MaestroError::Protocol(e.to_string()))?;
                }
            }
            ctx.decide(PolicyAction::Delegate, role.as_str())?;
            Ok(ToolOutcome::Delegate(DelegateRequest {
                role,
                key: args.key,
                title: args.title,
                description: args.description,
                write_scopes: args.write_scopes,
            }))
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile_rules;
    use maestro_types::config::{EnforcementMode, PolicyRule};

    fn ctx_for(role: Role, workspace: &Path, extra_rules: &[PolicyRule]) -> ToolCtx {
        let mut rules = profile_rules("balanced");
        rules.extend(extra_rules.iter().cloned());
        let mut sandbox = SandboxSpec::new(workspace);
        sandbox.mode = EnforcementMode::Preferred;
        sandbox.bwrap_path_override = Some(PathBuf::from("/nonexistent/bwrap"));
        ToolCtx {
            root: workspace.to_path_buf(),
            workspace: workspace.to_path_buf(),
            base_ref: None,
            role,
            scopes: scopes::compile_scopes(&["src/**".to_string()]).unwrap(),
            policy: PolicyEngine::new(role, &rules).unwrap(),
            checks: vec![CheckConfig {
                name: "echo".into(),
                command: vec!["echo".into(), "check-ok".into()],
            }],
            sandbox,
            max_read_chars: 10_000,
            decider: deny_ask_decider(),
        }
    }

    fn find<'a>(tools: &'a [Arc<dyn ToolExec>], name: &str) -> &'a Arc<dyn ToolExec> {
        tools
            .iter()
            .find(|t| t.name() == name)
            .expect("tool present")
    }

    #[tokio::test]
    async fn save_version_validates_label_and_cannot_choose_an_external_path() {
        let d = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Reviewer, d.path(), &[]);
        for args in [
            json!({"label":" "}),
            json!({"label":"x".repeat(501)}),
            json!({"label":"backup","path":"/tmp/elsewhere"}),
        ] {
            assert!(SaveVersionTool.execute(&ctx, &args).await.is_err());
        }
        assert!(
            matches!(SaveVersionTool.execute(&ctx,&json!({"label":" before edits "})).await.unwrap(),ToolOutcome::SaveVersion{label} if label=="before edits")
        );
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            0,
            "the executor must materialize the validated intent"
        );
    }

    #[tokio::test]
    async fn ranged_read_can_continue_and_visual_tools_respect_paths() {
        let d = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Coder, d.path(), &[]);
        std::fs::write(d.path().join("lines.txt"), "first\nsecond\nthird\n").unwrap();
        let ToolOutcome::Result(v) = ReadFileTool
            .execute(
                &ctx,
                &json!({"path":"lines.txt","startLine":2,"maxLines":1}),
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(v["content"], "second\n");
        assert_eq!(v["nextLine"], 3);
        assert_eq!(v["totalLines"], 3);
        assert_eq!(v["truncated"], true);
        for name in ["view_image", "git_diff"] {
            for path in [".", "../secret.png", "/tmp/secret.png", ".env"] {
                assert!(
                    UtilityTool(name)
                        .execute(&ctx, &json!({"path":path}))
                        .await
                        .is_err(),
                    "{name}: {path}"
                );
            }
        }
        assert!(ReadFileTool
            .execute(&ctx, &json!({"path":"lines.txt","startLine":0}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn review_diff_keeps_committed_task_patch_and_does_not_stage_files() {
        let d = tempfile::tempdir().unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@local"],
        ] {
            crate::git::git(d.path(), &args).await.unwrap();
        }
        std::fs::write(d.path().join("README.md"), "base\n").unwrap();
        crate::git::git(d.path(), &["add", "."]).await.unwrap();
        crate::git::git(d.path(), &["commit", "-qm", "base"])
            .await
            .unwrap();
        let base = crate::git::head(d.path()).await.unwrap();
        std::fs::write(d.path().join("greeting.txt"), "Hello\n").unwrap();
        crate::git::git(d.path(), &["add", "."]).await.unwrap();
        crate::git::git(d.path(), &["commit", "-qm", "task"])
            .await
            .unwrap();
        std::fs::write(d.path().join("untracked.txt"), "keep untracked").unwrap();
        let before = crate::git::git(d.path(), &["status", "--porcelain=v1"])
            .await
            .unwrap();
        let mut ctx = ctx_for(Role::Reviewer, d.path(), &[]);
        ctx.base_ref = Some(base.clone());
        let ToolOutcome::Result(v) = UtilityTool("git_diff")
            .execute(&ctx, &json!({"path":"greeting.txt"}))
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(v["diff"].as_str().unwrap().contains("+Hello"));
        assert_eq!(v["baseRef"], base);
        assert_eq!(v["changedPaths"], json!(["greeting.txt", "untracked.txt"]));
        assert_eq!(
            crate::git::git(d.path(), &["status", "--porcelain=v1"])
                .await
                .unwrap(),
            before
        );
    }

    #[test]
    fn role_capability_matrix() {
        let names = |role: Role| -> Vec<&'static str> {
            registry_for_role(role).iter().map(|t| t.name()).collect()
        };
        assert!(names(Role::Planner).is_empty());
        assert_eq!(
            names(Role::Designer),
            vec![
                "read_file",
                "list_files",
                "search",
                "delegate_task",
                "save_version",
                "view_image",
                "browser_inspect",
                "git_diff"
            ]
        );
        assert_eq!(
            names(Role::Coder),
            vec![
                "read_file",
                "list_files",
                "search",
                "write_file",
                "replace_text",
                "run_check",
                "delegate_task",
                "save_version",
                "view_image",
                "browser_inspect",
                "git_diff",
                "preview_start",
                "preview_status",
                "preview_stop",
                "run_project_task"
            ]
        );
        for role in [Role::Reviewer, Role::Security] {
            let tools = names(role);
            assert_eq!(
                tools,
                vec![
                    "read_file",
                    "list_files",
                    "search",
                    "run_check",
                    "delegate_task",
                    "save_version",
                    "view_image",
                    "browser_inspect",
                    "git_diff",
                    "preview_start",
                    "preview_status",
                    "preview_stop",
                    "run_project_task"
                ]
            );
            assert!(!tools.contains(&"write_file"));
            assert!(!tools.contains(&"replace_text"));
        }
    }

    #[test]
    fn safe_path_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();

        assert!(safe_path(&ws, "/etc/passwd").is_err());
        assert!(safe_path(&ws, "../outside.txt").is_err());
        assert!(safe_path(&ws, "src/../../outside.txt").is_err());
        assert!(safe_path(&ws, "src/ok.rs").is_ok());
        assert!(safe_path(&ws, "./src/ok.rs").is_ok());

        // Symlink escape: ws/link → sibling dir outside the workspace.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
        match safe_path(&ws, "link/secret.txt") {
            Err(MaestroError::PolicyDenied { reason, .. }) => {
                assert!(reason.contains("escapes"));
            }
            other => panic!("expected escape denial, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_and_write_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);
        let tools = registry_for_role(Role::Coder);

        let write = find(&tools, "write_file");
        let outcome = write
            .execute(&ctx, &json!({"path": "src/a.rs", "content": "fn a() {}\n"}))
            .await
            .unwrap();
        assert!(matches!(outcome, ToolOutcome::Result(_)));

        let read = find(&tools, "read_file");
        match read
            .execute(&ctx, &json!({"path": "src/a.rs"}))
            .await
            .unwrap()
        {
            ToolOutcome::Result(v) => {
                assert_eq!(v["content"], "fn a() {}\n");
                assert_eq!(v["truncated"], false);
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_scope_violation_denied() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);
        let tools = registry_for_role(Role::Coder);
        let write = find(&tools, "write_file");
        match write
            .execute(&ctx, &json!({"path": "docs/out.md", "content": "x"}))
            .await
        {
            Err(MaestroError::PolicyDenied { reason, .. }) => {
                assert!(reason.contains("write scopes"));
            }
            other => panic!("expected scope denial, got {other:?}"),
        }
        assert!(!dir.path().join("docs/out.md").exists(), "no side effects");
    }

    #[tokio::test]
    async fn read_only_roles_cannot_write_even_if_invoked_directly() {
        let dir = tempfile::tempdir().unwrap();
        for role in [Role::Reviewer, Role::Security] {
            let ctx = ctx_for(role, dir.path(), &[]);
            let outcome = WriteFileTool
                .execute(&ctx, &json!({"path": "src/x.rs", "content": "x"}))
                .await;
            match outcome {
                Err(MaestroError::PolicyDenied { reason, .. }) => {
                    assert!(reason.contains("read-only"));
                }
                other => panic!("expected read-only denial, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn replace_text_requires_exactly_one_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/two.rs"), "let x = 1;\nlet x = 1;\n").unwrap();
        std::fs::write(dir.path().join("src/one.rs"), "let y = 2;\n").unwrap();

        let replace = ReplaceTextTool;
        match replace
            .execute(
                &ctx,
                &json!({"path": "src/two.rs", "old_text": "let x = 1;", "new_text": "let x = 9;"}),
            )
            .await
        {
            Err(MaestroError::Protocol(msg)) => assert!(msg.contains("found 2")),
            other => panic!("expected exactly-once error, got {other:?}"),
        }

        replace
            .execute(
                &ctx,
                &json!({"path": "src/one.rs", "old_text": "let y = 2;", "new_text": "let y = 3;"}),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/one.rs")).unwrap(),
            "let y = 3;\n"
        );
    }

    #[tokio::test]
    async fn reading_env_is_hard_denied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=x\n").unwrap();
        std::fs::write(dir.path().join(".env.example"), "OK=1\n").unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);
        let read = ReadFileTool;

        assert!(matches!(
            read.execute(&ctx, &json!({"path": ".env"})).await,
            Err(MaestroError::PolicyDenied { .. })
        ));
        // .env.example stays readable.
        match read
            .execute(&ctx, &json!({"path": ".env.example"}))
            .await
            .unwrap()
        {
            ToolOutcome::Result(v) => assert_eq!(v["content"], "OK=1\n"),
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn in_workspace_symlink_cannot_alias_denied_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=x\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("docs.md"), "out of scope\n").unwrap();
        // Aliases stay inside the workspace, so escape detection alone
        // cannot catch them — policy must see the resolved target.
        std::os::unix::fs::symlink(dir.path().join(".env"), dir.path().join("src/settings"))
            .unwrap();
        std::os::unix::fs::symlink(dir.path().join("docs.md"), dir.path().join("src/alias.md"))
            .unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);

        // Reading .env through a cleanly named symlink is still hard-denied.
        assert!(matches!(
            ReadFileTool
                .execute(&ctx, &json!({"path": "src/settings"}))
                .await,
            Err(MaestroError::PolicyDenied { .. })
        ));
        // Writing through an in-scope symlink to an out-of-scope target is
        // a scope violation.
        match WriteFileTool
            .execute(&ctx, &json!({"path": "src/alias.md", "content": "x"}))
            .await
        {
            Err(MaestroError::PolicyDenied { reason, .. }) => {
                assert!(reason.contains("write scopes"), "reason: {reason}");
            }
            other => panic!("expected scope denial, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs.md")).unwrap(),
            "out of scope\n"
        );
    }

    #[tokio::test]
    async fn ask_effect_creates_no_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let ask_all_edits = vec![PolicyRule {
            action: PolicyAction::Edit,
            resource: "**".into(),
            effect: PolicyEffect::Ask,
            role: None,
        }];
        let ctx = ctx_for(Role::Coder, dir.path(), &ask_all_edits);
        match WriteFileTool
            .execute(&ctx, &json!({"path": "src/asked.rs", "content": "x"}))
            .await
        {
            Err(MaestroError::PolicyDenied { reason, .. }) => {
                assert!(reason.contains("approval required"));
            }
            other => panic!("expected default-deny decider, got {other:?}"),
        }
        assert!(!dir.path().join("src/asked.rs").exists());
    }

    #[tokio::test]
    async fn search_and_list_are_bounded_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Designer, dir.path(), &[]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("src/find_me.rs"), "the needle is here\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();

        match ListFilesTool.execute(&ctx, &json!({})).await.unwrap() {
            ToolOutcome::Result(v) => {
                let entries: Vec<String> = serde_json::from_value(v["entries"].clone()).unwrap();
                assert_eq!(entries, vec!["a.txt", "b.txt", "src/"]);
            }
            other => panic!("expected Result, got {other:?}"),
        }

        match SearchTool
            .execute(&ctx, &json!({"pattern": "needle"}))
            .await
            .unwrap()
        {
            ToolOutcome::Result(v) => {
                let results = v["results"].as_str().unwrap();
                assert!(results.contains("find_me.rs"), "results: {results}");
                assert!(results.contains("needle"));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_check_by_index_only() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);
        match RunCheckTool
            .execute(&ctx, &json!({"index": 0}))
            .await
            .unwrap()
        {
            ToolOutcome::Result(v) => {
                assert_eq!(v["exitCode"], 0);
                assert!(v["stdout"].as_str().unwrap().contains("check-ok"));
                assert_eq!(v["name"], "echo");
            }
            other => panic!("expected Result, got {other:?}"),
        }
        assert!(matches!(
            RunCheckTool.execute(&ctx, &json!({"index": 7})).await,
            Err(MaestroError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn delegate_validation() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(Role::Coder, dir.path(), &[]);
        let delegate = DelegateTaskTool;

        // Valid coder delegation.
        match delegate
            .execute(
                &ctx,
                &json!({
                    "role": "coder", "key": "fix-bug", "title": "Fix",
                    "description": "Fix the bug", "writeScopes": ["src/**"]
                }),
            )
            .await
            .unwrap()
        {
            ToolOutcome::Delegate(req) => {
                assert_eq!(req.role, Role::Coder);
                assert_eq!(req.key, "fix-bug");
                assert_eq!(req.write_scopes, vec!["src/**"]);
            }
            other => panic!("expected Delegate, got {other:?}"),
        }

        // Coder without scopes rejected.
        assert!(delegate
            .execute(
                &ctx,
                &json!({"role": "coder", "key": "no-scopes", "title": "t", "description": "d"})
            )
            .await
            .is_err());
        // Planner target rejected.
        assert!(delegate
            .execute(
                &ctx,
                &json!({"role": "planner", "key": "plan-it", "title": "t", "description": "d"})
            )
            .await
            .is_err());
        // Bad key rejected.
        assert!(delegate
            .execute(
                &ctx,
                &json!({"role": "reviewer", "key": "Bad_Key", "title": "t", "description": "d"})
            )
            .await
            .is_err());
    }
}

/// Bounded visual, preview and inspection tools; the role registry is authoritative.
pub struct UtilityTool(pub &'static str);
impl ToolExec for UtilityTool {
    fn name(&self) -> &'static str {
        self.0
    }
    fn execute<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        args: &'a Value,
    ) -> BoxFuture<'a, Result<ToolOutcome>> {
        async move{
  let get=|key:&str|args[key].as_str().ok_or_else(||MaestroError::Protocol(format!("Missing {key}")));
  let value=match self.0 {
   "view_image"|"git_diff"=>{
    let path=get("path")?;let safe=read_path(ctx,path)?;ctx.decide(PolicyAction::Read,path)?;ctx.decide(PolicyAction::Read,&safe.resolved_rel)?;
    if !safe.abs.is_file() {return Err(MaestroError::Config("Expected a regular file, not a directory".into()));}
    if self.0=="view_image" {
     use std::io::Read;
     let mut data=Vec::new();std::fs::File::open(&safe.abs)?.take(crate::media::MAX_BYTES as u64+1).read_to_end(&mut data)?;
     serde_json::to_value(crate::media::import(&ctx.root,path,&data)?)?
    } else {
     let tracked=tokio::process::Command::new("git").args(["ls-files","--error-unmatch","--",path]).current_dir(&ctx.workspace).output().await?.status.success();
     let mut cmd=tokio::process::Command::new("git");cmd.current_dir(&ctx.workspace).args(["--no-pager","diff","--no-ext-diff","--no-textconv"]);
     let base=ctx.base_ref.as_deref().unwrap_or("HEAD");
     if tracked {cmd.args([base,"--",path]);}else{cmd.args(["--no-index","--","/dev/null"]).arg(&safe.abs);}
     let out=cmd.output().await?;let text=String::from_utf8_lossy(&out.stdout);let (diff,truncated)=clip_chars(&text,20000);
     if !out.status.success() && out.status.code()!=Some(1){return Err(MaestroError::Git(String::from_utf8_lossy(&out.stderr).into()));}
     let mut changed_paths=Vec::new();
     for argv in [vec!["diff","--name-only","--no-renames","-z",base,"--"],vec!["ls-files","--others","--exclude-standard","-z"]] {
      let names=tokio::process::Command::new("git").args(argv).current_dir(&ctx.workspace).output().await?;
      if !names.status.success(){return Err(MaestroError::Git(String::from_utf8_lossy(&names.stderr).into()));}
      changed_paths.extend(names.stdout.split(|b|*b==0).filter(|p|!p.is_empty()).map(|p|String::from_utf8_lossy(p).into_owned()));
     }
     changed_paths.sort();changed_paths.dedup();
     json!({"path":path,"baseRef":base,"diff":diff,"truncated":truncated,"changedPaths":changed_paths})
    }
   },
   "browser_inspect"=>{ctx.decide(PolicyAction::Check,"browser_inspect")?;crate::browser::inspect(&ctx.root,args).await?},
   "preview_start"=>{ctx.decide(PolicyAction::Check,"preview_start")?;let port=match args.get("port"){None|Some(Value::Null)=>std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port() as u64,Some(v)=>v.as_u64().ok_or_else(||MaestroError::Config("Invalid preview port".into()))?};if port<1024||port>65535{return Err(MaestroError::Config("Invalid preview port".into()));}crate::browser::start(ctx,port as u16).await?},
   "preview_status"=>{ctx.decide(PolicyAction::Check,"preview_status")?;crate::browser::status(&ctx.workspace,false)},
   "preview_stop"=>{ctx.decide(PolicyAction::Check,"preview_stop")?;crate::browser::status(&ctx.workspace,true)},
   "run_project_task"=>{
    let name=get("name")?;if !["lint","build","test","typecheck"].contains(&name){return Err(MaestroError::Config("Supported scripts: lint, build, test, typecheck".into()));}
    ctx.decide(PolicyAction::Check,name)?;
    let pkg:Value=serde_json::from_slice(&std::fs::read(ctx.workspace.join("package.json"))?)?;
    if !pkg["scripts"][name].is_string(){return Err(MaestroError::Config(format!("No {name} script in package.json")));}
    let mut spec=ctx.sandbox.clone();spec.cwd=ctx.workspace.clone();
    let result=sandbox::run_check(&spec,&["npm".into(),"run".into(),name.into()]).await?;
    let mut v=serde_json::to_value(result)?;v["name"]=json!(name);v
   },
   _=>return Err(MaestroError::Protocol("Unknown utility tool".into()))
  };Ok(ToolOutcome::Result(value))
 }.boxed()
    }
}
