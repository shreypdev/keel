//! Logging: everything the core says goes to [`Host::log`](crate::Host::log) (SPEC 15: no
//! `println!` in core crates).
//!
//! Levels follow the Log port: [`TRACE`] 0, [`DEBUG`] 1, [`INFO`] 2, [`WARN`] 3, [`ERROR`] 4,
//! [`FATAL`] 5. A record is forwarded when its level is at least
//! [`RuntimeConfig::log_level`](crate::RuntimeConfig::log_level).
//!
//! ```
//! use keel_runtime::{keel_info, keel_warn};
//! use keel_runtime::testing::TestRuntime;
//!
//! let t = TestRuntime::new();
//! let _scope = t.ctx().enter();
//! keel_info!("connected to {}", "server");
//! keel_warn!(target: "my::module", "slow response: {} ms", 1200);
//! let logs = t.host().take_logs();
//! assert_eq!(logs[0].message, "connected to server");
//! assert_eq!(logs[1].target, "my::module");
//! ```

/// Level 0.
pub const TRACE: u8 = 0;
/// Level 1.
pub const DEBUG: u8 = 1;
/// Level 2.
pub const INFO: u8 = 2;
/// Level 3.
pub const WARN: u8 = 3;
/// Level 4.
pub const ERROR: u8 = 4;
/// Level 5: the runtime caught a panic.
pub const FATAL: u8 = 5;

/// Sends a record through the current runtime (the one whose call or task is executing on
/// this thread, else the global one). Dropped silently if there is none.
pub fn log(level: u8, target: &str, msg: &str) {
    if let Some(rt) = crate::runtime::current_or_global() {
        rt.log(level, target, msg);
    }
}

/// Logs at an explicit level: `keel_log!(level, "fmt {}", x)` or
/// `keel_log!(level, target: "my::target", "fmt {}", x)`. The default target is the calling
/// module's path.
#[macro_export]
macro_rules! keel_log {
    ($level:expr, target: $target:expr, $($arg:tt)+) => {
        $crate::log::log($level, $target, &::std::format!($($arg)+))
    };
    ($level:expr, $($arg:tt)+) => {
        $crate::log::log($level, ::core::module_path!(), &::std::format!($($arg)+))
    };
}

/// Logs at level 0 (trace). See [`keel_log!`].
#[macro_export]
macro_rules! keel_trace {
    ($($arg:tt)+) => { $crate::keel_log!($crate::log::TRACE, $($arg)+) };
}

/// Logs at level 1 (debug). See [`keel_log!`].
#[macro_export]
macro_rules! keel_debug {
    ($($arg:tt)+) => { $crate::keel_log!($crate::log::DEBUG, $($arg)+) };
}

/// Logs at level 2 (info). See [`keel_log!`].
#[macro_export]
macro_rules! keel_info {
    ($($arg:tt)+) => { $crate::keel_log!($crate::log::INFO, $($arg)+) };
}

/// Logs at level 3 (warn). See [`keel_log!`].
#[macro_export]
macro_rules! keel_warn {
    ($($arg:tt)+) => { $crate::keel_log!($crate::log::WARN, $($arg)+) };
}

/// Logs at level 4 (error). See [`keel_log!`].
#[macro_export]
macro_rules! keel_error {
    ($($arg:tt)+) => { $crate::keel_log!($crate::log::ERROR, $($arg)+) };
}

pub use crate::{
    keel_debug as debug, keel_error as error, keel_info as info, keel_trace as trace,
    keel_warn as warn,
};
