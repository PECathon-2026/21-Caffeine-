# Maestro

Maestro is a local multi-model engineering control plane, written in Rust. It
coordinates Shogun (planning), Samurai (implementation), Sentinel (testing), and
Shadow Council (review/security) through a durable SQLite-backed task graph.
The `guildRoles = true` configuration enables these four contracts; the older
five-role mode remains available for existing projects.

This directory is the backend of the [combined Maestro repository](../README.md).
See [installation from a fresh clone](../docs/SETUP.md). Commands below assume
you have changed into `backend/`.

The deterministic runtime owns every state transition. Models can inspect the
project, propose plans, use explicitly granted tools, and return evidence. They
cannot mark work complete, grant permissions, approve their own patches, or run
arbitrary shell.

## Highlights

- **Durable task graph** — missions, tasks, invocations, gates, approvals,
  scope locks, merge queue, and an append-only event log in SQLite (WAL).
- **Planner-generated DAGs** with strict validation, plan approval/editing
  before execution, and runtime delegation with depth and count bounds.
- **Safe autonomy** — per-task git worktrees, planner-declared write scopes
  verified against the actual git diff, ordered policy rules with immutable
  hard denials, bubblewrap-sandboxed checks with `.env` masking, and
  patch-hash-bound reviewer/security verdicts.
- **Serial checked integration** — per-mission integration branch, merge queue
  that runs project checks and rolls back on failure, conflict-resolver task
  spawning, and a final fast-forward gated on a hash-bound approval.
- **Resilient providers** — Anthropic, OpenAI Responses, and Chat Completions transports (all
  with SSE streaming support), per-provider rate limiting, circuit breakers,
  retry hints, fallback chains, and integer micro-USD cost accounting. A
  deterministic fixture provider is available only in test builds. Production
  has no canned responses. Foundry quotas cover requests and estimated tokens.
- **Background daemon** — axum HTTP + SSE control plane with durable leases,
  daemon generations, crash recovery, and exclusive workspace locking. The TUI
  auto-spawns it, so missions survive your terminal.
- **Chat-first TUI** (ratatui) — live transcript, team sidebar, task DAG,
  diff and approval views, command palette, themes. Event-driven rendering:
  zero timers at idle, settled content never re-renders.
- **Built-in observability** — every store operation is latency-metered
  (p50/p99/max), `maestro bench` reports against the <10 ms control-plane
  budget, and `/api/metrics` exposes op latencies, RSS, and provider health.

## Quick start

```bash
cargo install --path crates/maestro-cli

cd /path/to/your/project
maestro init     # inherits global config, or writes a GPT starter config
maestro connect gpt  # needed only for the starter config; prompts for an API key
maestro          # opens the chat console
```

Inside the chat, questions are answered by the configured Planner;
implementation requests launch the autonomous team. Credentials are stored
separately with 0600 permissions. For the Azure GPT configuration, see
[examples/foundry.toml](examples/foundry.toml) and the
[verified setup](docs/foundry-gpt-kimi.md). A project config overrides the global
config completely; add project-specific checks before running engineering missions.

Useful commands: `maestro status`, `maestro submit "<objective>"`,
`maestro run "<objective>" --local`, `maestro approvals`, `maestro plan
<mission> --approve`, `maestro diff <mission>`, `maestro events <mission>
--follow`, `maestro bench`, `maestro doctor`.
Every command takes `--json` for automation.

## Architecture

```
CLI / ratatui TUI ──HTTP+SSE──> daemon (axum)
                                  └─ mission workers
                                      └─ orchestrator (engine)
                                          ├─ provider registry (rate limit, breaker, pricing)
                                          ├─ plan validator · policy · scopes · sandbox
                                          ├─ git worktrees · merge queue
                                          └─ SQLite store · event bus · metrics
```

Workspace crates: `maestro-types` (shared contracts), `maestro-store`,
`maestro-providers`, `maestro-engine`, `maestro-daemon`, `maestro-tui`,
`maestro-cli`. See `SPEC.md` for the full contract and invariants, and
`docs/UI-BRIEF.md` for the TUI design rules.

## Development

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p maestro-cli -- --version
```

Requires stable Rust, git, and (for sandboxed checks) bubblewrap. Linux-first.

Test dependencies explicitly enable `test-support` for deterministic fixtures.
Do not enable that feature when installing the application. A normal
`cargo build --release -p maestro-cli` excludes the mock transport and rejects
`kind = "mock"` configuration.

## LangGraph agents

Run `python scripts/install-langgraph.py --configure` to enable LangChain native
tool calling and checkpointed LangGraph execution for working agents. Fixed role
models, Rust tool permissions, worktrees, and integration gates are preserved.
See [the runtime architecture and setup](docs/langgraph.md).

## Code history

Use `/history` in the TUI to inspect saved project and draft versions. `/undo`
previews undoing the last request; `/restore <version>` previews a selected
version. Both show a diff before applying and preserve a recovery version.
See [History and restore](docs/history.md) for CLI commands and recovery semantics.

### Visual and preview tools

Use `/attach <image-path>` to send a reference image, and ask Maestro to inspect repository images or capture a local app. `/preview` shows managed preview URLs/logs; `/preview stop` stops them. Agents also have ranged file reads, per-file Git diffs, and npm project checks. See [visual tools](docs/visual-tools.md) for setup, limits and the data flow.
