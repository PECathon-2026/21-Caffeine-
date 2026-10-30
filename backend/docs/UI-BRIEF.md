# TUI design brief (distilled from keelcode-cli, opencode, cli-perf-brief)

Design language: **flat minimalism**. No gradients, no glows, no decorative noise.
One restrained accent color. Hierarchy from spacing, gutter glyphs, and dimness —
not from color count.

## Hard performance rules (ported from keelcode's perf postmortem)

1. **Zero timers at idle.** No interval may exist unless something is animating
   (spinner while a task runs, elapsed clock while a mission is active). Ticker
   tasks are spawned when activity starts and aborted when it settles.
2. **One repaint clock.** All redraw requests funnel through a single dirty flag
   with a ~33ms minimum frame budget. A stream flush that repaints also advances
   the spinner/elapsed state — never two competing clocks.
3. **Settled content never re-renders.** Pre-render completed transcript turns
   into styled line buffers keyed by (turn id, width, theme); invalidate only on
   width/theme change. Only the unsettled streaming tail re-renders per flush.
4. **Viewport-only drawing.** Transcript is a flat Vec of pre-styled lines; draw
   the visible slice only. Never lay out offscreen rows.
5. **Streaming text is O(len) not O(len²).** Commit rendered prefix at safe block
   boundaries; re-render only the growing tail.
6. **Bounded everything.** Transcript ring buffer, event feed, tool output blocks
   (cap ~16 lines with "+N more"), caches with eviction. Measure RSS in tests.
7. **Skip `terminal.draw` entirely** when nothing changed.
8. **Leak hygiene:** every spawned task owned (JoinHandle kept, aborted on drop);
   raw-mode/alt-screen restored via Drop guard AND a panic hook; pending
   approval prompts denied-on-shutdown, never leaked.

## Layout (chat-first)

- **Header (1 row, can shed segments right-to-left when narrow):**
  brand · cwd · git branch+dirty · mission status · model. Never wraps — drops.
- **Transcript** (main area): gutter grammar — column-0 glyph tells the line kind
  before color: `❯` user, `◆` assistant/architect, `⏵` tool call, `⎿` tool
  result/continuation, `·` system. Tool outputs collapsed to a few lines,
  Ctrl+O toggles expanded detail.
- **Right sidebar (~40 cols, toggle with leader+b, auto-hidden < 100 cols):**
  team state — one row per active role: glyph, role, model, current task title,
  spinner. Below: mission task list with status glyphs.
- **Status line (1 row, segments earn their place, drops before wrapping):**
  left: mode block (CHAT / MISSION / APPROVAL reversed-video) + key hints;
  right: pending-approvals ⚠ count, daemon dot (green connected / red down),
  tokens, cost $, elapsed, context-pressure % only past 60%.
- **Composer:** bottom, placeholder "what should we build?", grows to 6 rows.
- **Tabs/views:** Chat (default), Tasks, Diff, Approvals, Events. Number keys
  1-5 switch when composer is empty; also via palette.

## Interaction vocabulary

- Slash commands typed in composer: /ask /run /plan /approve /reject /steer
  /status /models /model /connect /settings /sessions /new /help /quit.
- Command palette on Ctrl+P: fuzzy over commands + sessions + models.
- Model picker: fuzzy list, active model marked, context-size metadata per row.
  Selection is presentational — never mutates an in-flight request.
- Approval prompt: inline bordered card in the chat flow:
  `◆ Approval  <action>` / one-line collapsed resource / reason /
  `[a] approve · [d] deny · [v] view diff`. Edit/integrate approvals open the
  Diff tab scrolled to the patch. Escape returns without deciding.
- Plan approval: dedicated card listing tasks (role glyph, key, title, scopes),
  `[a] approve · [e] edit in $EDITOR · [d] reject`.
- j/k or arrows select tasks in Tasks view, Enter → detail; d → diff for task.
- Escape interrupts / closes topmost overlay; q never quits from composer
  (only /quit or Ctrl+C Ctrl+C double-tap).
- All keybindings in one table (single source of truth) rendered by /help.

## Theming

Semantic token struct (bg, panel, text, dim, accent, error, warn, ok,
diff_added_bg, diff_removed_bg, border…), default dark + light themes, loaded
from optional user JSON. All widgets read tokens only — no literal colors.
