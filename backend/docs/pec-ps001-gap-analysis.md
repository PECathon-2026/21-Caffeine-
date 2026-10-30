# PECathon PS-001: requirements compared with Maestro

Assessment date: **2026-10-29**, with a guild/runtime update on **2026-10-30**.

Current installed guild mode uses Shogun (Astra), Samurai (GPT 6.1 Sol), Sentinel (GPT 6.1 Sol), and Shadow Council (Astra). New plans exclude Designer and require independent Sentinel and Shadow verdicts for every implementation patch, including low-risk work. LangGraph runs working-agent tool loops. The new website connects to actual daemon state; image inputs, managed sandboxed previews, code history and restore controls are implemented. The older five-role/Kimi results below are historical evidence, not the current model roster. General automatic repair/re-review and complete controlled PS evaluations remain gaps.

Source: `/home/anay/Downloads/PECathon_2026_Problem_Statements.pdf`, pages 1-4
(PS-001 ends near the top of page 4).

Problem statement: **DevPilot: The Mystic Mikado's Autonomous AI Engineering Guild**.
Compared against the current Rust implementation, including uncommitted fixes,
not the older TypeScript prototype.

## Assessment

Maestro is a strong architectural fit for PS-001. Its task graph, repository
tools, independent reviewer roles, durable shared state, worktrees, approvals,
and bounded runtime are already substantial parts of the requested system.

It is **not yet ready to claim complete PS-001 coverage**. The largest gaps are
automatic cross-agent repair/re-review, acceptance evidence enforced by the
runtime, and a reproducible evaluation demonstrating those behaviors.

Later on the same date, Foundry authentication was repaired and the live runtime
was switched to GPT 6.1 Sol + Kimi K2.7 Code. Production no longer includes the
mock transport. Dependency handoffs now include saved final-report evidence,
Git patch provenance and actual check receipts rather than summaries alone.
See [the live setup and verification report](foundry-gpt-kimi.md) for the current
results. A real five-role mission now demonstrates a failing test, Coder repair,
new feature implementation, security regression coverage, independent review,
and checked integration, with 11 passing tests. This combined smoke is not the
five separate controlled evaluations requested by the PS, and general automatic
negative-review repair/re-review remains a gap.

The next work should complete and demonstrate these behaviors in the existing
runtime. PS-001 does not require another language rewrite or a larger agent team.

## What the statement actually asks

The system must turn a natural-language request into a working, tested,
reviewed change to an **existing small repository**. It must respond to new
information and failures, rather than just execute a predetermined sequence
of persona prompts.

Two explicit requirements determine the assessment:

> "Sequential LLM calls with different personas do not count."

> "Demonstrate at least one complete end-to-end mission and one feedback loop
> in which new evidence changes what an agent does next."

Reported outcomes must come from actual test runs. Verified outcomes must be
distinguished from assumptions and model claims.

The five task sections are planning; implementation; testing and verification;
review and security; and coordination/adaptive autonomy. The eight evaluation
criteria are task completion, agent coordination, tool-calling intelligence,
adaptability, code quality, testing/verification, security/reliability, and
transparency. The PDF does not assign numerical weights or a passing percentage.

## Requirement-by-requirement comparison

**Implemented** means a corresponding mechanism exists in the source. It does
not certify every possible model outcome. **Partial** means some support exists,
but enforcement or demonstration is incomplete. **Gap** identifies a missing
first-class workflow or required evidence package.

| PS requirement | Status | What Maestro has | What remains |
| --- | --- | --- | --- |
| A: Requirement analysis | Implemented | Planner emits objectives, task descriptions, acceptance criteria and risk. Plan validation rejects malformed contracts. | Demonstrate that the criteria reflect a real request, including edge cases. |
| A: Codebase reconnaissance | Partial | Repository inventory/Git summary and project instructions enter planning; Designer/Coder/Reviewer can read and search files. Architect chat also has read tools. | Mission Planner has no exploration tool loop. A verified reconnaissance handoff before planning would improve relevant-file and convention discovery. |
| A: Decomposition, delegation and plan revision | Partial | Validated dependency DAG; role assignment; downstream dynamic delegation; plan editing before execution; conflict-resolution tasks. | No general evidence-driven replanning of a running mission while preserving completed work. |
| A: Shared mission state | Implemented | SQLite tasks, events, directives, verdicts, artifacts, approvals and status; bounded dependency reports and runtime receipts enter prompts. | Findings, decisions and unresolved issues are not a unified typed shared record consulted by every relevant agent. |
| B: At least three developer tools | Implemented | Coder has seven tools: read, list, search, write, exact replacement, configured check, delegation. | Nothing requires arbitrary shell access; configured checks are sufficient terminal tooling if useful to the demo. |
| B: Context-aware development | Partial | Project instructions, task contracts, repository summaries, read/search tools and dependency handoffs. | Verify preservation of existing style and behavior with a controlled repository and regression tests. |
| B: Adaptive problem solving | Partial | Tool results/errors feed the next turn; retries, model fallback, directives and delegation exist. | Retrying an attempt is not evidence of a changed approach. Cross-attempt repair context is not reconstructed as a structured findings handoff. |
| B: Controlled execution | Implemented with limits | Workspace-confined paths, symlink checks, Coder scopes, role restrictions, configured commands, Bubblewrap and timeouts. | Configured checks are trusted executable/argument definitions; permissions do not prove that every configured command is harmless. Keep the demo configuration restrictive. |
| C: Automated testing | Partial | `run_check` executes configured tests/build/lint; integration runs configured checks and rolls back failures. | Global `checks` is currently empty. No test execution is guaranteed when none are configured. |
| C: Failure investigation and repair | Gap in orchestration | An agent can see its own failed tool result and try another action. Merge-check failures are recorded and block safely. | A failed verification does not automatically create a developer repair task carrying the failure and then rerun verification. |
| C: Independent verification | Partial | Separate read-only Reviewer/Security tasks; fresh patch-hash-bound verdicts; independent check processes. | No explicit acceptance-criterion-to-check evidence contract. A separate Tester role is optional, not required by the PS. |
| C: Evidence-based completion | Partial | Actual patch hashes, invocation records, tool/check events, final artifacts and review gates. | Final-report evidence can be empty/model-authored; no invariant requires successful real checks for each acceptance criterion. |
| D: Code review | Implemented | Reviewer task for Coder patches; independent tool permissions; reviewed hash must match the patch. | Validate review quality against a planted regression or readability issue. |
| D: Security inspection | Partial | Security role; high-risk security gate; secret-path denials; injection-guard prompts; sandboxing. | No general exposed-secret/vulnerability scanning pipeline or planted-vulnerability evaluation was found. Blocking reads of `.env` is not a repository secret scan. |
| D: Cross-agent critique | Partial | Review verdicts and reasons are durable; reviewers can delegate downstream work; users can intervene. | `changes_requested` does not reliably route findings back to the original Coder and trigger bounded repair/re-review. |
| D: Human oversight | Implemented when configured | Plan approvals, tool-policy approvals and final integration approval. Global final approval is enabled. | Pin final approval on in the evaluation profile; automatic finalization is also supported and should not be used to demonstrate this requirement. |
| E: Inter-agent communication | Implemented foundation | Dependency summaries, task contracts, delegated tasks, reviewed patches and persistent shared state affect execution. | Demonstrate actual use of another agent's findings. Merely showing five role labels is insufficient. |
| E: Feedback loops | Partial; main submission gap | Tool-result loops and bounded merge-conflict resolver tasks exist. | Need a reproducible test/review failure followed by a changed developer action and successful re-verification, with causal links in the run log. |
| E: Adaptive orchestration | Partial | Continue/retry/block/pause/cancel/resume; bounded delegation; provider fallback; human steering/approvals. | Structured clarification requests and general runtime replanning are missing. |
| E: Bounded autonomy | Implemented foundation | Task/turn/retry/delegation limits, repeated-call detection, token/cost budgets, policy, approvals, leases and cancellation. | Cost ceilings depend on pricing accuracy. Record why the demo stops or escalates after repeated failures. |

## Why Maestro is more than sequential personas

The runtime uses an actual dependency scheduler and persistent execution state.
A Coder writes a patch in a worktree. A separate Reviewer inspects that work and
produces a verdict bound to its patch hash. That verdict affects whether the
merge queue can accept the patch. Agents can also propose downstream tasks, and
accepted dependency patches change what later Coders see in their worktrees.

Those are meaningful coordination mechanisms. Sequential execution itself is
not forbidden: current global concurrency is one, and the PDF lists parallel
agents as a bonus. The disallowed case is independent persona calls whose
results do not influence one another.

The weakness is the return path after negative evidence:

```text
Current ordinary negative-review path:
Coder patch -> Reviewer requests changes -> gate fails -> mission blocks

Target PS-001 demonstration:
Coder patch -> Verifier finds failing case
            -> failure evidence handed to Coder
            -> Coder revises implementation
            -> Verifier reruns the check against the new patch
            -> Reviewer approves that patch
            -> human approves final integration
```

The target is a proposed next step, not behavior certified by the current tests.

## What was verified in this assessment

Ran the existing targeted integration suites against the current working tree:

```bash
cargo test --locked -p maestro-engine --test orchestrator -p maestro-daemon --test daemon
```

Result: **39 passed, 0 failed**: 17 orchestrator integration tests and 22 daemon
integration tests. These use deterministic mock providers and temporary
repositories. They are real software tests of the runtime, not live-LLM
capability evaluations.

Relevant passing tests include:

- Full mission completion, human approval and final integration.
- Downstream Coder receiving already-merged dependency code.
- Delegated Coder automatically receiving a Reviewer.
- Review gates bound to the hash actually reviewed.
- Failed integration checks rolling back and blocking.
- Scope violations, token exhaustion and approval parking.
- Daemon recovery, pause/resume, SSE replay, steering and chat routing.

The failing-check test explicitly expects **Blocked**, not automatic repair.
That is direct evidence of the difference between safely handling a failure
and satisfying the requested test-driven recovery loop.

Separately, earlier work in this conversation ran 140 Store/TUI tests
successfully. Historical sessions reported broader suite counts; those counts
are not substituted for the current targeted run.

## Prior real-model work: what it establishes

From the recovered sessions, report and mission databases:

| Trial | Recorded/reported outcome | What can be claimed |
| --- | --- | --- |
| Small Foundry smoke mission | Historical report: 55 seconds, seven calls, about $0.19, patch integrated. | Evidence of a past direct-provider end-to-end run; not freshly reproduced today. |
| Initial 23-project portfolio | Historical report: roughly $8.70 and 87 calls, with interventions. Initial mission later cancelled after work was salvaged. | Useful stress testing and product output; not a clean autonomous completion claim. |
| Portfolio redesign | Mission database marks it completed; historical report says autonomous final integration, about $4.18. | A stronger historical complete mission example; still needs a curated, reproducible run log for submission. |
| Next.js Registry | 339 invocation rows; mission cancelled after manual finishing. Historical assessment: around 85% autonomous, approximately $53. | Meaningful multi-agent partial work and diagnosed limitations; not fully autonomous success. |

These large website trials are less suitable as the primary PS-001 demo than a
small seeded repository. The statement asks judges to see the causal connection
between evidence, revision, verification, and the final result.

## Evaluation scenarios: coverage and recommended fixtures

| Required scenario | Current evidence | Recommended controlled evaluation |
| --- | --- | --- |
| Feature implementation | Mock write/review/integration tests; historical live website and smoke trials. | Add a small API/CLI feature to an existing project; prove old and new behavior with tests. |
| Bug investigation | Repository inspection and tool execution exist; no dedicated PS bug-investigation live fixture identified. | Seed an off-by-one or validation defect with a failing assertion that identifies the behavior, not the fix. |
| Test-driven recovery | A failed check blocks safely; no automatic repair E2E test found. | Require failure -> verifier evidence -> Coder revision -> same test passing on the new patch. |
| Security and code review | Security roles/gates and authority tests; no seeded vulnerability mission found. | Plant an unsafe input-handling path; require a finding, repair and a regression test proving the input is handled safely. |
| Unexpected conditions | Existing bounds/cancellation/approval/recovery tests cover several runtime problems. | Test an unavailable configured executable, an ambiguous request, and repeated failures; require explicit recovery or escalation. |

Use stable local check commands and real exit codes. Capture the base commit,
objective, plan, agent findings, patch revisions, checks, verdicts, approvals,
final commit, unresolved items, model IDs, and measured usage. Preserve the
initial failed result as well as the later successful result.

## Deliverables and README readiness

| PS deliverable | Current state | Remaining packaging |
| --- | --- | --- |
| Source and working demonstration | Rust source exists; executable/daemon restored locally. | Working provider credentials and reproducible demo setup are needed. |
| Multi-agent architecture and roles | Seven-crate system and five roles; [architecture.md](architecture.md) now documents the actual system. | Link the current architecture and responsibilities from the submission README. |
| Implementation/testing/verification workflow | Much of the runtime exists. | Complete feedback and evidence enforcement; include runnable fixture checks. |
| One E2E mission and a log showing feedback | Historical runs and durable logs exist. | Export one understandable, reproducible mission that explicitly shows new evidence changing an action. |
| Setup/configuration/tools | README, config code and tool registry exist. | Refresh stale setup details, include a secret-free example config, supported checks/tools and boundaries. |
| Human approval/security assumptions/limitations | Mechanisms and details exist across source/docs. | Consolidate them in README; distinguish tested boundaries from intended guarantees. |

The required README should cover architecture and responsibilities;
communication/orchestration/shared state; setup/configuration; tools and
boundaries; human approval, security assumptions and known limitations.

## What the PS does not require

- No prescribed language, model, agent framework or number of agents.
- No requirement to call Claude Code/Codex/OpenCode CLIs. Direct provider APIs
  are explicitly compatible with the permitted use of models, libraries and APIs.
- No mandatory different model for each role. Independence can come from a
  separate verifier agent/process and enforceable tool boundaries.
- No mandatory GitHub hosting: a local repository is explicitly enough.
- No mandatory web interface, hosted deployment or Japanese visual theme in
  PS-001. PS-002 has visual theme requirements; do not transfer them to PS-001.
- No stated sub-10-ms requirement. Maestro's performance engineering is useful
  supporting work, but the listed criteria emphasize outcomes and adaptation.

## Bonus alignment

- **Parallel/dynamic agents:** scheduler and delegation already exist. Actual
  parallelism is disabled by the current global one-task limit; dynamic
  delegation has a passing integration test.
- **Automatic reruns and self-correction:** partial. Low-level retries exist;
  general test/review-driven correction is the main next implementation.
- **Persistent memory or single-agent baseline:** durable mission/chat state
  exists, but full conversation memory is incomplete. No controlled
  single-agent comparison was found.

## Recommended implementation order

1. Restore a working provider endpoint/key and verify a tiny real model call.
2. Add typed verification findings linked to task, acceptance criterion,
   check result and exact patch hash.
3. Add a bounded repair/re-review cycle. Route failures to a Coder, preserve
   evidence, invalidate stale approvals, rerun verification, and escalate after
   a configured correction limit. A Reviewer can serve as verifier; a sixth
   role is not necessary for compliance.
4. Require actual configured check results and criterion coverage for a
   "verified" outcome. When checks cannot run, report that explicitly instead
   of letting model prose imply success.
5. Build the five small fixtures, execute them, and export evidence. Keep a
   single short feature/bug mission with an intentional failing edge case as
   the primary demo.
6. Update README and a secret-free evaluation config; include honest known
   limitations and measured results. Consider a same-budget single-agent
   baseline only after the mandatory demonstration works.

The strongest next milestone is one small, repeatable mission whose final
change improves because a different agent produced new test or review evidence.

## Source map

- Planning, handoffs, retries, tool-result loop and final handling:
  [orchestrator.rs](../crates/maestro-engine/src/orchestrator.rs)
- Plan contracts and validation:
  [plan.rs](../crates/maestro-engine/src/plan.rs)
- Tool selection and configured checks:
  [tools.rs](../crates/maestro-engine/src/tools.rs)
- Gates, failed-check blocking, conflict tasks and final integration:
  [integration.rs](../crates/maestro-engine/src/integration.rs)
- Optional model-authored final evidence:
  [protocol.rs](../crates/maestro-types/src/protocol.rs)
- Runtime integration tests:
  [orchestrator tests](../crates/maestro-engine/tests/orchestrator.rs),
  [daemon tests](../crates/maestro-daemon/tests/daemon.rs)
- Historical benchmark/live trial report:
  [REPORT-2026-08-28.md](REPORT-2026-08-28.md)
- Current architecture:
  [architecture.md](architecture.md)
