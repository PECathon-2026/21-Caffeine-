# Maestro — The Engineering Guild

A live pixel-art control room for the Rust Maestro daemon. **Shogun plans, Samurai builds, Sentinel tests, and Shadow Council reviews.** The default application now displays real missions, tasks, tool events, approvals and code history across multiple local repositories and sessions.

```sh
npm ci
MAESTRO_PROJECT=/path/to/your/project MAESTRO_BINARY=/path/to/maestro npm run dev
```

Open **http://127.0.0.1:5173**. Use **Open project** to add folders and **＋** beside a project to create sessions. The local bridge connects to or starts a separate Maestro daemon for each repository. Switching sessions preserves background work and drafts. Provider credentials and daemon authentication stay server-side. The Rust backend, LangGraph worker and browser worker must be installed separately.

The compact footer expands on focus; secondary controls stay out of the way. The website includes persistent chat and sessions, mission controls, a live agent roster, full task details, plan/patch approval, evidence inspection, named backups, previewed undo/restore, image attachments and local preview links. The model assignments stay fixed. Enable `guildRoles = true` under `[runtime]` in the Maestro configuration for the four PS-001 contracts and both review gates.

See [daemon integration and the TUI feature comparison](docs/DAEMON_INTEGRATION.md) for architecture, setup, presentation steps and remaining limits.

```sh
npm run typecheck
npm run build
npm run test:guild
node --test test/maestro.projects.test.mjs
```

The original guild artwork, navigation and scene implementation are preserved. The historical scripted prototype remains documented in [UI_PROTOTYPE.md](docs/UI_PROTOTYPE.md); its simulated results are not used as live evidence. This project builds on [bas3line/agent-office](https://github.com/bas3line/agent-office), under the original [MIT license](LICENSE). Nothing has been published or deployed.

A manual live integration check is available in `scripts/verify-maestro-live.py`. Run it with the Maestro browser-worker Python after building; it uses your configured model providers and creates a disposable repository for approval and undo checks.
