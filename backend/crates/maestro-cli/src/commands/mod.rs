//! Command implementations. Each returns the process exit code.

pub mod bench;
pub mod history;
pub mod missions;
pub mod project;
pub mod run;

use maestro_types::Result;

/// Emit a machine-readable JSON document on stdout.
pub(crate) fn emit_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
