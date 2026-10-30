# Combined repository verification — 30 October 2026

- Imported the current Rust backend, Python workers, tests, documentation and lockfile into `backend/`; backend runtime source is unchanged by the relocation.
- From the combined repository: `npm run backend:test` passed all 410 Rust tests; `npm run backend:build` produced the release executable.
- Type checking, the frontend build, all 40 routing/guild tests, seven LangGraph tests and two browser-worker tests passed.
- A disposable-project startup check removed `MAESTRO_BINARY` from its test environment and confirmed that the bridge started this checkout's `backend/target/release/maestro`, then read authenticated project state and shut down only that test daemon. This check made no model calls.
- Build products, local state, Python caches and environment files are ignored. The imported text scan found no credentials. Historical real-model evidence below was retained; it was not rerun for this source relocation.

# Multi-project workspace verification — 30 October 2026

- Read-only T3 schema inspection confirmed the project-folder → thread/session hierarchy.
- A real browser test opened two repositories and submitted three chats across independent sessions. To exercise response ordering deterministically, the browser held real model responses until all submissions existed, then released them. Every response matched its originating repository; drafts and selected sessions survived switching and reload.
- The full live mission regression passed: fixed guild planning/execution, both independent reviews, diff approval, named backup, undo and image chat.
- Two new routing/registry tests cover canonical folder persistence, new Git folder initialization, concurrent project routing, authentication isolation, scoped images/SSE and invalid-project rejection. All 38 existing guild tests, typecheck and production build pass.
- Browser checks covered 1440px, 1366px and 390px widths, collapsed/expanded floating input, light mode, folder browsing and connection controls. The empty mission banner is absent for ordinary chat.
- Pre-push full suite: 533 tests, 528 passed, 2 skipped, 3 failed. The same three legacy agent-installation tests also fail on unchanged baseline `0e9fde9` on this machine because their test PATH discovers installed Gemini, Claude and Codex CLIs. The new routing tests and guild tests pass.

Receipts: [multi-project chats](evidence/multi-project-live-2026-10-30.json), [mission regression](evidence/multi-project-mission-regression.json), [pending sessions](evidence/multi-project-pending.png), [desktop](evidence/workspace-desktop.png), [mobile composer](evidence/workspace-mobile-composer.png).

Manual reproducible checks: `scripts/verify-maestro-projects.py` and `scripts/verify-maestro-live.py`, run with the Maestro browser-worker Python and configured model credentials. Test repositories are disposable; user repositories are not modified by these checks.

# Live integration update

The default entry point now uses the Rust daemon integration. The sections below describe historical prototype validation; their simulated in-app outcomes are not used by the live client. See [DAEMON_INTEGRATION.md](DAEMON_INTEGRATION.md) and the live evidence files for current behavior.

Verified current behavior (real Azure Foundry requests, no scripted model results):

- Typecheck, production build and all 38 scene/UI unit tests passed.
- A live Shogun plan executed Samurai, Sentinel and Shadow Council tasks; both independent gates passed and the website displayed the real integration diff.
- Approving that diff wrote the requested file in a disposable repository. Previewing and applying Undo restored the original code.
- A named backup was saved through the UI. Uploaded image pixels were identified correctly by the live model, and persisted attachments loaded from the daemon.
- Automatic daemon startup, mutation-token rejection, draft persistence and 1366px/390px layouts passed browser checks without page errors.
- Live TUI guild labels, image attachment and preview commands passed a PTY check. Required-sandbox Next.js/Turbopack preview returned HTTP 200 and a Chromium capture.

Evidence: [live run receipt](evidence/live-daemon-2026-10-30.json), [tasks](evidence/live-tasks.png), [image chat](evidence/live-image-chat.png), [mobile](evidence/live-mobile.png). These checks do not establish all PS evaluation scenarios; see the integration guide for remaining engine gaps.

# Historical scripted UI validation — 30 October 2026

These results come from actual development checks on the UI. The patch, agent conversations and test results shown *inside the demo* remain scripted examples. There is no connected autonomous agent engine.

## Maestro branding and daylight repair

Renamed product branding, page titles, accessibility labels, the M emblem, export identifiers, download filenames, preview environment variable (`MAESTRO_PORT`) and documentation. Original upstream attribution is retained.

Fixed the daylight dock's conflicting dark background, pale warning tags and role labels, character nameplates, speech bubbles, tool tips and workspace labels. Daylight now has brighter greens, pink blossom, clearer water and a blue sky. Dark-mode artwork is preserved.

Changing to daylight or reduced motion now completes active journeys at the requested tool with the correct facing and a static working pose. Still-mode handoffs complete without waiting for animation frames. Explicit dark-mode pause keeps the actor's current position and route. Motion remains dark-mode-only.

Executed checks for this revision:

- Typecheck and production build passed.
- **38 repository UI tests passed**, including new interrupted-travel and immediate tool-action regressions.
- Existing mounted application checks passed **11 + 8 groups**. The movement check passed **1,172 frames** and camera-follow check passed **629 frames**.
- A new mounted scene check passed **6 daylight regression groups**: mid-route theme change, direct tool selection, manual/phase-triggered handoff completion, stable still markup, dark-mode pause/resume and reduced motion. See `light-mode-checks.json`.
- Two browser visual passes covered **1366 × 768** and **1440 × 900** laptops. All five detail pages were inspected in light mode, plus the full scene in both themes and dark Guild profiles. No horizontal overflow was observed on the sampled pages. Quench selection and the test-to-repair feedback loop were exercised in daylight. The sampled repair warning uses #983f3a on #f6e6e2 (5.58:1 contrast). See `maestro-visual-review.json` and Maestro laptop screenshots.
- A source search found no remaining old product-name references; the current source archive and exports use Maestro.

These are development tests and browser observations, not evidence of a real multi-agent mission. The historical sections below retain their original check counts.

## Operational-page redesign

Mission, Guild, Campaign, Evidence and Settings were rebuilt with shared navigation, a sans-serif type hierarchy, role portraits, clearer decisions and consistent form controls. Activity, tool workspaces and modal dialogs were also restyled. The closed main guild page and its camera, paths and artwork were preserved.

The build, typecheck and **36 repository UI tests** passed. The existing **11-group application integration check** passed again. A new mounted application check passed **8 groups**, covering all five page routes and active navigation, theme/pause/reduced-motion settings, evidence filtering and its empty state, feedback and retry consumption, clarification and stop holds, repeated tool failures, dialogs and the approval guard, and preservation of the main scene and Auto-follow. These are actual DOM interaction checks; see `detail-interaction-checks.json`.

Two visual review passes were performed. The first reviewed the five pages and identified a title-focus outline, inherited form colours and spacing issues. The second focused on laptops, following the user's clarification: **1366 × 768** and **1440 × 900**. Short-laptop styling reduces excess header/card spacing without reducing the body text scale. The observed pages had no horizontal page overflow; longer settings and evidence pages scroll vertically. Dark and light themes were inspected. Browser observations and captures are recorded in `detail-visual-review.json` and the laptop screenshots.

The browser's full-page screenshot mode sometimes produced repeated stitched content during the first pass. Final laptop captures use viewport screenshots. Screenshot stitching is not treated as evidence of duplicate application content.

React Bits' Animated Content and Spotlight Card were consulted as motion references. The implementation uses the existing TypeScript/CSS stack, without copying their source or adding React/GSAP. No Animos site is claimed as used; its exact URL was not provided.

## Automatic station focus revision

- `npm run typecheck`: passed.
- `npm run build`: passed.
- `npm run test:guild`: **36 passed, 0 failed, 0 cancelled**.
- `git diff --check`: passed.

The checks cover mission approval guards, honest evidence export, repair feedback, iteration limits, clarification and stop holds, repeated tool failures, five separated station footprints, tool bounds, collision-free shared corridors, distinct character cadences, movement without overshoot and dark-only animation permissions.

Six additional checks cover: fitting all five rooms on wide/narrow viewports; focused rooms remaining fully visible; repeated zoom/pan bounds; keeping a pointer's world location steady during zoom; turning before walking; and distinct front/side/rear sprite drawings for every role.

Six auto-follow checks cover phase ownership; handoff, hold and selected-tool targets; smooth transitions without restarting each frame; motion-disabled behavior; manual control until explicit resumption; and retargeting during a transition or resize.

## Automatic station focus integration

The actual scene and camera controls ran through **629 frames** in a separate mounted DOM check. All six groups passed: Forge → Dojo tracking; courtyard framing and return to Forge after the physical handoff; manual Fit guild staying in control across phase changes and explicit Auto-follow resumption; immediate, still framing in light/pause/reduced-motion modes; selected tools, holds, approval and completion; and mobile resize. Both SVG layers had identical viewBoxes at every sampled frame. The report is `auto-follow-checks.json` in the output package.

This verifies the scene's real integration with the camera controller, but does not substitute for browser rendering or a physical-device test.

## Mounted UI integration

A separate LinkeDOM harness ran the actual application and its event handlers. **All 11 groups passed**, covering:

- Activity, mission, guild and settings navigation.
- Light/dark mode and manual scenery pause.
- Scripted failed-check → repair progression and retry consumption.
- Persistence of scene and camera through panel changes.
- All ten physical tools: four compact in-scene actions and six detail workspaces.
- Zoom, workspace focus, aligned background/character viewports and Fit guild.
- Synthetic drag, wheel, two-pointer pinch and keyboard reset.
- Opening scripted evidence from the scanner's compact action.
- Campaign holds, clarification and explicit continuation.

This is a DOM integration run, **not a browser rendering test or physical-device touch test**. Its report is `ui-interaction-checks.json` in the output package. Camera geometry is additionally tested across multiple aspect ratios by the repository tests.

## Movement and depth regression checks

Seven new repository tests cover all **100 ordered tool-to-tool journeys**, mid-edge interruptions, required doorway passage, distinct courtyard handoff destinations, invalid-origin guards, visible paths matching the navigation graph, and role-specific foot contact.

A separate mounted-scene integration run advanced the real scene controller through **1,172 frames** with a controlled animation clock. Every sampled position remained on a walkway and clear of the defined solid footprints. The renderer kept characters and objects in ascending ground-depth order. Mid-route reassignment preserved the character's position. Both handoff participants physically reached their separate courtyard pads before receiving return routes. Switching to daylight kept the scene markup still. The report is `movement-checks.json` in the output package. This is an executed DOM animation test, not a browser video or an autonomous agent mission.

## Browser review

The updated desktop scene was visually inspected at **1280 × 720**, and the mobile view at **390 × 844**. Document dimensions matched the viewport with no page overflow at either size. Desktop and mobile screenshots were captured from the browser.

The Autonomy controls and shared mission board opened in the browser. Observed actor metadata showed Shogun using the north promenade (`y=286`) and front-door waypoints, then a safe return route to Council after changing tools. The browser's observed object/character order followed their ground depth, and the two SVG layers shared the same camera viewBox.

Automatic station focus was exercised in the browser through building, testing, repair, verification, review and approval. The view followed Forge → Dojo → courtyard → Forge, then Archive and Campaign in later stages. Fit guild disabled automatic tracking; advancing the mission preserved that manual view; enabling Auto-follow resumed the latest target. DOM observations captured in-progress transitions with identical viewBoxes on both SVG layers. Paused and light-mode observations showed still camera state. These observations are recorded in `auto-follow-browser.json`.

Mobile opens the active workspace at a readable scale and continues following mission changes. The Forge view was inspected at 300% zoom, with its selector highlighted. The new Auto-follow control fits the mobile toolbar. After correcting a spacing overlap, the camera controls and bottom dock had a 13-pixel gap at 390 × 844; the scene footnote had an 8-pixel gap above the camera controls. Fit guild remains available.

Earlier browser automation had intermittent pre-dispatch timeouts; these current desktop/mobile observations and screenshots succeeded. Physical-device pinch and an OS-level reduced-motion session remain untested. Gesture handlers and dark-only animation permissions have automated coverage.

## Inherited suite baseline

An earlier `npm test` run on Node 22.22.2 reported 504 tests: 479 passed, 23 cancelled, 2 skipped, 0 assertion failures. It was **not a clean full-suite pass**. Inherited `control.cloud.places.test.mjs` and `remote.manager.test.mjs` suites cancelled with “Promise resolution is still pending but the event loop has already resolved.” Those backend sources and tests were not modified. Upstream CI uses Node 24. The full inherited suite was not rerun for this UI revision.

## Scope

Designed and implemented directly in code; Figma was not used for this revision. The original repository was cloned, not forked. Nothing was merged, published or deployed. Real model coordination, isolated code changes, test execution, independent verification and security review remain future engine work.
