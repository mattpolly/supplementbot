use std::sync::atomic::{AtomicBool, Ordering};

pub static DEBUG_LOGGING: AtomicBool = AtomicBool::new(false);

pub fn init() {
    let enabled = std::env::var("DEBUG_LOGGING")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    DEBUG_LOGGING.store(enabled, Ordering::Relaxed);
}

#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::logging::DEBUG_LOGGING.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!($($arg)*);
        }
    };
}
