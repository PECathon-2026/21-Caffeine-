//! Local fallbacks for operations the daemon API does not expose:
//! role→model assignment (edits `maestro.toml`) and credential storage
//! (`.maestro/credentials.json`, dir 0700 / file 0600, atomic write) —
//! mirroring the TS behavior described in SPEC §Config and credentials.

use maestro_types::{MaestroError, Result};
use std::path::Path;

/// Bounded tail of the detached daemon's stderr, for a failed startup probe.
pub fn daemon_startup_error(root: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let path = root.join(".maestro/daemon.stderr.log");
    let detail = (|| -> std::io::Result<Option<String>> {
        let mut file = std::fs::File::open(&path)?;
        let start = file.metadata()?.len().saturating_sub(16_384);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(16_384).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("error: "))
            .map(|line| {
                line.chars()
                    .filter(|c| !c.is_control())
                    .take(2_000)
                    .collect()
            }))
    })()
    .ok()
    .flatten();
    match detail {
        Some(detail) => format!("daemon failed to start: {detail}"),
        None => format!("daemon did not become ready; see {}", path.display()),
    }
}

/// User-level `maestro.toml` (mirror of `maestro_engine::config`, which the
/// TUI deliberately does not link): `$MAESTRO_CONFIG_DIR` > `$XDG_CONFIG_HOME`
/// > `~/.config`, each under `maestro/`.
fn user_config_path() -> Option<std::path::PathBuf> {
    let dir = if let Some(dir) = std::env::var_os("MAESTRO_CONFIG_DIR") {
        std::path::PathBuf::from(dir)
    } else if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(xdg).join("maestro")
    } else {
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config/maestro")
    };
    Some(dir.join("maestro.toml"))
}

/// Read-modify-write the config in effect: the project `maestro.toml` when
/// present, otherwise the user-level one (global-inherited projects). Sets
/// `roles.<role>.model` to `{provider, model}` (creating the role table if
/// needed) and returns the path that was written.
pub fn set_role_model(
    root: &Path,
    role: &str,
    provider: &str,
    model: &str,
) -> Result<std::path::PathBuf> {
    let project = root.join("maestro.toml");
    let path = if project.exists() {
        project
    } else {
        user_config_path().filter(|p| p.exists()).ok_or_else(|| {
            MaestroError::Config(
                "no maestro.toml here and no user-level config — run `maestro init`".into(),
            )
        })?
    };
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| MaestroError::Config(format!("{}: {e}", path.display())))?;
    let mut doc: toml::Table =
        toml::from_str(&raw).map_err(|e| MaestroError::Config(format!("maestro.toml: {e}")))?;

    let roles = doc
        .entry("roles")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| MaestroError::Config("maestro.toml: [roles] is not a table".into()))?;
    let role_tbl = roles
        .entry(role.to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| {
            MaestroError::Config(format!("maestro.toml: [roles.{role}] is not a table"))
        })?;
    let mut model_tbl = toml::value::Table::new();
    model_tbl.insert("provider".into(), toml::Value::String(provider.into()));
    model_tbl.insert("model".into(), toml::Value::String(model.into()));
    role_tbl.insert("model".into(), toml::Value::Table(model_tbl));

    let rendered =
        toml::to_string_pretty(&doc).map_err(|e| MaestroError::Config(format!("toml: {e}")))?;
    atomic_write(&path, rendered.as_bytes(), None)?;
    Ok(path)
}

/// Store `providers.<id> = secret` in `.maestro/credentials.json`
/// (`{version, providers}`), dir 0700, file 0600, atomic replace.
pub fn write_credential(root: &Path, provider_id: &str, secret: &str) -> Result<()> {
    let dir = root.join(".maestro");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join("credentials.json");
    let mut doc: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({"version": 1, "providers": {}}));
    if !doc.is_object() {
        doc = serde_json::json!({"version": 1, "providers": {}});
    }
    let providers = doc
        .as_object_mut()
        .and_then(|o| {
            o.entry("providers")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
        })
        .ok_or_else(|| MaestroError::Config("credentials.json: providers not an object".into()))?;
    providers.insert(provider_id.to_string(), serde_json::json!(secret));

    let rendered = serde_json::to_vec_pretty(&doc)?;
    atomic_write(&path, &rendered, Some(0o600))?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| MaestroError::Io("no parent dir".into()))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into()),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_error_surfaces_failure_after_long_log_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".maestro")).unwrap();
        std::fs::write(
            dir.path().join(".maestro/daemon.stderr.log"),
            format!(
                "{}\nerror: incompatible saved database\n",
                "x".repeat(20_000)
            ),
        )
        .unwrap();
        assert_eq!(
            daemon_startup_error(dir.path()),
            "daemon failed to start: incompatible saved database"
        );
        let empty = tempfile::tempdir().unwrap();
        assert!(daemon_startup_error(empty.path()).contains("daemon.stderr.log"));
    }

    #[test]
    fn set_role_model_rewrites_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("maestro.toml"),
            "version = 1\n\n[roles.coder.model]\nprovider = \"mock\"\nmodel = \"coder\"\n",
        )
        .unwrap();
        set_role_model(dir.path(), "coder", "anthropic", "claude-sonnet").unwrap();
        let raw = std::fs::read_to_string(dir.path().join("maestro.toml")).unwrap();
        let doc: toml::Table = toml::from_str(&raw).unwrap();
        assert_eq!(
            doc["roles"]["coder"]["model"]["provider"].as_str(),
            Some("anthropic")
        );
        assert_eq!(
            doc["roles"]["coder"]["model"]["model"].as_str(),
            Some("claude-sonnet")
        );
        assert_eq!(doc["version"].as_integer(), Some(1)); // untouched
    }

    #[test]
    fn set_role_model_creates_missing_role() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("maestro.toml"), "version = 1\n").unwrap();
        set_role_model(dir.path(), "reviewer", "mock", "reviewer").unwrap();
        let raw = std::fs::read_to_string(dir.path().join("maestro.toml")).unwrap();
        assert!(raw.contains("reviewer"));
    }

    #[test]
    fn write_credential_creates_secure_file() {
        let dir = tempfile::tempdir().unwrap();
        write_credential(dir.path(), "anthropic", "sk-test").unwrap();
        let path = dir.path().join(".maestro/credentials.json");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["providers"]["anthropic"].as_str(), Some("sk-test"));
        assert_eq!(doc["version"].as_i64(), Some(1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Second write merges.
        write_credential(dir.path(), "openai", "sk-2").unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["providers"]["anthropic"].as_str(), Some("sk-test"));
        assert_eq!(doc["providers"]["openai"].as_str(), Some("sk-2"));
    }
}

#[cfg(test)]
mod set_role_model_tests {
    use super::*;

    #[test]
    fn writes_project_file_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("maestro.toml"), "version = 1\n").unwrap();
        let path = set_role_model(dir.path(), "coder", "prov", "m1").unwrap();
        assert_eq!(path, dir.path().join("maestro.toml"));
        let written = std::fs::read_to_string(path).unwrap();
        assert!(written.contains("[roles.coder.model]"));
        assert!(written.contains("provider = \"prov\""));
    }

    #[test]
    fn falls_back_to_the_user_level_config() {
        let project = tempfile::tempdir().unwrap(); // no maestro.toml here
        let userdir = tempfile::tempdir().unwrap();
        std::fs::write(userdir.path().join("maestro.toml"), "version = 1\n").unwrap();
        // Env is process-global: this test must not run concurrently with
        // another MAESTRO_CONFIG_DIR user — it's the only one in this crate.
        std::env::set_var("MAESTRO_CONFIG_DIR", userdir.path());
        let result = set_role_model(project.path(), "reviewer", "prov", "m2");
        std::env::remove_var("MAESTRO_CONFIG_DIR");
        let path = result.unwrap();
        assert_eq!(path, userdir.path().join("maestro.toml"));
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("[roles.reviewer.model]"));
    }
}
