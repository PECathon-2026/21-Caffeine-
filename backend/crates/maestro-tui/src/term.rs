//! Terminal discipline: raw mode + alternate screen behind a Drop guard, and
//! a panic hook that restores the terminal before the panic prints.

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use maestro_types::{MaestroError, Result};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the panic hook should restore the terminal. Armed on enter,
/// cleared on restore, so the (unremovable) hook is inert once the guard
/// has already cleaned up.
static HOOK_ARMED: AtomicBool = AtomicBool::new(false);

/// Restores the terminal on Drop. Construct exactly one.
pub struct TermGuard {
    restored: bool,
}

fn restore_terminal() {
    let mut out = std::io::stdout();
    let _ = crossterm::execute!(out, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Gate for the panic hook body: true exactly once per armed guard (the
/// swap also disarms, so re-entrant panics don't double-restore).
fn hook_should_restore() -> bool {
    HOOK_ARMED.swap(false, Ordering::SeqCst)
}

/// Make the installed panic hook a no-op (called by `restore()`).
fn disarm_hook() {
    HOOK_ARMED.store(false, Ordering::SeqCst);
}

impl TermGuard {
    pub fn enter() -> Result<TermGuard> {
        enable_raw_mode().map_err(|e| MaestroError::Io(format!("raw mode: {e}")))?;
        let mut out = std::io::stdout();
        if let Err(e) = crossterm::execute!(out, EnterAlternateScreen, EnableMouseCapture) {
            // Don't leave the user's shell in raw mode on a partial setup.
            let _ = disable_raw_mode();
            return Err(MaestroError::Io(format!("alt screen: {e}")));
        }

        // Panic hook: restore the terminal, then let the default hook print.
        // The hook stays installed forever, so its body is gated on the
        // armed flag and becomes a no-op after restore().
        HOOK_ARMED.store(true, Ordering::SeqCst);
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if hook_should_restore() {
                restore_terminal();
            }
            default_hook(info);
        }));

        Ok(TermGuard { restored: false })
    }

    /// Explicit restore (also invoked by Drop; idempotent).
    pub fn restore(&mut self) {
        if !self.restored {
            self.restored = true;
            disarm_hook();
            restore_terminal();
        }
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_hook_gating_disarms_after_restore() {
        // Disarmed by default: the hook body must do nothing.
        HOOK_ARMED.store(false, Ordering::SeqCst);
        assert!(!hook_should_restore());

        // Armed (as enter() does): fires exactly once, then self-disarms so
        // re-entrant panics don't double-restore.
        HOOK_ARMED.store(true, Ordering::SeqCst);
        assert!(hook_should_restore());
        assert!(!hook_should_restore(), "swap must disarm after one shot");

        // disarm_hook (what restore() runs) makes a later panic a no-op.
        HOOK_ARMED.store(true, Ordering::SeqCst);
        disarm_hook();
        assert!(!hook_should_restore());
    }
}
