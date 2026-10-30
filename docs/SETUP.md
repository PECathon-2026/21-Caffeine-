# Install the complete Maestro workspace

This repository contains the website, local bridge, Rust daemon/TUI, and Python workers. Linux is the supported execution platform. Install Node.js 24+, npm, a current stable Rust toolchain, Python 3.11+ with `venv` and `pip`, Git, and Bubblewrap (`bwrap`) for sandboxed tools. Browser inspection also needs Chromium and its system libraries. Local validation used Rust 1.97.1 and Python 3.14.6.

## Clone and build

```sh
git clone https://github.com/PECathon-2026/21-Caffeine-.git
cd 21-Caffeine-
npm ci
npm run backend:build
npm run workers:install
```

The daemon executable is `backend/target/release/maestro`. The worker installer creates `~/.local/share/maestro/langgraph-venv` and installs the pinned LangGraph worker. The browser installer uses system Chromium when available, otherwise downloads Playwright Chromium. Minimal Linux installations also need Playwright's required system libraries.

## Configure Azure deployments

For a new setup, copy the template without replacing an existing configuration:

```sh
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/maestro"
cp -n backend/examples/foundry.toml "${XDG_CONFIG_HOME:-$HOME/.config}/maestro/maestro.toml"
```

If you set `MAESTRO_CONFIG_DIR`, use that directory instead. Edit the configuration to replace every `YOUR-RESOURCE` endpoint and confirm deployment names and quotas against your Azure account. Keep `guildRoles = true` and `finalApproval = true`. The template fixes Shogun/Shadow to Astra and Samurai/Sentinel to GPT 6.1 Sol, with explicit failure fallbacks. It does not dynamically choose models by task complexity.

Supply the key through `MAESTRO_FOUNDRY_API_KEY` in the environment launching the bridge. Do not put it in this repository or browser code. Existing users can keep their working configuration and credential setup.

Enable the installed worker in that configuration:

```sh
python3 backend/scripts/install-langgraph.py --configure --skip-dependencies
```

This updates the existing runtime section with the installed worker's absolute command. The machine-specific configuration stays outside Git. A project's own `maestro.toml` **replaces** global configuration completely; it is not a partial overlay. Add real project checks before using a mission as evidence of tested correctness. The generic template has `checks = []` because check commands depend on the project.

## Start the workspace

```sh
MAESTRO_PROJECT=/absolute/path/to/your/git-project npm run dev
```

Open **http://127.0.0.1:5173**. The bridge uses `backend/target/release/maestro` when built, otherwise `maestro` on PATH. `MAESTRO_BINARY=/absolute/path/to/maestro` overrides either choice. Set `MAESTRO_PORT` to change the UI port. With a custom Cargo target directory, supply its binary path explicitly.

Use **Open project** for another repository and **＋** for another session. Existing repository daemons are reused. After updating the backend or runtime configuration, restart a repository daemon when its work and previews can be interrupted. Starting the website does not upgrade an already running daemon.

For the terminal UI:

```sh
./backend/target/release/maestro --cwd /absolute/path/to/your/git-project
```

The bridge binds to localhost. A static hosted website cannot reach a user's filesystem or start their local daemon by itself.

## Repository map

| Path | Purpose |
| --- | --- |
| `web/guild/` | Live project/session UI and pixel world |
| `scripts/preview-guild.mjs` | Local HTTP/SSE bridge |
| `scripts/maestro-projects.mjs` | Persistent project registry |
| `backend/crates/` | Seven Rust crates: types, store, providers, engine, daemon, TUI, CLI |
| `backend/workers/` | LangGraph and browser workers with tests |
| `backend/scripts/` | Worker installers and historical live smoke script |
| `backend/examples/` | Credential-free configuration template |
| `backend/docs/` | Architecture, history, tools, and recorded evidence |

The root `src/` contains the inherited Agent Office Node CLI and supporting code; the live Maestro backend is in `backend/`. Root npm publishing metadata still refers to the upstream package and is not a Maestro backend distribution mechanism.

## Validate

```sh
npm run typecheck
npm run build
npm run backend:test
node --test test/maestro.projects.test.mjs test/web.guild*.test.mjs
~/.local/share/maestro/langgraph-venv/bin/python -m unittest discover -s backend/workers/langgraph -p 'test_*.py'
~/.local/share/maestro/langgraph-venv/bin/python -m unittest discover -s backend/workers/browser -p 'test_*.py'
```

GitHub CI checks the frontend, Rust workspace/production build, and Python workers separately. Deterministic tests require no provider key. The manual `scripts/verify-maestro-live.py` and `scripts/verify-maestro-projects.py` checks exercise real models in disposable repositories; they require credentials and incur provider usage.

See [integration](DAEMON_INTEGRATION.md) for feature coverage and limits, and [backend architecture](../backend/docs/architecture.md) for execution details. Backend source was imported from the current local `maestro-rs` working tree, including previously uncommitted fixes. This repository is now the combined source location.
