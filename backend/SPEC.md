# Maestro-rs — engineering specification

Maestro is a local multi-model engineering control plane, rebuilt in Rust. It coordinates
specialist LLM roles (Planner, Designer, Coder, Reviewer, Security) through a durable,
SQLite-backed task graph. The deterministic runtime owns every state transition; models
can inspect, propose, use explicitly granted tools, and return evidence — nothing else.

This document is the contract between crates. `maestro-types` is the machine-readable
half of the contract: if prose here and types disagree, the types win and this file must
be fixed.

## Goals (non-negotiable)

1. **Latency.** Control-plane overhead for any single operation (scheduling decision,
   lease acquisition, event append, policy evaluation, TUI action dispatch) must be
   < 10 ms on commodity hardware. Provider network time and model latency are excluded.
   Practical rules: prepared/cached SQL statements, WAL mode, no per-operation
   allocation storms, no polling loops where a notification channel works.
2. **No leaks.** Every spawned task is either awaited, aborted on drop (via
   `JoinHandle`/`AbortHandle` kept in a struct), or bounded. Every channel has a bounded
   capacity or a documented reason to be unbounded. `broadcast` receivers must tolerate
   `Lagged`. No detached `tokio::spawn` without an owner.
3. **Event-driven UI.** The TUI never polls the database on a timer for state that can
   arrive via the event stream. Redraw only on input or event, coalesced to ≤ 60 fps.
4. **Determinism and audit.** Every provider request, tool result, and state transition
   is a durable row. Malformed model output fails the attempt; it is never interpreted
   loosely.
5. **Safety invariants** (ported from the TS version, all still binding):
   - Reviewer and Security never write production files.
   - Child authority never exceeds parent and project policy.
   - A model cannot approve its own patch.
   - Task and delegation depth are bounded.
   - Credentials never appear in prompts, events, or project config.
   - Destructive or external side effects require a policy decision outside the model.
   - Reads of `.env` / `.env.*` (except `.env.example`) are always denied; the sandbox
     masks them with `/dev/null` binds.

## Workspace layout and crate ownership

```
crates/
  maestro-types      pure data: ids, enums, domain structs, config, plan, events,
                     provider DTOs, daemon API DTOs, agent turn protocol. No I/O.
  maestro-store      rusqlite store, migrations, event bus (tokio broadcast),
                     leases, gates, approvals, scope locks, merge queue, chat.
  maestro-providers  Provider trait, Anthropic + OpenAI-compatible (both streaming),
                     test-only deterministic fixtures, registry, model discovery, pricing/cost,
                     token-bucket rate limiter, circuit breaker.
  maestro-engine     plan validation, policy, scopes, sandbox (bubblewrap),
                     git worktrees + merge queue processing, context compiler,
                     prompts, repository tools, orchestrator.
  maestro-daemon     axum HTTP + SSE control server, mission workers, generations,
                     workspace lock, recovery. Also exports `Client` (HTTP client
                     used by CLI/TUI to reach a running daemon).
  maestro-tui        ratatui chat-first console + mission dashboard views.
  maestro-cli        the `maestro` binary (clap).
```

Dependency direction (strict, no cycles):
`types` ← `store` ← (`providers`, `engine`) ← `daemon` ← (`tui`, `cli`).
`providers` depends only on `types`. `tui` may depend on `store` (read-only helpers),
`daemon::Client`, and `engine` for in-process fallback.

## Coding standards

- Edition 2021, stable toolchain. `cargo clippy --workspace --all-targets` must be
  warning-clean (deny `clippy::unwrap_used` in lib code; tests may unwrap).
- Libraries return `Result<T, MaestroError>` (thiserror, defined in `maestro-types`);
  binaries may use `anyhow` at the edge.
- Logging via `tracing`; never `println!` outside the CLI's user-facing output.
- All timestamps are unix **milliseconds** as `i64` (`maestro_types::now_ms()`).
- All ids are 32-char lowercase hex UUIDv4 simple form (`maestro_types::new_id()`).
- JSON casing on the wire and in the DB: `camelCase` (serde rename_all) to stay
  compatible with the TS version's payload conventions.
- SQLite access is synchronous rusqlite behind a `std::sync::Mutex<Connection>`
  wrapped in `Arc` (a single writer is fine at our scale and keeps latency low);
  long-running async code must call store methods via `tokio::task::block_in_place`
  or accept the microsecond-scale lock hold. Do not introduce a connection pool.
- Every crate ships unit tests. Integration tests that need git create throwaway
  repos under `std::env::temp_dir()` and clean up (use `tempfile` if needed —
  add it as a dev-dependency).

## Data model (SQLite, `.maestro/maestro.db`)

WAL, `foreign_keys=ON`, `busy_timeout=5000`, `synchronous=NORMAL`. Migrations are a
versioned list (`PRAGMA user_version`) of idempotent steps — an improvement over the
TS version's additive-only checks.

Tables (columns in `maestro-types` structs; store owns SQL):

- `missions` — id, objective, status, plan_summary, base_head, mission_branch,
  integration_head, final_approval_id, created/updated. Status machine:
  `planning → planned → awaiting_plan_approval? → running → awaiting_approval |
  completed | failed | blocked | paused | cancelled`.
  `awaiting_plan_approval` is NEW: when `runtime.plan_approval = true` the mission
  halts after planning until the plan is approved/edited via API/TUI.
- `tasks` — UNIQUE(mission_id, task_key); status
  `proposed → ready → running → completed | failed | blocked` with retry back to
  `ready` while attempts < max. Lease columns (owner, expires_at, generation),
  worktree columns (workspace_path, branch_name, base_ref), integration columns
  (patch_hash, integrated_at, integration_commit), delegation (parent_task_id,
  delegation_depth), write_scopes_json, model_override (NEW: retry with a
  different model).
- `events` — append-only AUTOINCREMENT id, mission_id, task_id?, kind, payload
  (JSON), created_at. This is the SSE cursor source and the audit log.
- `invocations` — one row per provider call: role, provider_id, model, status,
  input_tokens, output_tokens, cost_micro_usd (NEW), latency_ms (NEW), error.
- `gates` — UNIQUE(task_id, role), verdict, reasons JSON, patch_hash. A gate passes
  iff verdict = approved AND its patch_hash equals the current patch hash.
- `directives` — steering text, optional task scope, created_by, active flag.
- `approvals` — action, resource, task_id?, patch_hash?, policy snapshot JSON,
  status pending/approved/rejected/expired, expires_at, decided_at/by. Deduped by
  (mission, action, resource) while pending.
- `scope_locks` — PK(mission_id, task_id, scope); conservative glob-overlap check
  at acquisition inside one transaction.
- `merge_queue` — mission_id, task_id, position, patch_hash, status
  pending/checking/merged/conflict/failed/superseded. Crash reconciliation resets
  `checking → pending`.
- `daemon_instances` — generation, pid, url, started_at, heartbeat_at, status.
- `chat_sessions` / `chat_messages` — persistent architect conversations, linked to
  missions/tasks/approvals.
- `provider_health` (NEW) — provider_id, window stats: success, failure, consecutive
  failures, open_until (circuit breaker state survives restarts).

Store exposes a typed API (no SQL outside `maestro-store`). The event bus is
`tokio::sync::broadcast::Sender<EventRecord>` fed by every `append_event`; capacity
1024; SSE replays from a cursor (`events.id`) then follows the bus.

## Agent turn protocol (unchanged semantics from TS)

Models must reply with exactly one JSON object per turn:

```json
{"kind":"tool","tool":"read_file","arguments":{"path":"src/app.rs"}}
```

or

```json
{"kind":"final","status":"completed","summary":"...","evidence":["..."],
 "risks":[], "verdict":"approved"}
```

`verdict` (approved/changes_requested/rejected) is REQUIRED for reviewer/security
finals and forbidden otherwise. Parsing: strip code fences, then first `{` to last
`}` fallback. Anything else fails the attempt.

Tools by role (enforced post-parse, pre-exec):
- planner: none (plans only)
- designer: read_file, list_files, search, delegate_task
- coder: read_file, list_files, search, write_file, replace_text, run_check,
  delegate_task
- reviewer/security: read_file, list_files, search, run_check, delegate_task

`run_check` only runs pre-configured project checks by index — never arbitrary shell.
Writes are scope-checked (declared globs) at write time and re-verified against
`git diff --name-only` at completion.

## Orchestrator

- Planner produces a JSON plan validated by `plan.rs`: 1–12 tasks, kebab-case keys,
  dependencies reference earlier tasks only, no planner-role tasks, coder tasks
  must declare non-empty write scopes (relative globs, no `..`).
- NEW: plan approval gate — if enabled, mission waits in `awaiting_plan_approval`;
  the API allows approve, reject, or replacing the plan JSON (revalidated).
- Scheduler loop: recover expired leases → promote ready tasks → select batch
  (max_concurrent, skip shared-worktree pairs, skip overlapping coder scopes) →
  run batch concurrently (JoinSet). Wake-driven: the loop parks on a Notify that
  fires on task completion/steer/approval, plus a coarse timer for lease expiry.
- Lease = single conditional UPDATE (CAS). Heartbeat at ttl/3 while a task runs.
- Retries bounded by max_attempts; abort/pause returns task to ready; cancel blocks it.
- Verdict gates: every coder task requires a passing reviewer gate recorded by a
  dependent reviewer task (patch-hash-bound); security gate required when
  risk = high. Staleness re-checked at merge time.
- Delegation: depth-capped, 50-task mission cap, coder delegations need scopes.
- Token budget checked before each call; NEW cost budget (micro-USD) as well.
- NEW loop detection: sha256 of (tool, canonical args); if the same hash repeats
  `loop_detection_window` times consecutively, fail the attempt with a diagnostic.
- Steering directives re-read every turn and injected into prompts.
- Provider invocation: candidates = [primary, ...fallbacks]; per-candidate bounded
  retries with jittered backoff honoring Retry-After; rate limiter + circuit
  breaker per provider; every attempt is an invocation row with latency and cost.

## Integration (git)

- Mission branch `maestro/mission/<id12>` from base HEAD; each coder task gets a
  worktree `.maestro/worktrees/<taskId>` on branch `maestro/<mission8>/<key>-<id6>`.
- patch_hash = sha256 of `git diff --binary <base>` with untracked files
  intent-added.
- Serial merge queue: enqueue (gates + hash freshness asserted) → `merge --no-ff`
  into integration worktree → run all sandboxed checks → rollback on failure.
  Conflicts spawn delegated resolver coder + reviewer tasks once; a second
  conflict blocks the mission.
- Final integration approval binds the sha256 of the whole base→integration diff;
  finalize recomputes and refuses stale hashes, requires clean checkout, then
  `merge --ff-only` onto the user's branch.

## Policy and sandbox

- Ordered glob rules, last match wins; actions: read, edit, check, delegate,
  network, external_directory. Effects: allow, ask, deny. Mandatory hard denials
  (cannot be overridden): reviewer/security edit, external_directory, secret-file
  reads. `ask` creates a durable approval and parks the task (`blocked`,
  "Waiting for approval") until decided.
- Sandbox: bubblewrap (`bwrap` resolved from PATH, not hardcoded),
  `--die-with-parent --new-session --unshare-pid/ipc/uts`, `--unshare-net` unless
  allowed, ro-bind /usr /bin /lib /lib64, tmpfs /tmp, bind cwd only, `--clearenv`
  + allowlist, `/dev/null` over every `.env*` in the tree. `required` fails
  closed; `preferred` falls back to host exec with a warning event. Timeout and
  output clipping enforced.

## Daemon HTTP API (localhost, bearer token required off-loopback)

```
GET  /health                          → {ok, version, generation, pid}
GET  /api/state                       → full control-plane snapshot (missions, workers, approvals)
GET  /api/missions                    → [MissionSummary]
POST /api/missions                    → {objective, sessionId?} → {missionId}   (submit)
GET  /api/missions/:id                → MissionDetail (tasks, stats, plan)
POST /api/missions/:id/pause|resume|cancel
POST /api/missions/:id/retry         → {taskId, modelOverride?}
POST /api/missions/:id/steer         → {text, taskId?}
GET  /api/missions/:id/plan          → plan JSON
POST /api/missions/:id/plan/approve  → {approve: bool, editedPlan?}
GET  /api/missions/:id/diff          → {unified}
GET  /api/approvals?missionId=       → [Approval]
POST /api/approvals/:id              → {approve: bool, note?}
GET  /api/events?missionId=&after=   → SSE (replay from cursor, then live;
                                        Last-Event-ID honored; 15s heartbeats)
GET  /api/models                     → discovered models per provider
POST /api/chat/:sessionId/messages   → chat turn (architect Q&A or mission launch)
POST /api/shutdown
```

Errors: JSON `{error}` with 400/401/404/409. All DTOs live in
`maestro_types::api`. The daemon is the only long-lived writer; CLI/TUI prefer the
daemon and fall back to in-process engine execution only when no daemon is running
AND the command is explicitly local (e.g. `maestro run --local`).

NEW versus TS version: the TUI auto-spawns a detached daemon when none is running,
so chat-launched missions survive terminal exit.

Workspace exclusivity: lock dir `.maestro/daemon.lock` (atomic mkdir) + pid
liveness probe; state file `.maestro/daemon.json` {url, pid, generation, token?}.
On boot: register generation, mark stale instances dead, reset running tasks with
foreign lease generations to ready, reconcile merge queue, resume running/planning
missions.

## Providers

`#[async_trait] trait Provider { async fn generate(&self, req) -> Result<ModelResponse>;
async fn stream(&self, req) -> Result<BoxStream<StreamEvent>>; async fn list_models(&self) -> ... }`

- Anthropic: `/v1/messages`, `x-api-key`, `anthropic-version: 2023-06-01`, SSE
  streaming (`content_block_delta`), usage from `message_delta`.
- OpenAI Responses: `/responses`, typed output/SSE parsing, `max_output_tokens`,
  JSON text format and `store: false`. Incomplete/failed outputs are errors.
- OpenAI-compatible: `/chat/completions` with `stream: true` SSE deltas; bearer
  optional (local endpoints); `response_format: {type: "json_object"}` when asked.
- Mock: deterministic, no network; model name selects behavior (`planner` → canonical
  plan; `coder-writer` → write_file then final; `slow-designer` → abortable delay;
  chat prompts → canned answer). Powers all E2E tests.
- Registry caches instances; expands [model, ...fallbacks]; per-provider token
  bucket (requests/min) and circuit breaker (open after N consecutive failures,
  half-open probe after cooldown).
- Pricing table (static map model-prefix → $/Mtok in/out) computes
  cost_micro_usd per invocation; unknown models cost 0 and are flagged.
- Cancellation: every request takes a `CancellationToken`/`AbortSignal` equivalent
  (tokio `CancellationToken` from tokio-util or a watch channel — pick one, put it
  in types as a type alias) combined with a 180 s timeout.

## Config and credentials

- `maestro.toml` at project root (TOML now, not JSON). All keys are camelCase
  (serde rename_all); unknown keys are rejected at load (deny_unknown_fields),
  never silently ignored. Sections: providers (id, kind, baseUrl, apiKeyEnv),
  role→model assignment with fallbacks (model, fallbacks, maxContextChars),
  runtime knobs (maxConcurrentTasks=2, maxAttemptsPerTask=2,
  maxTurnsPerAttempt, maxTotalTokens=500k, maxCostMicroUsd, leaseTtlMs,
  planApproval=false, loopDetectionWindow=3), safety (policy profile + rules,
  sandbox mode required/preferred, network deny, worktree mode
  required/preferred), integration (strategy mission-branch,
  finalApproval=true), checks list (name + argv command).
- Production default uses GPT through Responses and requires credentials. The
  mock transport exists only behind the explicit test-support feature.
- Credentials in `.maestro/credentials.json` `{version, providers:{id:secret}}`,
  dir 0700 file 0600, atomic write; injected into env only if unset (real env wins).
- `maestro init` writes config and appends `.maestro/` to `.gitignore`.

## TUI (ratatui) — chat-first console

Design language: flat minimalism. No gradients, no glow, no rainbow spinners.
Restrained accent color, dim borders, whitespace over chrome. Dark and light safe.

Layout (adapted from opencode/keelcode patterns):
- Chat view (default): scrollback transcript (user/architect/agent/system lines,
  streamed tokens render live), right sidebar with team state (role, model, task,
  spinner), status bar (mission, branch, tokens, cost, elapsed, daemon state).
- Tabs: [1] Chat  [2] Tasks (DAG list with status glyphs, j/k select, Enter detail)
  [3] Diff (unified, syntax-dim)  [4] Approvals (a approve / r reject)  [5] Events.
- Command palette on `:` or `/` — slash commands: /ask /run /plan /approve /reject
  /steer /status /models /model /connect /settings /sessions /new /quit.
- Plain text routed: question → architect answer; implementation request → mission
  (with plan approval flow if enabled); text during a running mission → steering.
- Modal prompts for approvals with diff preview.
- All state arrives over the daemon SSE stream; input handled via crossterm
  EventStream (async); redraw coalesced with a 16ms debounce; zero timer polling
  except a 1s clock tick for elapsed timers.
- Exiting must always restore the terminal (raw mode guard on Drop, panic hook).

## CLI commands

`maestro` (no args → TUI chat if config exists, else init wizard),
`init`, `serve`, `status [--json]`, `submit <objective>`, `run <objective> --local`,
`missions`, `mission <id>`, `tasks <missionId>`, `events <missionId> [--follow]`,
`approvals`, `approve <id>`, `reject <id>`, `steer <missionId> <text>`,
`pause|resume|cancel <missionId>`, `retry <missionId> <taskId> [--model <ref>]`,
`plan <missionId> [--approve|--reject]`, `diff <missionId>`, `models`,
`connect <providerId>`, `doctor` (environment checks: git, bwrap, db integrity),
`--version`, `--json` global flag for machine-readable output.

Every command that mutates state goes through the daemon when one is running.

## Extended scope (build these too)

- **Artifacts** (`artifacts` table): typed handoff objects — kind
  (design/patch/test_report/review_findings/security_findings/worktree/note),
  task_id, mission_id, content (JSON), content_hash, created_at. Coder finals
  auto-attach a worktree artifact; reviewer/security finals attach findings.
  Dependency handoffs in prompts are built from artifacts, not just summaries.
- **Metrics** (`maestro-store::metrics`): in-process op-latency recorder —
  `time_op("lease.acquire", || ...)` records microseconds into fixed
  log-scale buckets per op; `snapshot() -> Vec<OpMetrics>` computes p50/p99/max.
  Store wraps every public method with it. Exposed at GET /api/metrics and via
  `maestro bench` (runs a synthetic workload: 1000 event appends, 1000 lease
  CAS cycles, 200 policy evals, reports p50/p99 vs the 10ms budget and exits
  non-zero on breach). Zero overhead goal: recording = one atomic add.
- **Provider scorecards**: `GET /api/scorecards` + `maestro scorecards` —
  per (role, provider, model) aggregates derived from invocations: calls,
  success rate, p50 latency, tokens, cost. No new table; SQL aggregate.
- **Tool bus seam**: engine tools implement a `ToolExec` trait (name, json
  schema-ish arg validation, execute). Registry keyed by name per role. This is
  the seam where MCP tools plug in later — do NOT implement MCP now, but keep
  the registry open (no hardcoded match on tool names outside the registry).
- **Themes & keybindings**: TUI loads optional `~/.config/maestro/theme.json`
  and `keybindings.json` overlays over built-in dark/light themes and default
  keymap. Semantic tokens only.
- **Shell completions**: `maestro completions <shell>` via clap_complete.
- **Doctor**: `maestro doctor` checks git presence/version, bwrap, db
  integrity (`PRAGMA integrity_check`), config validity, credential file modes,
  daemon liveness; prints a table; exits non-zero on hard failures.

## Testing bar

- Unit tests per module (plan validation, policy last-match + hard denials, scope
  overlap, context compaction, pricing, rate limiter, circuit breaker, loop
  detection, migrations).
- Store: lease CAS races (spawn N threads competing), retry cascades, approval
  expiry, merge-queue reconciliation, event bus lag tolerance.
- Engine E2E with mock providers + real temp git repos: full 5-role mission to
  completion; merge queue with two coders; failing integration check rolls back;
  scope violation fails the task; conflict spawns resolver tasks.
- Daemon: submit → SSE frames → steer → approve → finalize over real HTTP on an
  ephemeral port; auth required off-loopback; generation recovery.
- Latency smoke test: 1000 sequential event appends + lease acquisitions complete
  in < 10s total (asserts the <10ms budget with headroom).
- `cargo test --workspace` and `cargo clippy --workspace --all-targets` must pass.
