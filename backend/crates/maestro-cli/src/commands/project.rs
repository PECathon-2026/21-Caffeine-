//! Project-level commands: init, serve, models, connect, doctor.

use super::emit_json;
use crate::context::{probe_daemon, read_secret, Ctx};
use crate::output::render_table;
use maestro_providers::ProviderRegistry;
use maestro_types::config::EnforcementMode;
use maestro_types::provider::ModelDiscovery;
use maestro_types::Result;
use std::path::{Path, PathBuf};

pub async fn init(ctx: &Ctx, local: bool) -> Result<i32> {
    let config_path = maestro_engine::config::config_path(&ctx.root);
    let existed = config_path.exists();
    // With a user-level config present, a mock project template would only
    // shadow it — set up the state dir/gitignore and inherit instead.
    let inherit = !existed && !local && maestro_engine::config::user_config_path().is_some();
    if inherit {
        maestro_engine::init_state_only(&ctx.root)?;
    } else {
        maestro_engine::init_config(&ctx.root)?;
    }
    if ctx.json {
        emit_json(&serde_json::json!({
            "config": config_path,
            "created": !existed && !inherit,
            "inheritsUserConfig": inherit,
        }))?;
    } else if existed {
        println!("maestro.toml already present at {}", config_path.display());
    } else if inherit {
        println!(
            "using the user-level config (~/.config/maestro/maestro.toml);              run `maestro init --local` to write a project override"
        );
    } else {
        println!("wrote {}", config_path.display());
        println!(
            "{}",
            ctx.style
                .dim("default config uses GPT via Responses; run `maestro connect gpt` to store your API key")
        );
    }
    Ok(0)
}

pub async fn serve(ctx: &Ctx, bind: Option<String>, token: Option<String>) -> Result<i32> {
    ctx.config()?; // clear error before the daemon boots in an uninitialized dir
    maestro_daemon::run_daemon(maestro_daemon::DaemonOptions {
        root: ctx.root.clone(),
        bind,
        token,
    })
    .await?;
    Ok(0)
}

pub async fn models(ctx: &Ctx) -> Result<i32> {
    let discovery: ModelDiscovery = match probe_daemon(&ctx.root).await {
        Some(client) => client.models().await?.discovery,
        None => {
            let config = ctx.config()?;
            let creds = maestro_engine::load_credentials(&ctx.root)?;
            maestro_engine::inject_env(&config, &creds);
            ProviderRegistry::new(&config).discover_models().await
        }
    };
    if ctx.json {
        emit_json(&serde_json::json!({ "discovery": discovery }))?;
        return Ok(0);
    }
    if discovery.models.is_empty() {
        println!("{}", ctx.style.dim("no models"));
    } else {
        let rows: Vec<Vec<String>> = discovery
            .models
            .iter()
            .map(|m| {
                vec![
                    m.provider_id.clone(),
                    m.model.clone(),
                    if m.discovered {
                        "yes".into()
                    } else {
                        "no".into()
                    },
                    m.context_window
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();
        print!(
            "{}",
            render_table(
                &["PROVIDER", "MODEL", "DISCOVERED", "CONTEXT"],
                &rows,
                &ctx.style
            )
        );
    }
    for (provider, error) in &discovery.errors {
        eprintln!(
            "{}",
            ctx.style
                .dim(&format!("{provider}: discovery failed: {error}"))
        );
    }
    Ok(0)
}

pub async fn connect(ctx: &Ctx, provider_id: &str, global: bool) -> Result<i32> {
    if let Ok(config) = ctx.config() {
        if !config.providers.contains_key(provider_id) {
            eprintln!(
                "warning: provider '{provider_id}' is not configured in maestro.toml; \
                 storing the credential anyway"
            );
        }
    }
    let secret = read_secret(&format!("API key for {provider_id}: "))?;
    let location = if global {
        maestro_engine::save_global_credential(provider_id, &secret)?;
        "~/.config/maestro/credentials.json"
    } else {
        maestro_engine::save_credential(&ctx.root, provider_id, &secret)?;
        ".maestro/credentials.json"
    };
    if ctx.json {
        emit_json(&serde_json::json!({
            "providerId": provider_id,
            "stored": true,
            "global": global,
        }))?;
    } else {
        println!("credential for {provider_id} stored in {location} (0600)");
    }
    Ok(0)
}

// ---- doctor -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn as_str(&self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }
}

struct Check {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

fn check(name: &'static str, status: CheckStatus, detail: impl Into<String>) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
    }
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
                continue;
            }
            #[cfg(not(unix))]
            return Some(candidate);
        }
    }
    None
}

fn command_version(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().unwrap_or("").trim().to_string())
}

fn credentials_mode_ok(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().ok().map(|m| m.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

pub async fn doctor(ctx: &Ctx) -> Result<i32> {
    let mut checks: Vec<Check> = Vec::new();

    // git
    match command_version("git") {
        Some(version) => checks.push(check("git", CheckStatus::Ok, version)),
        None => checks.push(check("git", CheckStatus::Fail, "git not found in PATH")),
    }

    // config
    let config = match ctx.config() {
        Ok(config) => {
            let sandbox = match config.safety.sandbox {
                EnforcementMode::Required => "required",
                EnforcementMode::Preferred => "preferred",
            };
            checks.push(check(
                "config",
                CheckStatus::Ok,
                format!(
                    "{} valid (profile {}, sandbox {sandbox})",
                    maestro_engine::config::config_source(&ctx.root),
                    config.safety.profile
                ),
            ));
            Some(config)
        }
        Err(e) => {
            checks.push(check("config", CheckStatus::Fail, e.to_string()));
            None
        }
    };

    // bubblewrap sandbox
    match find_in_path("bwrap") {
        Some(path) => checks.push(check(
            "sandbox",
            CheckStatus::Ok,
            format!("bwrap at {}", path.display()),
        )),
        None => {
            let required = config
                .as_ref()
                .map(|c| c.safety.sandbox == EnforcementMode::Required)
                .unwrap_or(false);
            let (status, detail) = if required {
                (
                    CheckStatus::Fail,
                    "bwrap not found and safety.sandbox = required (checks will fail closed)",
                )
            } else {
                (
                    CheckStatus::Warn,
                    "bwrap not found; sandbox = preferred falls back to host execution",
                )
            };
            checks.push(check("sandbox", status, detail));
        }
    }

    // database integrity
    let db_path = ctx.root.join(".maestro").join("maestro.db");
    if db_path.exists() {
        match maestro_store::Store::open(&ctx.root.join(".maestro")) {
            Ok(store) => match store.integrity_check() {
                Ok(true) => checks.push(check("database", CheckStatus::Ok, "integrity_check ok")),
                Ok(false) => checks.push(check(
                    "database",
                    CheckStatus::Fail,
                    "integrity_check reported corruption",
                )),
                Err(e) => checks.push(check("database", CheckStatus::Fail, e.to_string())),
            },
            Err(e) => checks.push(check("database", CheckStatus::Fail, e.to_string())),
        }
    } else {
        checks.push(check(
            "database",
            CheckStatus::Ok,
            "no database yet (created on first mission)",
        ));
    }

    // daemon liveness (informational)
    match probe_daemon(&ctx.root).await {
        Some(client) => checks.push(check(
            "daemon",
            CheckStatus::Ok,
            format!("running at {}", client.base_url()),
        )),
        None => checks.push(check("daemon", CheckStatus::Ok, "not running")),
    }

    // credentials file permissions
    let creds_path = maestro_engine::credentials::credentials_path(&ctx.root);
    if creds_path.exists() {
        match credentials_mode_ok(&creds_path) {
            Some(0o600) => checks.push(check(
                "credentials",
                CheckStatus::Ok,
                "credentials.json mode 0600",
            )),
            Some(mode) => checks.push(check(
                "credentials",
                CheckStatus::Fail,
                format!("credentials.json mode {mode:03o}, expected 0600 (fix: chmod 600)"),
            )),
            None => checks.push(check(
                "credentials",
                CheckStatus::Warn,
                "cannot inspect file mode",
            )),
        }
    } else {
        checks.push(check("credentials", CheckStatus::Ok, "none stored"));
    }

    let failed = checks.iter().any(|c| c.status == CheckStatus::Fail);
    if ctx.json {
        let list: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "status": c.status.as_str(),
                    "detail": c.detail,
                })
            })
            .collect();
        emit_json(&serde_json::json!({ "ok": !failed, "checks": list }))?;
    } else {
        for c in &checks {
            let glyph = match c.status {
                CheckStatus::Ok => ctx.style.green("✓"),
                CheckStatus::Warn => ctx.style.bold("!"),
                CheckStatus::Fail => ctx.style.red("✗"),
            };
            println!("{glyph} {:<12} {}", c.name, ctx.style.dim(&c.detail));
        }
    }
    Ok(if failed { 1 } else { 0 })
}
