# Maestro — current Rust architecture

> **Runtime update:** the installed working-agent loop now uses LangChain native
> tool calls and LangGraph checkpoints. See [LangGraph execution](langgraph.md)
> for the current boundary and snapshot behavior. The custom Rust turn protocol
> described below remains the `native` compatibility backend; planning and
> architect chat still use Rust.


The current presentation client and four-agent role mapping are documented in
[the website integration guide](../../21-Caffeine-/docs/DAEMON_INTEGRATION.md).
Enable `[runtime] guildRoles = true` for Shogun/Samurai/Sentinel/Shadow contracts and both independent gates.

Source snapshot: **2026-10-29**, including the current uncommitted fixes.
Implementation: `/home/anay/Desktop/Projects/maestro-rs`, Rust package version
`0.1.0`. The sibling `Maestro/` directory is the older TypeScript prototype.

Maestro is its own agent runtime. It calls provider APIs directly, builds model
context, executes tools, schedules tasks, and manages Git integration. It does
not execute Claude Code, Codex CLI, or OpenCode to do the work. Their local
configuration was used as a source of provider settings and credentials.

## 1. Whole-system view

```mermaid
flowchart TB
    User["User"]

    subgraph Client["Client process — installed maestro executable"]
        CLI["maestro-cli<br/>commands, initialization, diagnostics"]
        TUI["maestro-tui<br/>Ratatui chat, tasks, diffs, approvals"]
        Local["Optional local execution<br/>CLI engine/store path"]
        CLI --> TUI
        CLI --> Local
    end

    subgraph Server["Detached maestro serve process — one per workspace"]
        API["maestro-daemon<br/>Axum HTTP API"]
        SSE["SSE event delivery<br/>durable replay plus live events"]
        Workers["Mission worker registry<br/>owned Tokio tasks and cancellation"]
        Engine["maestro-engine<br/>planner, scheduler, agent loops"]
        Context["Context compiler<br/>task, instructions, handoffs, evidence"]
        Tools["Tool registry<br/>role, policy, path and scope checks"]
        Integration["Git integration manager<br/>worktrees, gates, merge queue"]
        Store["maestro-store<br/>SQLite connection, event bus, metrics"]
        Providers["maestro-providers<br/>models, rate limits, circuit breakers"]
        API --> Workers --> Engine
        API --> Store
        API --> Engine
        Engine --> Context
        Engine --> Providers
        Engine --> Tools
        Engine --> Integration
        Engine --> Store
        Integration --> Store
        Store --> SSE
    end

    subgraph External["External resources"]
        Models["GPT Responses<br/>Azure Foundry"]
        Repo["Project Git repository<br/>task and integration worktrees"]
        Checks["Bubblewrap check process<br/>configured executable and arguments"]
        DB[(".maestro/maestro.db")]
    end

    User --> CLI
    User --> TUI
    CLI -->|HTTP commands| API
    TUI -->|HTTP commands and snapshots| API
    SSE -->|HTTP event stream| TUI
    SSE -->|Follow mode| CLI
    Local -.->|Same engine and store implementation| Engine
    Providers -->|Direct HTTPS requests| Models
    Tools --> Repo
    Tools --> Checks
    Integration --> Repo
    Integration --> Checks
    Store --> DB
```

The dotted local path represents an alternative execution mode in the CLI
process; it does not call a daemon-resident engine. Agent workers are async
tasks, not independent Claude/Codex processes. Git and configured checks do
spawn operating-system processes.

## 2. Chat routing and the architect

```mermaid
flowchart TD
    Input["User submits plain text"] --> Save["Ensure chat session exists<br/>Persist user message"]
    Save --> Active{"Session has an active mission?"}
    Active -->|Yes| Steer["Persist steering directive<br/>Wake mission worker"]
    Steer --> Next["Directive enters subsequent task prompts"]
    Active -->|No| Intent{"Implementation-request heuristic"}
    Intent -->|Work request| Git{"Git repository with a commit?"}
    Git -->|No| Explain["Persist actionable launch refusal<br/>Show it in chat"]
    Git -->|Yes| Mission["Create mission<br/>Link it to chat session<br/>Start worker"]
    Intent -->|Question or small talk| Architect["Conversational architect<br/>Uses Planner model assignment"]
    Architect --> Call["Direct provider request"]
    Call --> Kind{"Leading tool-call JSON?"}
    Kind -->|Yes| Read["Execute real read-only tool<br/>list_files, read_file, search"]
    Read --> Evidence["Append bounded tool result<br/>to this question's context"]
    Evidence -->|Within eight-turn limit| Call
    Kind -->|No, prose answer| Reply["Persist assistant answer<br/>Return it to TUI"]
```

Slash commands provide explicit actions; the diagram shows ordinary chat text.
The heuristic strips common courtesy prefixes, keeps informational questions
and small talk conversational, and otherwise defaults to work. It is not a
model-based intent classifier.

The architect has its own smaller tool loop and protocol:

```json
{"tool":"read_file","args":{"path":"README.md"}}
```

It uses read-only enforcement even though it calls the Planner's model.
Saved sessions replay in the UI, but earlier saved chat turns are **not currently
loaded into `answer_question`**. Each question starts a new model context with
its own tool results. Architect calls also bypass mission invocation accounting
and mission token/cost budgets.

## 3. Mission planning and agent execution

```mermaid
sequenceDiagram
    actor U as User
    participant D as Daemon
    participant O as Orchestrator
    participant P as Provider registry
    participant M as Remote model API
    participant S as SQLite store
    participant T as Repository tools

    U->>D: Submit objective
    D->>O: Create mission and start worker
    O->>S: Persist mission
    O->>P: Planner prompt and repository summary
    P->>M: Direct model request
    M-->>P: Plan JSON
    P-->>O: Model response and usage
    O->>O: Validate DAG, roles, scopes, limits
    O->>S: Persist plan, tasks and invocation
    opt Plan approval configured
        O->>S: Await plan approval
        U->>D: Approve or edit plan
        D->>O: Resume approved plan
    end
    loop While runnable tasks remain
        O->>S: Recover leases and refresh ready tasks
        O->>O: Merge eligible patches before next wave
        O->>S: Claim task lease and scope locks
        loop Bounded agent attempt
            O->>O: Compile role and task context
            O->>P: Model request
            P->>M: Direct HTTPS request
            M-->>P: One tool call or final JSON report
            P-->>O: Response, usage and estimated cost
            O->>S: Record invocation and agent turn
            alt Tool call
                O->>T: Check authority and execute
                T-->>O: Tool result or policy decision
                O->>S: Persist operational event
            else Final report
                O->>O: Validate final and patch or verdict
                O->>S: Save artifacts, gates and task outcome
            end
        end
        O->>S: Release locks and refresh dependent tasks
    end
    O->>O: Complete integration or enter resting state
    S-->>D: Events available for live delivery and replay
    D-->>U: Status, evidence, diff and approvals
```

Plans contain 1–12 tasks with dependencies pointing to earlier tasks. Runtime
delegation can expand the graph subject to configured depth and task-count caps.
Task attempts use leases, heartbeat renewal, guarded outcome writes, and scope
locks. Independent ready tasks can run concurrently when configuration permits.

Mission agents use Maestro's JSON-in-text protocol, not provider-native tool
execution:

```json
{"kind":"tool","tool":"read_file","arguments":{"path":"src/main.rs"}}
```

The model proposes the tool call. Maestro validates and executes it. A final
report contains `status`, `summary`, `evidence`, and `risks`; Reviewer/Security
also supply a verdict and reasons.

## 4. How the five roles collaborate

This is an example valid graph, not a fixed pipeline used by every mission.

```mermaid
flowchart LR
    Planner["Planner<br/>creates validated task DAG"] --> Design["Designer task<br/>design summary"]
    Design -->|Dependency handoff| CoderA["Coder A<br/>scoped patch A"]
    Design -->|Dependency handoff| CoderB["Coder B<br/>scoped patch B"]
    CoderA --> ReviewA["Reviewer A<br/>hash-bound verdict"]
    CoderB --> ReviewB["Reviewer B<br/>hash-bound verdict"]
    CoderA --> Security["Security task<br/>required for high-risk patch"]
    ReviewA --> Gates["Fresh approved gates"]
    ReviewB --> Gates
    Security --> Gates
    Gates --> Queue["Serial checked merge queue"]
    CoderA -.->|delegate_task| Child["Downstream delegated task"]
    Child -->|If Coder, auto-pair review| ChildReview["Delegated Reviewer"]
    ChildReview --> Gates
```

| Role/path | Tools currently registered |
| --- | --- |
| Mission Planner | No repository tools; receives supplied context and produces plan JSON |
| Designer | `list_files`, `read_file`, `search`, `delegate_task` |
| Coder | Read/search tools, `write_file`, `replace_text`, `run_check`, `delegate_task` |
| Reviewer / Security | Read/search tools, `run_check`, `delegate_task`; no writes |
| Conversational architect | `list_files`, `read_file`, `search` only |

Communication uses task contracts, completed dependency summaries, accepted
code, delegation, and verdict records. Dependency handoffs include the latest
model final report, runtime patch provenance (hash, base, changed paths and
scope verification), and the first and most recent runtime check receipts.
Each receipt has an artifact ID and check receipts include the actual command,
workspace, exit status and bounded output. Model-authored claims are labeled
separately from runtime receipts. Full agent conversations are not shared. A delegated task depends on its parent completing.

There is no free-form inter-agent chat bus and no automatic general-purpose
`changes_requested → original coder revision → re-review` loop. A failed gate
blocks integration. Conflict resolution during merging has its own bounded
Coder/Reviewer task creation mechanism.

## 5. Provider transport and reliability

```mermaid
flowchart TD
    Role["Role model assignment<br/>or task model override"] --> Chain["Primary model and fallback candidates"]
    Chain --> Budget["Mission invocation path<br/>check token and cost ceilings"]
    Budget --> Retry["Bounded provider retry loop<br/>backoff and retry hints"]
    Retry --> Registry["ProviderRegistry"]
    Architect["Architect question path<br/>candidate loop without mission accounting"] --> Registry
    Registry --> Guard["Circuit-breaker admission<br/>request and estimated token quota"]
    Guard --> Transport{"Provider kind"}
    Transport -->|anthropic| Anthropic["reqwest<br/>POST baseUrl + /v1/messages<br/>x-api-key"]
    Transport -->|openai_compatible| OpenAI["reqwest<br/>POST baseUrl + /chat/completions<br/>Bearer credential"]
    Transport -->|openai_responses| Responses["reqwest<br/>POST baseUrl + /responses<br/>Bearer credential"]
    Anthropic --> Result["ModelResponse<br/>text, usage, latency"]
    OpenAI --> Result
    Responses --> Result
    Result --> Pricing["Pricing lookup<br/>estimated integer micro-USD cost"]
    Pricing --> Caller["Return to calling loop"]
    Guard -.->|Failure| Retry
    Retry -.->|Candidate exhausted| Chain
```

All network transports implement `generate` and `stream`. The current mission
and architect loops call **`generate`**, so their model responses are consumed
whole. The daemon's **SSE mission event stream is a different channel** from
provider token streaming.

GPT uses Responses (`input`, `max_output_tokens`, `text.format`, `store: false`);
the optional Chat Completions transport supports `messages`, `max_tokens`, and
`response_format`, but no active deployment uses it.
Incomplete, failed, or empty Responses outputs are errors; only assistant text
enters the agent protocol. No Claude/Codex/OpenCode CLI adapter is involved.
Production excludes the mock transport; test dependencies explicitly enable it.

All seven deployed GPT variants are configured. Kimi and other low-quota models
are excluded from active providers, roles, fallbacks, and the picker.
Pinned deployment IDs prevent Azure's global model catalog from populating the
picker with unavailable models. Per-deployment request buckets and rolling token
reservations guard both generate and stream calls. Token reservations estimate
input as UTF-8 bytes / 3 plus message overhead and reserve the output cap for
60 seconds. This is an estimate; Azure's 429/Retry-After remains authoritative.
Unknown pricing entries can record zero estimated cost, so dollar budgets are
only as accurate as the pricing catalog. Provider cancellation uses propagated
cancellation tokens.

## 6. Tool authority and check isolation

```mermaid
flowchart TD
    Proposed["Parsed model tool request"] --> Registry{"Tool registered for role?"}
    Registry -->|No| Error["Return denied or invalid tool result"]
    Registry -->|Yes| Path["For path tools: resolve workspace path<br/>reject traversal and symlink escape"]
    Path --> Policy["Policy evaluation on resolved resource<br/>hard denials plus ordered rules"]
    Policy --> Decision{"allow / ask / deny"}
    Decision -->|deny| Error
    Decision -->|ask| Approval["Create or consult durable approval<br/>park task until decision"]
    Approval -->|Approved and task retried| Policy
    Decision -->|allow| Kind{"Operation"}
    Kind -->|Read/search/list| Read["Bounded repository result"]
    Kind -->|Write/replace| Scope["Verify Coder role and write scope"]
    Scope --> Write["Modify task worktree"]
    Kind -->|Configured check index| Sandbox["Bubblewrap sandbox<br/>filtered environment, secret masking<br/>timeout and bounded output"]
    Kind -->|Delegation| Delegate["Validate child role and limits<br/>persist downstream task"]
    Read --> Evidence["Result becomes next-turn evidence"]
    Write --> Evidence
    Sandbox --> Evidence
    Delegate --> Evidence
```

Reviewer/Security writes and external-directory access are hard-denied. Secret
file matching covers `.env` and `.env.*`, with `.env.example` explicitly allowed;
this is not a universal detector for every possible secret-bearing file.

Checks use configured executable/argument arrays, not arbitrary shell strings
chosen by the model. Bubblewrap is Linux-specific and controls check processes.
It does not contain the daemon's own provider HTTPS traffic. Required sandbox
mode fails closed; other configured modes have different guarantees.

## 7. Git worktrees, review gates, and final integration

```mermaid
flowchart TD
    Base["User repository<br/>capture original HEAD"] --> Mission["Mission integration branch<br/>integration worktree"]
    Mission --> Fork["Coder task worktree and branch<br/>fork from current accepted base"]
    Fork --> Write["Coder edits and completes task"]
    Write --> Validate["Commit task work<br/>verify actual patch scopes<br/>calculate patch hash"]
    Validate --> Review["Reviewer and optional Security<br/>verdicts bind to hashes captured<br/>at review-attempt start"]
    Review --> Gate{"Fresh approved gates?"}
    Gate -->|No| Block["Block integration<br/>retry or intervention needed"]
    Gate -->|Yes| Queue["Enqueue patch<br/>recheck freshness at merge time"]
    Queue --> Merge["Serial merge into mission branch"]
    Merge --> Conflict{"Merge conflict?"}
    Conflict -->|Yes, within bounds| Resolver["Create scoped resolver Coder<br/>and dependent Reviewer"]
    Resolver --> Validate
    Conflict -->|No| Checks["Run configured integration checks"]
    Checks -->|Failure| Rollback["Reset failed merge<br/>preserve failure evidence"]
    Checks -->|Pass| Accepted["Advance accepted integration head"]
    Accepted -.->|Before next spawn wave| Fork
    Accepted --> Finish{"All task work and gates complete?"}
    Finish -->|No| Continue["Continue scheduling"]
    Finish -->|Yes| FinalApproval{"Final approval enabled?"}
    FinalApproval -->|Yes| Human["User approves aggregate diff hash"]
    FinalApproval -->|No| Revalidate["Validate aggregate state<br/>clean checkout and original HEAD"]
    Human --> Revalidate
    Revalidate -->|Valid| FF["Fast-forward user branch<br/>mark completed and clean up"]
    Revalidate -->|Dirty or HEAD moved| Retain["Recoverable refusal<br/>preserve mission work"]
```

This shows the normal `mission-branch` strategy. A configurable `direct`
integration strategy also exists and has different behavior. Missions with no
patch can complete without final integration.

Retried Coder worktrees are refreshed when their saved fork point no longer
matches the accepted integration head. Failed missions preserve their mission
branch; explicit cancellation can delete it. Paused/blocked missions retain
workspaces for recovery. There is currently no mission-rebase command when the
user's HEAD changes during a mission.

## 8. Persistent data model

Logical relationships below describe runtime associations; not every edge is
implemented as a SQL foreign-key constraint. Optional task/mission links are
used for chat and mission-level invocations.

```mermaid
erDiagram
    MISSIONS ||--o{ TASKS : contains
    MISSIONS ||--o{ EVENTS : records
    MISSIONS ||--o{ INVOCATIONS : meters
    MISSIONS ||--o{ APPROVALS : requests
    MISSIONS ||--o{ DIRECTIVES : receives
    MISSIONS ||--o{ MERGE_QUEUE : integrates
    MISSIONS ||--o{ ARTIFACTS : retains
    TASKS ||--o{ INVOCATIONS : invokes
    TASKS ||--o{ GATES : patch_target
    TASKS ||--o{ GATES : reviewing_source
    TASKS ||--o{ SCOPE_LOCKS : claims
    TASKS ||--o{ TASKS : delegates
    CHAT_SESSIONS ||--o{ CHAT_MESSAGES : contains

    MISSIONS {
        string id PK
        string objective
        string status
        string plan_json
        string base_head
        string integration_head
        string final_approval_id
    }
    TASKS {
        string id PK
        string mission_id
        string task_key
        string role
        string dependencies_json
        string write_scopes_json
        string lease_owner
        int lease_expires_at
        string lease_generation
        string patch_hash
        string status
    }
    EVENTS {
        int seq PK
        string mission_id
        string task_id
        string kind
        string payload
    }
    INVOCATIONS {
        string id PK
        string provider_id
        string model
        int input_tokens
        int output_tokens
        int cost_micro_usd
        int latency_ms
        string status
    }
    GATES {
        string task_id
        string source_task_id
        string role
        string verdict
        string patch_hash
    }
    APPROVALS {
        string id PK
        string action
        string resource
        string patch_hash
        string policy_snapshot
        string status
    }
    DIRECTIVES {
        string id PK
        string mission_id
        string task_id
        string text
    }
    SCOPE_LOCKS {
        string mission_id
        string task_id
        string scope
    }
    MERGE_QUEUE {
        string id PK
        string task_id
        int position
        string patch_hash
        string status
        string merge_commit
    }
    ARTIFACTS {
        string id PK
        string kind
        string content
        string content_hash
    }
    CHAT_SESSIONS {
        string id PK
        string title
        string active_mission_id
    }
    CHAT_MESSAGES {
        string id PK
        string session_id
        string role
        string content
        string mission_id
        string task_id
        string approval_id
    }
```

Additional tables track daemon generations and provider health. SQLite uses WAL,
versioned `PRAGMA user_version` migrations, prepared statements, and a shared
connection protected by a mutex. In-memory broadcast events support live
subscribers; persisted sequence numbers support replay after reconnect. Metrics
record store-operation latencies, and the daemon exposes metrics and RSS.

## 9. Configuration and credential resolution

```mermaid
flowchart LR
    Project["Project maestro.toml"] --> Select{"Project config exists?"}
    Global["User maestro.toml<br/>MAESTRO_CONFIG_DIR or XDG/HOME path"] --> Select
    Select -->|Yes: whole project file| Effective["Effective config"]
    Select -->|No: use user file| Effective
    Effective --> Roles["Role assignments and fallbacks"]
    Effective --> Runtime["Concurrency, attempts, budgets"]
    Effective --> Safety["Policy, checks and sandbox settings"]
    Effective --> Integration["Integration strategy and approvals"]

    GlobalKeys["User credentials.json"] --> Overlay["Credential map<br/>project entry overrides same provider"]
    ProjectKeys["Project .maestro/credentials.json"] --> Overlay
    Overlay --> Inject["Populate provider apiKeyEnv<br/>only when variable is unset"]
    Env["Existing process environment"] -->|Takes precedence| Inject
    Inject --> Requests["Provider request headers"]
```

Project config replaces the user config **wholly**, rather than merging each
field. Credentials use per-provider overlays. Saved keys are separate from
model configuration and use `0600` files. Production's starter config uses GPT
and requires a key. The installed global config uses the current Foundry
resource key, retrieved through the authenticated Azure CLI because the inherited
Codex key was stale.

Current global assignments, updated on 2026-10-30:

| Role | Model | Fallback |
| --- | --- | --- |
| Planner | `foundry-astra/gpt-6-astra` | GPT 6.1 Sol, GPT 6 Sol |
| Designer | `foundry-sol/gpt-6-sol` | GPT 6.1 Sol, GPT 5.6 Terra |
| Coder | `foundry-gpt/gpt-6.1-sol` | GPT 6 Sol, GPT 5.6 Sol |
| Reviewer | `foundry-gpt/gpt-6.1-sol` | GPT 6 Sol, Astra |
| Security | `foundry-astra/gpt-6-astra` | GPT 6.1 Sol |

Every enabled deployment: 1,000 RPM / 1,000,000 TPM.
All roles use 48,000 context characters / 8,192 output tokens.
These are Maestro operating budgets, not claimed model context
maximums. Azure's model metadata did not report context maxima.
Global concurrency is two tasks, with two million mission tokens and final
approval enabled. The estimated-cost ceiling is $20, using reference rates
of $10/$50 per million input/output tokens for Astra, $2/$10 for GPT 6.1 Sol
and GPT 6 Sol.
These estimates exclude Azure-specific billing adjustments and cache accounting.
Astra reasoning effort is `medium`; other GPT deployments use `low`. Global `checks` is empty;
checks must be supplied by each project. See [Foundry setup and verification](foundry-gpt-kimi.md).

## 10. Startup, recovery, and the terminal UI

```mermaid
flowchart TD
    Launch["maestro launched"] --> UI["TUI connecting view"]
    UI --> Probe["Read workspace daemon.json<br/>probe /health"]
    Probe --> Live{"Healthy daemon?"}
    Live -->|No| Spawn["Spawn detached maestro serve<br/>stderr to private workspace log"]
    Spawn --> Boot["Load config and credentials<br/>open and migrate database"]
    Boot --> Legacy{"Legacy TypeScript schema?"}
    Legacy -->|Yes| Fail["Reject incompatible schema<br/>surface startup error"]
    Legacy -->|No| Lock["Acquire workspace lock<br/>bind loopback ephemeral port"]
    Lock --> Generation["Write discovery metadata<br/>register daemon generation"]
    Generation --> Recovery["Reclaim orphaned leases and locks<br/>reconcile merge queue<br/>resume eligible mission work"]
    Recovery --> Ready["Serve HTTP and SSE"]
    Live -->|Yes| Build{"Matching executable build?"}
    Build -->|Yes| Attach["Attach snapshots and event stream"]
    Build -->|No, idle| Restart["Stop old daemon and respawn"]
    Restart --> Spawn
    Build -->|No, busy| Keep["Attach with outdated-build notice"]
    Keep --> Attach
    Ready --> Attach
    Attach --> Welcome["Fresh chat session<br/>recent-session picker"]
    Welcome --> Render["Event-driven terminal rendering<br/>cached transcript and diff windows"]
```

The Rust daemon defaults to **`127.0.0.1:0`**: the operating system chooses an
available port, published in `.maestro/daemon.json`. A token is required to bind
off-loopback; configured tokens protect `/api` routes even on loopback.

The UI has Chat, Tasks, Diff, Approvals, and Events views; slash suggestions;
model/role and session pickers; cached markdown rendering; and a responsive
team sidebar. Repainting uses a dirty flag and an approximately 33-ms frame
budget. Animation ticks run when activity requires them. This frame budget is
separate from the historical sub-10-ms target for local control operations.

Quitting the UI does not stop the detached daemon. Daemon shutdown cancels and
joins mission workers with a bounded worker grace period. A hard bound on the
entire HTTP graceful-shutdown drain remains a known follow-up.

## 11. Files and source entry points

```text
~/.local/bin/maestro                 Installed standalone Rust executable
~/.config/maestro/
  maestro.toml                      User-level model/runtime configuration
  credentials.json                  User-level provider credentials

<project>/
  maestro.toml                      Optional whole-project override
  .maestro/
    maestro.db                      Rust SQLite state
    daemon.json                     Bound URL, PID and generation
    daemon.lock/                    Exclusive workspace lock
    daemon.stderr.log               Detached startup/runtime stderr
    credentials.json                Optional credential overrides
    instructions.md                 Optional project instructions
    worktrees/                      Isolated task checkouts
    integration/                    Mission integration checkouts
```

| Concern | Source |
| --- | --- |
| Shared contracts | [maestro-types/src](../crates/maestro-types/src) |
| CLI dispatch and launch | [lib.rs](../crates/maestro-cli/src/lib.rs), [context.rs](../crates/maestro-cli/src/context.rs) |
| TUI event loop | [app.rs](../crates/maestro-tui/src/app.rs) |
| HTTP routing and chat | [http.rs](../crates/maestro-daemon/src/http.rs) |
| Owned mission workers | [workers.rs](../crates/maestro-daemon/src/workers.rs) |
| Planning and agent loops | [orchestrator.rs](../crates/maestro-engine/src/orchestrator.rs) |
| Prompts and context | [prompts.rs](../crates/maestro-engine/src/prompts.rs), [context.rs](../crates/maestro-engine/src/context.rs) |
| Tools, policy and sandbox | [tools.rs](../crates/maestro-engine/src/tools.rs), [policy.rs](../crates/maestro-engine/src/policy.rs), [sandbox.rs](../crates/maestro-engine/src/sandbox.rs) |
| Worktrees and integration | [worktrees.rs](../crates/maestro-engine/src/worktrees.rs), [integration.rs](../crates/maestro-engine/src/integration.rs) |
| Provider transport | [registry.rs](../crates/maestro-providers/src/registry.rs), [anthropic.rs](../crates/maestro-providers/src/anthropic.rs), [openai_compatible.rs](../crates/maestro-providers/src/openai_compatible.rs) |
| Database schema and migrations | [migrations.rs](../crates/maestro-store/src/migrations.rs) |

## 12. Current boundaries

- No Claude Code, Codex CLI, or OpenCode adapter.
- No MCP client/plugin runtime, semantic/LSP context retrieval, or cross-repo
  parent-folder mission orchestration.
- No general automatic failed-review correction loop or free-form agent chat.
- Saved chat history is visible in the UI but is not full model conversation
  memory. Architect calls are outside mission accounting.
- Provider streaming exists as a transport capability; the current agent loops
  do not consume it.
- No mission-rebase command for a changed user HEAD.
- Foundry authentication is repaired; GPT and Kimi have both answered live requests.

Historical implementation and benchmark evidence:
[REPORT-2026-08-28.md](REPORT-2026-08-28.md). UI design intent:
[UI-BRIEF.md](UI-BRIEF.md). Those older documents may describe intended behavior
or an earlier snapshot; this document follows the current source paths.


## Inspection routing and blocked-run visibility (2026-10-29)

Requests such as “look into the repo and explain it” and “go through the project”
use read-only architect chat. An explicit follow-up action, such as “and fix the
login bug,” still launches an implementation mission. Standalone Reviewer and
Security tasks with no dependencies can inspect the repository directly; tasks
with declared dependencies still require those dependency workspaces.

The TUI fetches role/model assignments when connecting and keeps that roster
visible while idle. Blocked and paused mission states are announced in the
transcript, task failures include their reason, and blocked snapshots retain a
visible explanation when resumed. Informational chat detaches from a stopped
mission while preserving its history.

Verified against the reported Family-Tree request: the exact wording returned an
architect explanation in 41.1 seconds, kept the mission count at one (the original
failed run), and cleared the chat's active-mission association. The existing
working-tree diff and Git status were unchanged. Routing, daemon, engine and TUI
checks passed: 230 relevant tests.

Follow-up: conversational prefixes are stripped repeatedly, so “Now explain me
the tech stack of this project” and “Can you now explain…” also stay in architect
chat. The former wording previously launched a Kimi review mission whose tool
loop repeatedly waited for its 20,000-token-per-minute quota. The corrected live
request returned an explanation in 32.1 seconds, created no mission, and preserved
the application working tree. Engine and daemon checks passed: 132 tests.


## Task recovery and transcript navigation (2026-10-30)

Working agents judge completion against their own task, rather than the whole
mission objective. Designers deliver a design handoff; inability to edit or
launch the server is expected for that role. Before accepting a blocked report,
the runtime gives one task-scope clarification within the existing turn budget.
A repeated genuine block is honored. Final blocked/failed reports persist their
reasons (falling back to the summary) in task state and events. Legacy reports
can display their summary when no reason was stored.

Up to two malformed protocol replies can be corrected within the same attempt,
retaining gathered tool evidence instead of restarting inspection. This does not
execute malformed calls or mark incomplete tasks successful.

The transcript clamps scrolling to the first full viewport, including after
resize or tool-output collapse. User messages have a full-width highlight; the
current request stays at the top while following, and the relevant earlier
request is pinned while reading history. Sidebar task titles wrap. Press `2`,
select a task, and press Enter for full task details; arrows/j/k, mouse wheel,
and Page Up/Down scroll descriptions, reasons, acceptance criteria, and summaries.

The reported Family-Tree Designer task was replayed with real GPT calls in a
separate evidence store. The first replay identified absent dependencies and
Next.js guides. After `npm ci`, the same task completed in 108 seconds and
produced a concrete design handoff. Existing Git status and tracked diff were
preserved. This verified the design stage; it did not run the redesign or launch
stages of the original mission. See
[replay evidence](evidence/designer-recovery-2026-10-30.json).

Validation: 236 engine, daemon, and TUI tests passed. An actual PTY session
verified top-of-history scrolling, a highlighted pinned latest request, wrapped
sidebar titles, and task-detail navigation. The release binary was installed and
both idle user daemons restarted with the updated engine.

## Visual inputs and local validation

The current engine also supports durable image attachments, workspace image viewing, Playwright inspection of localhost pages, managed Next.js/Vite previews, ranged reads and per-file diffs. Image payloads are hydrated at invocation time; checkpoints retain references. See [visual tools](visual-tools.md) for the detailed Mermaid flow and execution limits.
