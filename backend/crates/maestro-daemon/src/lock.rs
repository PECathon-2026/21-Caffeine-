//! Workspace exclusivity: the `.maestro/daemon.lock` directory (atomic
//! `create_dir`) plus the `.maestro/daemon.json` discovery file.
//!
//! A stale lock (its recorded pid is dead, or no `daemon.json` exists at all)
//! is reclaimed automatically. Pid liveness is probed via `/proc/<pid>` on
//! Linux; other unixes fall back to spawning `kill -0 <pid>` — both avoid a
//! `libc` dependency.

use maestro_types::api::DaemonInfo;
use maestro_types::{MaestroError, Result};
use std::path::{Path, PathBuf};

const LOCK_DIR: &str = "daemon.lock";
const INFO_FILE: &str = "daemon.json";

/// Owns the workspace lock. [`WorkspaceLock::release`] (or `Drop`, best
/// effort) removes the lock directory and the discovery file.
#[derive(Debug)]
pub(crate) struct WorkspaceLock {
    lock_dir: PathBuf,
    info_path: PathBuf,
    released: bool,
}

/// True when a process with this pid exists.
pub(crate) fn pid_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    let proc_dir = Path::new("/proc");
    if proc_dir.is_dir() {
        return proc_dir.join(pid.to_string()).exists();
    }
    // Non-Linux unix fallback: signal 0 probes existence without delivering.
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Path of `<state_dir>/daemon.json`.
pub(crate) fn info_path(state_dir: &Path) -> PathBuf {
    state_dir.join(INFO_FILE)
}

/// Read and parse `daemon.json`, if present and valid.
pub(crate) fn read_info(state_dir: &Path) -> Option<DaemonInfo> {
    let raw = std::fs::read_to_string(info_path(state_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Atomically write `daemon.json` with mode 0600 (it may hold the token).
pub(crate) fn write_info(state_dir: &Path, info: &DaemonInfo) -> Result<()> {
    let path = info_path(state_dir);
    let tmp = state_dir.join(format!("{INFO_FILE}.tmp-{}", std::process::id()));
    let body = serde_json::to_string_pretty(info)?;
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

impl WorkspaceLock {
    /// Acquire the lock, reclaiming it when the holder recorded in
    /// `daemon.json` is dead. `Conflict` when another daemon is alive.
    pub(crate) fn acquire(state_dir: &Path) -> Result<WorkspaceLock> {
        let lock_dir = state_dir.join(LOCK_DIR);
        for attempt in 0..2 {
            match std::fs::create_dir(&lock_dir) {
                Ok(()) => {
                    return Ok(WorkspaceLock {
                        lock_dir,
                        info_path: info_path(state_dir),
                        released: false,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    match read_info(state_dir) {
                        Some(info) if pid_alive(info.pid) => {
                            return Err(MaestroError::Conflict(format!(
                                "a daemon is already running for this workspace \
                                 (pid {}, {})",
                                info.pid, info.url
                            )));
                        }
                        // Dead holder, or a crash between mkdir and the json
                        // write: reclaim.
                        _ => {
                            tracing::warn!(
                                lock = %lock_dir.display(),
                                "reclaiming stale daemon lock"
                            );
                            let _ = std::fs::remove_file(info_path(state_dir));
                            std::fs::remove_dir_all(&lock_dir)?;
                            if attempt == 1 {
                                return Err(MaestroError::Conflict(
                                    "could not reclaim the daemon lock".into(),
                                ));
                            }
                        }
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(MaestroError::Conflict(
            "could not acquire the daemon lock".into(),
        ))
    }

    /// Remove `daemon.json` and the lock directory.
    pub(crate) fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let _ = std::fs::remove_file(&self.info_path);
        let _ = std::fs::remove_dir_all(&self.lock_dir);
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        self.release_inner();
    }
}
