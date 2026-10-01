//! Logging: everything the core says goes to [`Host::log`](crate::Host::log) (SPEC 15: no
//! `println!` in core crates).
//!
//! Levels follow the Log port: [`TRACE`] 0, [`DEBUG`] 1, [`INFO`] 2, [`WARN`] 3, [`ERROR`] 4,
//! [`FATAL`] 5. A record is forwarded when its level is at least
//! [`RuntimeConfig::log_level`](crate::RuntimeConfig::log_level).
//!
//! ```
//! use undra_runtime::{undra_info, undra_warn};
//! use undra_runtime::testing::TestRuntime;
//!
//! let t = TestRuntime::new();
//! let _scope = t.ctx().enter();
//! undra_info!("connected to {}", "server");
//! undra_warn!(target: "my::module", "slow response: {} ms", 1200);
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

/// Logs at an explicit level: `undra_log!(level, "fmt {}", x)` or
/// `undra_log!(level, target: "my::target", "fmt {}", x)`. The default target is the calling
/// module's path.
#[macro_export]
macro_rules! undra_log {
    ($level:expr, target: $target:expr, $($arg:tt)+) => {
        $crate::log::log($level, $target, &::std::format!($($arg)+))
    };
    ($level:expr, $($arg:tt)+) => {
        $crate::log::log($level, ::core::module_path!(), &::std::format!($($arg)+))
    };
}

/// Logs at level 0 (trace). See [`undra_log!`].
#[macro_export]
macro_rules! undra_trace {
    ($($arg:tt)+) => { $crate::undra_log!($crate::log::TRACE, $($arg)+) };
}

/// Logs at level 1 (debug). See [`undra_log!`].
#[macro_export]
macro_rules! undra_debug {
    ($($arg:tt)+) => { $crate::undra_log!($crate::log::DEBUG, $($arg)+) };
}

/// Logs at level 2 (info). See [`undra_log!`].
#[macro_export]
macro_rules! undra_info {
    ($($arg:tt)+) => { $crate::undra_log!($crate::log::INFO, $($arg)+) };
}

/// Logs at level 3 (warn). See [`undra_log!`].
#[macro_export]
macro_rules! undra_warn {
    ($($arg:tt)+) => { $crate::undra_log!($crate::log::WARN, $($arg)+) };
}

/// Logs at level 4 (error). See [`undra_log!`].
#[macro_export]
macro_rules! undra_error {
    ($($arg:tt)+) => { $crate::undra_log!($crate::log::ERROR, $($arg)+) };
}

pub use crate::{
    undra_debug as debug, undra_error as error, undra_info as info, undra_trace as trace,
    undra_warn as warn,
};
