# Maestro — The Engineering Guild

A full-screen pixel guild built directly in TypeScript on a local clone of [bas3line/agent-office](https://github.com/bas3line/agent-office). No GitHub fork was created. Five separate workspaces surround walking lanes, with four smaller characters, distinct work animations, and ten interactive tools.

**UI-first prototype:** the interface, tool interactions, shared record, bounded decisions, and animations work locally. Agent messages, patches, check results, and review findings are illustrative. There is no connected LLM, autonomous repository editing, test runner, or merge operation. This is not a claim of meeting the problem statement’s real multi-agent execution requirement.

## Run

Node 22+ is recommended; upstream CI uses Node 24.

```sh
npm ci
npm run dev
```

Open **http://127.0.0.1:5173**. The preview serves static files on loopback and does not start the upstream transcript readers or agent services. After edits, run `npm run build` and refresh. Set `MAESTRO_PORT=5174` to change the port.

```sh
npm run build
npm run typecheck
npm run test:guild  # 36 checks of routing, camera, auto-follow, animation and campaign state
npm test            # Full inherited suite; see known baseline in validation notes
```

## Five workspaces

| Workspace | Character and work animation | Interactive tools |
| --- | --- | --- |
| Shogun’s Council | Elder in a straw hat; unrolls a scroll and stamps criteria | Strategy table; acceptance scroll |
| Samurai Forge | Stocky blacksmith with apron and hammer; strikes the anvil and quenches the workpiece | Forge anvil; quench barrel |
| Sentinel Dojo | Compact robot with a visor and antenna; extends a scanner and signals results | Test console; inspection scanner |
| Shadow Archive | Hooded archivist; reads, turns pages and annotates | Review desk; evidence archive |
| Living Campaign | Shared coordination area visited by the existing agents | Shared mission board; autonomy console |

Each character has a separate sprite construction, walk speed, work cadence, and route between its own tools. Sprites range from 38 to 44 world pixels high, with individual foot baselines so each character meets its ground shadow. Canonical station footprints have at least 24 world pixels of clearance. The dojo and archive occupy separate rows.

Click a physical tool or focus it with Tab and press Enter/Space. The anvil, barrel, scanner and acceptance scroll open compact actions beside the object. Plans, test evidence, review findings and campaign decisions open a detail panel. Closing either returns focus to its tool; Escape dismisses it. Click a character to inspect its role.

The main canvas fills the viewport. Mission, guild, activity, settings and campaign controls live in the bottom toolbar. Panels overlay the scene only when opened. The fullscreen control also supports browser fullscreen where available. Light mode and reduced-motion preferences keep the scene still; dark mode permits role-specific animation. The scenery pause and mission playback controls are separate.

## Operational pages

The main guild scene is preserved. Mission, Guild, Campaign, Evidence and Settings share a separate workspace navigation and a readable Geist type scale. Their layouts are tuned for laptops, including shorter 1366 × 768 screens.

- **Mission:** the request, current stage, acceptance criteria and role assignments.
- **Guild:** four distinct pixel portraits, role descriptions, planned tools and profile dialogs.
- **Campaign:** the current decision, retry budget, shared messages, clarification form and continue/retry/stop controls. Open it from another detail page, or select **Open campaign dashboard** in the scene's Campaign workspace.
- **Evidence:** sample results, an interactive event/findings filter, acceptance status and the approval dialog.
- **Settings:** theme, scenery motion, iteration budget, connection status and reset controls.

Activity, physical-tool workspaces, new-mission forms, profiles and approval dialogs use the same typography and control treatment. Short page entrances and role-card lighting follow the existing dark-mode, pause and reduced-motion preferences. Settings and shared state remain local to the current page session.

Motion references: [React Bits Animated Content](https://reactbits.dev/animations/animated-content) and [Spotlight Card](https://reactbits.dev/components/spotlight-card). These patterns are implemented directly in the existing TypeScript/CSS app; React, GSAP and copied React Bits source were not added.

## Six visual improvements

1. **Composed world:** an octagonal courtyard, connected stone paths, koi pond, raked garden, bonsai, flower beds, benches, forest and entrance terrace. The five workshop footprints remain separated.
2. **Depth and light:** consistent shadows, layered materials, warm furnace spill and reflected metal, teal inspection light, violet archive light and lantern pools.
3. **Directional characters:** individually drawn front, side and rear views. Characters turn before moving, pause at path corners, think on arrival, then use their own tools at distinct cadences.
4. **Scene navigation:** **Auto-follow** starts enabled and zooms into the station doing the work. It follows mission stages, frames the courtyard during handoffs, moves to Campaign for holds or approval, and returns to the whole guild on completion. Select a workspace, use `+` / `−`, scroll, drag or pinch to take manual control. **Fit guild** restores the complete view; **Auto-follow** resumes tracking the latest work. With the scene focused, arrow keys pan, `+` / `−` zoom, and `0` fits the guild. The camera persists through panel changes and applies to both scene layers.
5. **Actions in the world:** cool a workpiece, select forge work, inspect a scripted scanner finding, or open criteria from the physical object. Handoffs update the shared record. Substantive information lives in panels.
6. **Consistent controls:** shared gold, ember, teal, violet and blue role colours; readable interface typography; a compact workspace navigator and mission toolbar; responsive controls and visible keyboard focus.

The 1160 × 650 landscape campus spreads the archive, council and campaign across the upper row, with the forge and dojo below. Front steps join the two promenades and central handoff courtyard. Wide screens place the workspace selector in the header to leave more room for the scene.

Automatic station focus works continuously on desktop and mobile, including after resizing. Camera transitions ease between stations only while scenery motion is allowed in dark mode. Light mode, scenery pause and reduced motion reframe immediately without animation. Manual exploration stays in control through subsequent mission changes until **Auto-follow** is enabled again.

The compact layout uses a horizontal workspace selector. **Fit guild** remains available for the complete overview. Focusing a room enlarges its physical tools on narrow screens. Touch gesture handlers are implemented and exercised with synthetic pointer events; a physical-device touch test remains outstanding.

## The Living Campaign

The coordination workspace previews requirement E with observable local state:

- **Shared record:** findings, replies, criteria and review notes survive UI rerenders and are included in the exported demo log. They reset on reload or a fresh mission.
- **Feedback:** sending a sample failure from Sentinel or critique from Shadow records the handoff, revises the plan to repair, consumes a retry, and routes both participants to separate pads in the shared courtyard. They wait until both have arrived before returning to their tools.
- **Continue / retry / clarify / stop:** the autonomy console controls the next UI decision. Clarification requires a recorded response before continuation. Stop requires explicit continuation.
- **Bounded retries:** the iteration budget stops further repairs at the limit. Increasing the budget and explicitly continuing can release that hold.
- **Unavailable tools:** the first simulated failure revises the approach; a repeated failure pauses for human input.
- **Human oversight:** the sample mission stops at approval. The demo cannot skip its approval gate or apply a patch.

These are real frontend state transitions around a scripted example, **not communication between autonomous LLM agents**. The interface labels its simulated checks and evidence accordingly.

## Try a feedback loop

1. Click the **Test console** in Sentinel’s workspace.
2. Choose **Preview sample checks**. The scripted whitespace case fails.
3. Choose **Send failure to Samurai**. The mission changes to repair and consumes an iteration.
4. Open **Campaign** to read the shared messages and revised decision.
5. Select **Autonomy controls**. Try another repair, exhaust the budget, request clarification, or stop.
6. Use **Next step** or **Play demo** to continue the sample mission through verification, review and its approval gate.

Changing a tool while an agent is walking replans from its current point on the path. Routes enter rooms through their front doorways and go around furniture. Invalid origins stop instead of producing a straight-line shortcut. The selected agent’s planned route is shown as a subtle dashed line.

The other tool panels support pinning criteria, marking reviewed criteria, sending a sample patch for verification, and recording a review note. They do not execute commands against a repository.

## Source layout

- `web/guild/app.ts`: controls, tool panels, forms, views and approval UI.
- `web/guild/detail-pages.ts`: operational pages, page navigation, evidence filtering and campaign controls.
- `web/guild/detail-pages.css`: scoped detail-page, dialog and workspace styling; laptop density and motion preferences.
- `web/guild/layout.ts`: workspace footprints, tool locations, work positions and shared walking corridors.
- `web/guild/characters.ts`: four independent sprites and animation profiles.
- `web/guild/scene.ts`: persistent scene, route interruptions, synchronized handoffs and ground-depth ordering of agents and props.
- `web/guild/routes.ts`: shared walkway graph, shortest paths from the current position and solid ground footprints. The same outdoor edges draw the visible paving.
- `web/guild/workspaces.ts`: room floors and separately ordered walls, roofs, furniture and tools.
- `web/guild/world.ts`: original scenery and five workspaces, drawn directly in SVG.
- `web/guild/materials.ts`: pixel materials, palette and banded lighting.
- `web/guild/gardens.ts`: hand-placed courtyard, pond, gardens and foreground.
- `web/guild/camera.ts`: viewport geometry, focus, limits and pointer-anchored zoom.
- `web/guild/follow-camera.ts`: working-station selection, eased camera transitions and explicit manual takeover.
- `web/guild/navigation.ts`: persistent camera controls and mouse, touch and keyboard handlers.
- `web/guild/cockpit.css`: scene presentation, role tokens and responsive controls.
- `web/guild/mission.ts`: scripted mission stages and evidence disclosure.
- `web/guild/campaign.ts`: shared messages, retry bounds, escalation and human holds.
- `scripts/preview-guild.mjs`: static development server.
- `test/web.guild*.test.mjs`: mission, scene and campaign checks.

This revision was designed and implemented directly in code. Previous Figma concepts are not the current source of truth. Fonts are bundled under `assets/fonts` with their OFL licenses.

## Engine work still required

The intended engine needs real concurrent or event-driven agents, shared persistent state, model-selected tools, isolated repository changes, test evidence, independent verification, security review and a human approval boundary. It must enforce paths and command permissions below the model layer. The current frontend has no command-execution or repository-access API, so it should not be treated as a sandbox implementation.

The original `src/ingest`, `src/control`, `src/server`, and `src/remote` code remains preserved. The UI does not call those APIs. Package names and CLI aliases remain upstream’s; nothing has been published or deployed.

See `docs/MAESTRO_VALIDATION.md` for actual development checks and limitations. The original MIT license remains in `LICENSE`; the upstream README is preserved at `docs/UPSTREAM_README.md`.
