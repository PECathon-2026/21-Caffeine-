//! Credential storage: `.maestro/credentials.json`, dir `0700`, file `0600`,
//! atomic writes. Secrets are injected into the environment only when the
//! variable is not already set, and are never logged.

use crate::config::state_dir;
use maestro_types::config::MaestroConfig;
use maestro_types::{MaestroError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// On-disk shape of `.maestro/credentials.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialsFile {
    pub version: u32,
    /// provider id → secret.
    #[serde(default)]
    pub providers: BTreeMap<String, String>,
}

impl Default for CredentialsFile {
    fn default() -> Self {
        CredentialsFile {
            version: 1,
            providers: BTreeMap::new(),
        }
    }
}

/// Absolute path of the credentials file for a project root.
pub fn credentials_path(root: &Path) -> PathBuf {
    root.join(".maestro").join("credentials.json")
}

/// The user-level credentials file (`~/.config/maestro/credentials.json`).
pub fn user_credentials_path() -> Option<PathBuf> {
    crate::config::user_config_dir().map(|d| d.join("credentials.json"))
}

/// Load stored credentials: user-level entries first, project entries
/// overriding per provider id. Absent files yield the empty default; a
/// malformed file is an error (never silently discarded).
pub fn load_credentials(root: &Path) -> Result<CredentialsFile> {
    let mut creds = match user_credentials_path() {
        Some(path) if path.exists() => read_credentials_file(&path)?,
        _ => CredentialsFile::default(),
    };
    let project = credentials_path(root);
    if project.exists() {
        for (id, secret) in read_credentials_file(&project)?.providers {
            creds.providers.insert(id, secret);
        }
    }
    Ok(creds)
}

fn read_credentials_file(path: &Path) -> Result<CredentialsFile> {
    let raw = fs::read_to_string(path)?;
    let creds: CredentialsFile = serde_json::from_str(&raw)?;
    Ok(creds)
}

/// Store (or replace) a secret in the user-level store, shared by every
/// project that doesn't override it. Dir `0700`, file `0600`, atomic write.
pub fn save_global_credential(provider_id: &str, secret: &str) -> Result<()> {
    let dir = crate::config::user_config_dir()
        .ok_or_else(|| MaestroError::Config("cannot resolve a user config directory".into()))?;
    fs::create_dir_all(&dir)?;
    let mut perms = fs::metadata(&dir)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    fs::set_permissions(&dir, perms)?;

    let path = dir.join("credentials.json");
    let mut creds = if path.exists() {
        read_credentials_file(&path)?
    } else {
        CredentialsFile::default()
    };
    creds
        .providers
        .insert(provider_id.to_string(), secret.to_string());
    let body = serde_json::to_string_pretty(&creds)?;
    let tmp = dir.join(format!("credentials.json.tmp-{}", std::process::id()));
    write_0600(&tmp, body.as_bytes())?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Store (or replace) the secret for one provider. The write is atomic
/// (temp file + rename), the state dir is `0700` and the file `0600`.
pub fn save_credential(root: &Path, provider_id: &str, secret: &str) -> Result<()> {
    let dir = state_dir(root)?; // created 0700 on demand
    let mut creds = load_credentials(root)?;
    creds
        .providers
        .insert(provider_id.to_string(), secret.to_string());
    let body = serde_json::to_string_pretty(&creds)?;

    let tmp = dir.join(format!("credentials.json.tmp-{}", std::process::id()));
    write_0600(&tmp, body.as_bytes())?;
    let path = credentials_path(root);
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// For every configured provider with an `api_key_env`, set that env var from
/// the stored secret IF the variable is not already set — a real environment
/// always wins over the credential store.
///
/// Call this early, before spawning threads: `std::env::set_var` mutates
/// process-global state (safe API on edition 2021, but not thread-safe by
/// contract). Secret values are never logged.
pub fn inject_env(config: &MaestroConfig, creds: &CredentialsFile) {
    for (provider_id, provider) in &config.providers {
        let Some(env_var) = provider.api_key_env.as_deref() else {
            continue;
        };
        if env_var.is_empty() || std::env::var_os(env_var).is_some() {
            continue;
        }
        if let Some(secret) = creds.providers.get(provider_id) {
            std::env::set_var(env_var, secret);
            tracing::debug!(provider = %provider_id, var = %env_var, "injected credential env var");
        }
    }
}

fn write_0600(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::config::{ProviderConfig, ProviderKind};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn absent_file_loads_empty() {
        let _guard = crate::test_env::lock();
        let user = tempfile::tempdir().unwrap();
        std::env::set_var("MAESTRO_CONFIG_DIR", user.path());
        let dir = tempfile::tempdir().unwrap();
        let creds = load_credentials(dir.path()).unwrap();
        std::env::remove_var("MAESTRO_CONFIG_DIR");
        assert!(creds.providers.is_empty());
        assert_eq!(creds.version, 1);
    }

    #[test]
    fn save_sets_modes_and_round_trips() {
        let _guard = crate::test_env::lock();
        let user = tempfile::tempdir().unwrap();
        std::env::set_var("MAESTRO_CONFIG_DIR", user.path());
        let dir = tempfile::tempdir().unwrap();
        save_credential(dir.path(), "anthropic", "sk-test").unwrap();
        save_credential(dir.path(), "openai", "sk-other").unwrap();

        let creds = load_credentials(dir.path()).unwrap();
        assert_eq!(creds.providers.get("anthropic").unwrap(), "sk-test");
        assert_eq!(creds.providers.get("openai").unwrap(), "sk-other");

        let file_mode = fs::metadata(credentials_path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        let dir_mode = fs::metadata(dir.path().join(".maestro"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);

        // Global store: written under the user dir, merged on load, and the
        // project entry wins on collision.
        save_global_credential("anthropic", "sk-global").unwrap();
        save_global_credential("shared", "sk-shared").unwrap();
        let creds = load_credentials(dir.path()).unwrap();
        assert_eq!(creds.providers.get("anthropic").unwrap(), "sk-test"); // project wins
        assert_eq!(creds.providers.get("shared").unwrap(), "sk-shared"); // global fills in
        let mode = fs::metadata(user.path().join("credentials.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        std::env::remove_var("MAESTRO_CONFIG_DIR");
    }

    #[test]
    fn inject_env_respects_existing_values() {
        let mut config = MaestroConfig::default();
        config.providers.insert(
            "p1".into(),
            ProviderConfig {
                kind: ProviderKind::Anthropic,
                base_url: None,
                api_key_env: Some("MAESTRO_TEST_INJECT_UNSET".into()),
                headers: BTreeMap::new(),
                requests_per_minute: None,
                tokens_per_minute: None,
                deployment: None,
                reasoning_effort: None,
            },
        );
        config.providers.insert(
            "p2".into(),
            ProviderConfig {
                kind: ProviderKind::Anthropic,
                base_url: None,
                api_key_env: Some("MAESTRO_TEST_INJECT_PRESET".into()),
                headers: BTreeMap::new(),
                requests_per_minute: None,
                tokens_per_minute: None,
                deployment: None,
                reasoning_effort: None,
            },
        );
        std::env::set_var("MAESTRO_TEST_INJECT_PRESET", "from-env");

        let mut creds = CredentialsFile::default();
        creds.providers.insert("p1".into(), "stored-1".into());
        creds.providers.insert("p2".into(), "stored-2".into());
        inject_env(&config, &creds);

        assert_eq!(
            std::env::var("MAESTRO_TEST_INJECT_UNSET").unwrap(),
            "stored-1"
        );
        assert_eq!(
            std::env::var("MAESTRO_TEST_INJECT_PRESET").unwrap(),
            "from-env"
        );
        std::env::remove_var("MAESTRO_TEST_INJECT_UNSET");
        std::env::remove_var("MAESTRO_TEST_INJECT_PRESET");
    }
}
