//! Load, save, and initialize `maestro.toml`, plus the `.maestro` state dir
//! and the safety-profile → policy-rule expansion.

use maestro_types::config::{MaestroConfig, PolicyAction, PolicyEffect, PolicyRule};
use maestro_types::{MaestroError, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// File name of the project configuration at the repository root.
pub const CONFIG_FILE: &str = "maestro.toml";

/// Name of the state directory under the project root.
pub const STATE_DIR: &str = ".maestro";

/// Absolute path of the config file for a project root.
pub fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_FILE)
}

/// User-level config directory: `$MAESTRO_CONFIG_DIR` (tests, overrides) >
/// `$XDG_CONFIG_HOME/maestro` > `~/.config/maestro`. `None` when no home can
/// be resolved.
pub fn user_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MAESTRO_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("maestro"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("maestro"))
}

/// The user-level `maestro.toml`, when it exists on disk.
pub fn user_config_path() -> Option<PathBuf> {
    let path = user_config_dir()?.join(CONFIG_FILE);
    path.exists().then_some(path)
}

/// Load and validate `maestro.toml` from `root`.
///
/// Returns [`MaestroError::NotFound`] when the file does not exist and
/// [`MaestroError::Config`] when it fails to parse or validate.
pub fn load_config(root: &Path) -> Result<MaestroConfig> {
    let path = config_path(root);
    if !path.exists() {
        // No project file: fall back to the user-level config so providers
        // and role assignments only have to be set up once per machine. A
        // project file, when present, wins wholly — no partial merging.
        if let Some(user_path) = user_config_path() {
            return load_config_file(&user_path);
        }
        return Err(MaestroError::NotFound(format!(
            "no {CONFIG_FILE} at {} (and no user-level config)",
            root.display()
        )));
    }
    load_config_file(&path)
}

/// Where the effective config came from, for `doctor` and startup lines.
pub fn config_source(root: &Path) -> &'static str {
    if config_path(root).exists() {
        "project maestro.toml"
    } else if user_config_path().is_some() {
        "user config (~/.config/maestro)"
    } else {
        "built-in default (GPT; API key required)"
    }
}

fn load_config_file(path: &Path) -> Result<MaestroConfig> {
    let raw = fs::read_to_string(path)?;
    let config: MaestroConfig = toml::from_str(&raw)
        .map_err(|e| MaestroError::Config(format!("invalid {}: {e}", path.display())))?;
    validate_config(&config)?;
    Ok(config)
}

/// Atomically write `maestro.toml` (temp file + rename in the same directory).
pub fn save_config(root: &Path, config: &MaestroConfig) -> Result<()> {
    validate_config(config)?;
    let body = toml::to_string_pretty(config)
        .map_err(|e| MaestroError::Config(format!("cannot serialize config: {e}")))?;
    let path = config_path(root);
    let tmp = root.join(format!("{CONFIG_FILE}.tmp-{}", std::process::id()));
    fs::write(&tmp, body)?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Initialize a project: write a default `maestro.toml` (kept as-is when one
/// already exists) and make sure `.gitignore` excludes `.maestro/`.
///
/// Returns the effective configuration.
pub fn init_config(root: &Path) -> Result<MaestroConfig> {
    let config = if config_path(root).exists() {
        load_config(root)?
    } else {
        let config = MaestroConfig::default();
        save_config(root, &config)?;
        config
    };
    ensure_gitignore(root)?;
    Ok(config)
}

/// Project setup without writing `maestro.toml`: gitignore only. Used by
/// `maestro init` when a user-level config already covers the project — a
/// mock project template would only shadow it.
pub fn init_state_only(root: &Path) -> Result<()> {
    ensure_gitignore(root)
}

/// Return `root/.maestro`, creating it with mode `0700` on demand.
pub fn state_dir(root: &Path) -> Result<PathBuf> {
    let dir = root.join(STATE_DIR);
    if !dir.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
    }
    Ok(dir)
}

/// Expand a safety profile name into its base ordered policy rules.
///
/// - `"strict"`: read allow; edit, check, delegate ask.
/// - `"balanced"` (and any unknown profile): read, edit, check, delegate allow.
/// - `"permissive"`: read, edit, check, delegate allow; network and
///   external_directory keep their default deny.
pub fn profile_rules(profile: &str) -> Vec<PolicyRule> {
    fn rule(action: PolicyAction, effect: PolicyEffect) -> PolicyRule {
        PolicyRule {
            action,
            resource: "**".to_string(),
            effect,
            role: None,
        }
    }
    match profile {
        "strict" => vec![
            rule(PolicyAction::Read, PolicyEffect::Allow),
            rule(PolicyAction::Edit, PolicyEffect::Ask),
            rule(PolicyAction::Check, PolicyEffect::Ask),
            rule(PolicyAction::Delegate, PolicyEffect::Ask),
        ],
        "permissive" => vec![
            rule(PolicyAction::Read, PolicyEffect::Allow),
            rule(PolicyAction::Edit, PolicyEffect::Allow),
            rule(PolicyAction::Check, PolicyEffect::Allow),
            rule(PolicyAction::Delegate, PolicyEffect::Allow),
        ],
        // "balanced" and anything unrecognized.
        _ => vec![
            rule(PolicyAction::Read, PolicyEffect::Allow),
            rule(PolicyAction::Check, PolicyEffect::Allow),
            rule(PolicyAction::Delegate, PolicyEffect::Allow),
            rule(PolicyAction::Edit, PolicyEffect::Allow),
        ],
    }
}

/// The full ordered rule set for a config: profile rules + configured extras.
pub fn effective_rules(config: &MaestroConfig) -> Vec<PolicyRule> {
    let mut rules = profile_rules(&config.safety.profile);
    rules.extend(config.safety.rules.iter().cloned());
    rules
}

/// Structural validation beyond serde: every role assignment (primary and
/// fallbacks) must reference a configured provider.
pub fn validate_config(config: &MaestroConfig) -> Result<()> {
    if config.runtime.agent_runtime == maestro_types::config::AgentRuntime::Langgraph
        && (config.runtime.langgraph_command.is_empty()
            || config.runtime.langgraph_command[0].trim().is_empty())
    {
        return Err(MaestroError::Config(
            "LangGraph requires runtime.langgraphCommand; run scripts/install-langgraph.py".into(),
        ));
    }

    for (id, provider) in &config.providers {
        if provider.tokens_per_minute == Some(0) || provider.requests_per_minute == Some(0) {
            return Err(MaestroError::Config(format!(
                "provider '{id}' quota limits must be positive or omitted"
            )));
        }
        if provider
            .deployment
            .as_ref()
            .is_some_and(|d| d.trim().is_empty())
        {
            return Err(MaestroError::Config(format!(
                "provider '{id}' deployment must not be empty"
            )));
        }
    }
    for (role, assignment) in &config.roles {
        let mut refs = vec![&assignment.model];
        refs.extend(assignment.fallbacks.iter());
        for model_ref in refs {
            if let Some(provider) = config.providers.get(&model_ref.provider) {
                if provider
                    .deployment
                    .as_ref()
                    .is_some_and(|d| d != &model_ref.model)
                {
                    return Err(MaestroError::Config(format!(
                        "role '{role}' model '{}' does not match provider '{}' deployment",
                        model_ref.model, model_ref.provider
                    )));
                }
                if provider.tokens_per_minute.is_some()
                    && assignment.max_output_tokens.unwrap_or(0) == 0
                {
                    return Err(MaestroError::Config(format!(
                        "role '{role}' needs maxOutputTokens for provider '{}' token quota",
                        model_ref.provider
                    )));
                }
            }
            if !config.providers.contains_key(&model_ref.provider) {
                return Err(MaestroError::Config(format!(
                    "role '{role}' references unknown provider '{}'",
                    model_ref.provider
                )));
            }
        }
    }
    Ok(())
}

fn ensure_gitignore(root: &Path) -> Result<()> {
    let path = root.join(".gitignore");
    let existing = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let already = existing
        .lines()
        .any(|l| matches!(l.trim(), ".maestro/" | ".maestro"));
    if already {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(".maestro/\n");
    fs::write(&path, updated)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::config::ModelRef;

    #[test]
    fn load_missing_is_not_found_and_user_config_fills_in() {
        let _guard = crate::test_env::lock();
        let dir = tempfile::tempdir().unwrap();
        // Point the user-level dir at an empty temp dir so the developer's
        // real ~/.config/maestro can't leak into the test.
        let user_dir = tempfile::tempdir().unwrap();
        std::env::set_var("MAESTRO_CONFIG_DIR", user_dir.path());

        match load_config(dir.path()) {
            Err(MaestroError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
        assert_eq!(
            config_source(dir.path()),
            "built-in default (GPT; API key required)"
        );

        // A user-level file becomes the fallback for config-less projects…
        let mut user_cfg = MaestroConfig::default();
        user_cfg.runtime.max_concurrent_tasks = 7;
        let body = toml::to_string_pretty(&user_cfg).unwrap();
        std::fs::write(user_dir.path().join(CONFIG_FILE), body).unwrap();
        let loaded = load_config(dir.path()).unwrap();
        assert_eq!(loaded.runtime.max_concurrent_tasks, 7);
        assert_eq!(config_source(dir.path()), "user config (~/.config/maestro)");

        // …and a project file wins wholly over it.
        let mut proj_cfg = MaestroConfig::default();
        proj_cfg.runtime.max_concurrent_tasks = 2;
        save_config(dir.path(), &proj_cfg).unwrap();
        let loaded = load_config(dir.path()).unwrap();
        assert_eq!(loaded.runtime.max_concurrent_tasks, 2);
        assert_eq!(config_source(dir.path()), "project maestro.toml");

        std::env::remove_var("MAESTRO_CONFIG_DIR");
        match load_config(&dir.path().join("nope")) {
            Err(MaestroError::NotFound(_)) | Ok(_) => {}
            other => panic!("expected NotFound or user fallback, got {other:?}"),
        }
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config = MaestroConfig::default();
        save_config(dir.path(), &config).unwrap();
        let loaded = load_config(dir.path()).unwrap();
        assert_eq!(loaded.version, config.version);
        assert!(loaded.providers.contains_key("mock"));
        assert_eq!(loaded.roles.len(), config.roles.len());
        assert_eq!(loaded.safety.profile, "balanced");
    }

    #[test]
    fn init_writes_defaults_and_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        init_config(dir.path()).unwrap();
        assert!(config_path(dir.path()).exists());
        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|l| l.trim() == ".maestro/"));
        // Idempotent: second init does not duplicate the entry.
        init_config(dir.path()).unwrap();
        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(
            gitignore
                .lines()
                .filter(|l| l.trim() == ".maestro/")
                .count(),
            1
        );
    }

    #[test]
    fn gitignore_appended_not_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "target/").unwrap();
        init_config(dir.path()).unwrap();
        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("target/"));
        assert!(gitignore.contains(".maestro/"));
    }

    #[test]
    fn state_dir_created_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = state_dir(dir.path()).unwrap();
        assert!(state.is_dir());
        let mode = fs::metadata(&state).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn unknown_config_keys_fail_loudly() {
        let dir = tempfile::tempdir().unwrap();
        // A snake_case key (the config is camelCase on disk) must be a loud
        // load error, never silently ignored.
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            "version = 1\n\n[runtime]\nmax_attempts = 3\n",
        )
        .unwrap();
        match load_config(dir.path()) {
            Err(MaestroError::Config(msg)) => {
                assert!(msg.contains("unknown field"), "unhelpful error: {msg}");
                assert!(
                    msg.contains("max_attempts"),
                    "error must name the key: {msg}"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_provider_ref_rejected() {
        let mut config = MaestroConfig::default();
        if let Some(assignment) = config.roles.get_mut("coder") {
            assignment.fallbacks.push(ModelRef {
                provider: "ghost".into(),
                model: "m".into(),
            });
        }
        match validate_config(&config) {
            Err(MaestroError::Config(msg)) => assert!(msg.contains("ghost")),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn profiles_expand_as_documented() {
        let strict = profile_rules("strict");
        assert_eq!(strict.len(), 4);
        assert!(strict
            .iter()
            .any(|r| r.action == PolicyAction::Edit && r.effect == PolicyEffect::Ask));
        let balanced = profile_rules("balanced");
        assert!(balanced.iter().all(|r| r.effect == PolicyEffect::Allow));
        let permissive = profile_rules("permissive");
        assert!(!permissive.iter().any(|r| matches!(
            r.action,
            PolicyAction::Network | PolicyAction::ExternalDirectory
        )));
    }
}
