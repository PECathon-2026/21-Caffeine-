//! The `maestro` binary. Thin by design: `--version` is answered straight
//! from argv before any dependency initializes (startup-latency requirement,
//! see SPEC §Goals); everything else lives in `maestro_cli`.

fn main() {
    // Fast path: mirror clap's `--version` output without touching clap,
    // tokio, or any store/engine code.
    let mut is_serve = false;
    for arg in std::env::args().skip(1) {
        if arg == "--version" || arg == "-V" {
            println!("maestro {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        if arg == "--" {
            break;
        }
        is_serve |= arg == "serve";
    }
    // Behave like a normal Unix CLI when piped into `head` etc.: die on
    // SIGPIPE instead of panicking on EPIPE. The daemon (`serve`) keeps
    // Rust's default ignore so closed sockets surface as io errors.
    #[cfg(unix)]
    if !is_serve {
        // SAFETY: single-threaded here (before the runtime starts); resetting
        // a signal disposition has no other observable effect.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    std::process::exit(maestro_cli::run());
}
