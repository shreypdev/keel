//! [`RuntimeConfig`] and the runtime's error types.

use core::fmt;

use undra_wire::{Decode, Encode, Reader, WireError, Writer};

/// The value of [`RuntimeConfig::mode`] for a core running inside the host process.
pub const MODE_INPROC: &str = "inproc";
/// The value of [`RuntimeConfig::mode`] for a core driven over a dev transport; adds
/// devtools logging (SPEC 5.10).
pub const MODE_DEV: &str = "dev";

/// Runtime settings, passed to `undra_init` as an encoded record (SPEC 6).
///
/// Wire layout: `platform String, mode String, core_threads u8, blocking_threads u8,
/// log_level u8`. The codec is hand-written (this crate has no macros).
///
/// # Example
///
/// ```
/// use undra_runtime::RuntimeConfig;
/// use undra_wire::{Decode, Encode, Reader, Writer};
///
/// let cfg = RuntimeConfig { platform: "ios".into(), ..RuntimeConfig::default() };
/// let mut w = Writer::new();
/// cfg.encode(&mut w);
/// assert_eq!(RuntimeConfig::decode(&mut Reader::new(w.as_slice())).unwrap(), cfg);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// The host platform: `"ios"`, `"android"`, `"web"`, `"rust"`, ...
    pub platform: String,
    /// `"inproc"` or `"dev"` ([`MODE_INPROC`], [`MODE_DEV`]).
    pub mode: String,
    /// `0` runs no `undra-core` thread: the host drives the executor with
    /// [`Runtime::poll`](crate::Runtime::poll) (always the case on wasm). Any other value
    /// starts the one `undra-core` thread; v1 has a single mutator, so more is not used.
    pub core_threads: u8,
    /// Size of the blocking pool; `0` means `min(4, available cores)`.
    pub blocking_threads: u8,
    /// Records below this level are not forwarded to [`Host::log`](crate::Host::log).
    pub log_level: u8,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            platform: "rust".to_owned(),
            mode: MODE_INPROC.to_owned(),
            core_threads: 1,
            blocking_threads: 0,
            log_level: 2,
        }
    }
}

impl Encode for RuntimeConfig {
    fn encode(&self, w: &mut Writer) {
        self.platform.encode(w);
        self.mode.encode(w);
        self.core_threads.encode(w);
        self.blocking_threads.encode(w);
        self.log_level.encode(w);
    }
}

impl Decode for RuntimeConfig {
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(RuntimeConfig {
            platform: String::decode(r)?,
            mode: String::decode(r)?,
            core_threads: u8::decode(r)?,
            blocking_threads: u8::decode(r)?,
            log_level: u8::decode(r)?,
        })
    }
}

/// Why [`Runtime::init`](crate::Runtime::init) failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitError {
    /// A global runtime already exists; shut it down first.
    AlreadyInitialized,
    /// `RuntimeConfig::mode` was neither `"inproc"` nor `"dev"`.
    InvalidMode(String),
    /// The `undra-core` thread could not be started.
    Spawn(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::AlreadyInitialized => f.write_str("an Undra runtime is already initialized"),
            InitError::InvalidMode(mode) => {
                write!(
                    f,
                    "invalid runtime mode {mode:?}: expected \"inproc\" or \"dev\""
                )
            }
            InitError::Spawn(reason) => {
                write!(f, "could not start the undra-core thread: {reason}")
            }
        }
    }
}

impl std::error::Error for InitError {}

/// Why [`Runtime::restore`](crate::Runtime::restore) failed. A failed restore leaves the
/// runtime unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The snapshot payload is malformed.
    Decode(WireError),
    /// The snapshot names a store type that no `#[undra::store]` registered a restorer for.
    UnknownStoreType {
        /// The type id from the snapshot.
        type_id: u32,
    },
    /// A store's own `restore` rejected its values.
    Store {
        /// The store type.
        type_id: u32,
        /// What the store's decoder reported.
        source: WireError,
    },
    /// A store's `restore` function panicked.
    Panicked {
        /// The store type.
        type_id: u32,
        /// The panic message.
        message: String,
    },
    /// The snapshot contains the null handle, the same handle twice, a generation of `0` or
    /// `u32::MAX`, or a slot index implausibly far beyond the table.
    BadHandle {
        /// The offending raw handle.
        handle: u64,
    },
    /// The snapshot's generation floor is `u32::MAX`: every generation had been issued when it
    /// was taken, so a runtime restored from it could never create an object (ADR-022).
    GenerationFloor {
        /// The floor from the snapshot.
        floor: u32,
    },
    /// The runtime has been shut down.
    ShutDown,
    /// Called from inside a host callback on the thread that holds the core lock.
    Reentrant,
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RestoreError::Decode(e) => write!(f, "malformed snapshot: {e}"),
            RestoreError::UnknownStoreType { type_id } => {
                write!(f, "snapshot contains unknown store type {type_id:#010x}")
            }
            RestoreError::Store { type_id, source } => {
                write!(f, "store {type_id:#010x} rejected its snapshot: {source}")
            }
            RestoreError::Panicked { type_id, message } => {
                write!(f, "restoring store {type_id:#010x} panicked: {message}")
            }
            RestoreError::BadHandle { handle } => {
                write!(
                    f,
                    "snapshot has an invalid or duplicate handle {handle:#018x}"
                )
            }
            RestoreError::GenerationFloor { floor } => write!(
                f,
                "snapshot generation floor {floor:#010x} leaves no generation to issue"
            ),
            RestoreError::ShutDown => f.write_str("the runtime is shut down"),
            RestoreError::Reentrant => {
                f.write_str("E_REENTRANT: restore called from inside a host callback")
            }
        }
    }
}

impl std::error::Error for RestoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(cfg: &RuntimeConfig) -> RuntimeConfig {
        let mut w = Writer::new();
        cfg.encode(&mut w);
        let mut r = Reader::new(w.as_slice());
        let back = RuntimeConfig::decode(&mut r).unwrap();
        r.finish().unwrap();
        back
    }

    #[test]
    fn config_layout_is_platform_mode_then_three_bytes() {
        let cfg = RuntimeConfig {
            platform: "web".into(),
            mode: "dev".into(),
            core_threads: 1,
            blocking_threads: 2,
            log_level: 3,
        };
        let mut w = Writer::new();
        cfg.encode(&mut w);
        assert_eq!(
            w.as_slice(),
            [
                3, 0, 0, 0, b'w', b'e', b'b', 3, 0, 0, 0, b'd', b'e', b'v', 1, 2, 3
            ]
        );
        assert_eq!(roundtrip(&cfg), cfg);
    }

    #[test]
    fn config_round_trips_unicode_and_empty() {
        let cfg = RuntimeConfig {
            platform: "plataforma \u{1F30A}".into(),
            mode: String::new(),
            core_threads: 255,
            blocking_threads: 0,
            log_level: 5,
        };
        assert_eq!(roundtrip(&cfg), cfg);
        assert_eq!(
            roundtrip(&RuntimeConfig::default()),
            RuntimeConfig::default()
        );
    }

    #[test]
    fn config_rejects_every_truncation() {
        let mut w = Writer::new();
        RuntimeConfig::default().encode(&mut w);
        let bytes = w.into_vec();
        for cut in 0..bytes.len() {
            assert!(
                RuntimeConfig::decode(&mut Reader::new(&bytes[..cut])).is_err(),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn config_rejects_invalid_utf8() {
        let bytes = [1, 0, 0, 0, 0xff, 0, 0, 0, 0, 1, 0, 2];
        assert!(matches!(
            RuntimeConfig::decode(&mut Reader::new(&bytes)),
            Err(WireError::InvalidUtf8 { .. })
        ));
    }

    #[test]
    fn errors_display_something_useful() {
        assert!(
            InitError::InvalidMode("x".into())
                .to_string()
                .contains("\"x\"")
        );
        assert!(
            RestoreError::UnknownStoreType { type_id: 7 }
                .to_string()
                .contains("0x00000007")
        );
        assert!(RestoreError::Reentrant.to_string().contains("E_REENTRANT"));
    }
}
