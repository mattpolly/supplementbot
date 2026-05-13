/// Debug logging controlled by `DEBUG_LOGGING=true` in the environment.
///
/// All `debug_log!` calls are silent unless DEBUG_LOGGING=true is set.
/// Startup messages, errors, and safety events always print regardless.
use std::sync::atomic::{AtomicBool, Ordering};

pub static DEBUG_LOGGING: AtomicBool = AtomicBool::new(false);

pub fn init() {
    let enabled = std::env::var("DEBUG_LOGGING")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    DEBUG_LOGGING.store(enabled, Ordering::Relaxed);
    if enabled {
        eprintln!("[logging] DEBUG_LOGGING=true — verbose diagnostic output enabled");
    }
}

/// Print only when DEBUG_LOGGING=true.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::logging::DEBUG_LOGGING.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!($($arg)*);
        }
    };
}
