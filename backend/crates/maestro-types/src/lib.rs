//! Pure data contracts shared by every Maestro crate. No I/O lives here.

pub mod api;
pub mod config;
pub mod domain;
pub mod error;
pub mod event;
pub mod history;
pub mod plan;
pub mod protocol;
pub mod provider;

pub use error::MaestroError;

/// Unix milliseconds. Every timestamp in Maestro uses this.
pub type Millis = i64;

pub fn now_ms() -> Millis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 32-char lowercase hex UUIDv4 (simple form) — the id format for every entity.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub type Result<T> = std::result::Result<T, MaestroError>;

/// Identity of the running binary: length + mtime of `current_exe`.
///
/// Lets a client detect a daemon left running from an older build (the
/// daemon captures this at boot; a client compares against its own). After
/// an in-place rebuild the old daemon's exe path is unlinked, its metadata
/// read fails, and the ids can no longer agree — exactly what we want.
pub fn build_id() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{}-{mtime}", m.len())
        })
        .unwrap_or_else(|| "unknown".into())
}
