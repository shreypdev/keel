//! The panic guard (SPEC 5.6, constitution R6).
//!
//! Every place where the runtime runs code it does not own (a dispatcher, a task poll, an
//! event subscriber, a store's `restore`) goes through [`guarded`], which turns an unwinding
//! panic into a [`PanicReport`] carrying the message and a backtrace.
//!
//! A panic's backtrace only exists at the moment of the panic, so the first guard installs
//! (once per process) a chained panic hook. While a guard is active on the panicking thread
//! the hook records the report in a thread-local instead of printing it; otherwise it defers
//! to the previously installed hook, so panics outside the runtime behave as before.
//!
//! On wasm (`panic = "abort"`) nothing can catch a panic; the hook instead logs it at level 5
//! through the host before the trap, as SPEC 7 requires: the message, then `at <location>` and,
//! when a call was running, `in <operation>` (ADR-046 decision 4.4; the TypeScript runtime turns the
//! record and the trap's stack into the same report a native core delivers through `Diagnostics`).
//!
//! What a native panic produces (ADR-046 decision 4): the message, `file:line:col`, the thread's
//! name and frames. A **release** build keeps raw addresses only (offsets into the image, from the
//! [`FrameSource`](crate::diagnostics::FrameSource) the embedding shim installs), which a
//! symbolicator resolves with the symbol files of `undra build --release`; a **debug** build also
//! names each frame from the backtrace text. Neither costs anything until a panic.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::sync::Once;

/// The most frames a report carries.
#[cfg(not(target_family = "wasm"))]
pub(crate) const MAX_FRAMES: usize = 48;

/// One frame of a caught panic: what `PanicFrame` of `undra-ports` encodes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frame {
    /// Offset of the instruction into its image (0 when unknown).
    pub address: u64,
    /// The function, in builds whose backtrace names it.
    pub symbol: Option<String>,
    /// The source file, likewise.
    pub file: Option<String>,
    /// The line in `file`.
    pub line: Option<u32>,
}

/// A caught panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PanicReport {
    /// The panic message.
    pub message: String,
    /// The panic location followed by the backtrace, or `"unavailable"` on wasm.
    pub backtrace: String,
    /// `file:line:col` of the panic, empty when the payload did not come through the hook.
    pub location: String,
    /// The name of the thread that panicked.
    pub thread: String,
    /// The backtrace as frames, innermost first.
    pub frames: Vec<Frame>,
}

/// The payload of a panic that crossed a thread boundary (`spawn_blocking`) and is re-raised
/// with `resume_unwind`; keeps the original report intact.
pub(crate) struct CarriedPanic(pub PanicReport);

/// The per-thread state of the guard: how many guards are active and the last panic recorded
/// while one was. One thread-local holds both, so entering and leaving a guard (the hot path of
/// every dispatched call) each cost one thread-local access.
struct GuardState {
    depth: Cell<u32>,
    last: RefCell<Option<PanicReport>>,
}

thread_local! {
    static STATE: GuardState = const {
        GuardState {
            depth: Cell::new(0),
            last: RefCell::new(None),
        }
    };
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(carried) = payload.downcast_ref::<CarriedPanic>() {
        carried.0.message.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}

#[cfg(not(target_family = "wasm"))]
fn capture_backtrace() -> String {
    std::backtrace::Backtrace::force_capture().to_string()
}

#[cfg(target_family = "wasm")]
fn capture_backtrace() -> String {
    "unavailable".to_owned()
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned()
}

/// The frames of a panic that is being reported now, from the backtrace `text` and the addresses
/// the embedding shim can read (ADR-046 decision 4.1). Release builds keep the addresses alone.
#[cfg(not(target_family = "wasm"))]
fn capture_frames(text: &str) -> Vec<Frame> {
    let addresses = crate::diagnostics::capture_addresses(MAX_FRAMES + 16);
    if cfg!(debug_assertions) {
        crate::diagnostics::name_frames(parse_backtrace(text), &addresses, MAX_FRAMES)
    } else {
        addresses
            .into_iter()
            .take(MAX_FRAMES)
            .map(|address| Frame {
                address,
                ..Frame::default()
            })
            .collect()
    }
}

#[cfg(target_family = "wasm")]
fn capture_frames(_text: &str) -> Vec<Frame> {
    Vec::new()
}

/// Reads the frames out of the text of a `std::backtrace::Backtrace`: a line `N: symbol`, then
/// optionally `at file:line:col`.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn parse_backtrace(text: &str) -> Vec<Frame> {
    let mut frames: Vec<Frame> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(at) = trimmed.strip_prefix("at ") {
            if let Some(frame) = frames.last_mut() {
                let (file, line_no) = split_location(at);
                frame.file = Some(file.to_owned());
                frame.line = line_no;
            }
            continue;
        }
        let Some((index, symbol)) = trimmed.split_once(": ") else {
            continue;
        };
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        frames.push(Frame {
            address: 0,
            symbol: (symbol != "<unknown>").then(|| symbol.to_owned()),
            file: None,
            line: None,
        });
    }
    frames
}

/// `file:line:col` into the file and the line.
#[cfg(not(target_family = "wasm"))]
fn split_location(at: &str) -> (&str, Option<u32>) {
    let mut parts = at.rsplitn(3, ':');
    let last = parts.next().and_then(|p| p.parse::<u32>().ok());
    match (
        last,
        parts.next().and_then(|p| p.parse::<u32>().ok()),
        parts.next(),
    ) {
        (Some(_column), Some(line), Some(file)) => (file, Some(line)),
        (Some(line), None, _) => (at.rsplit_once(':').map_or(at, |(file, _)| file), Some(line)),
        _ => (at, None),
    }
}

fn hook(previous: &(dyn Fn(&PanicHookInfo<'_>) + Send + Sync), info: &PanicHookInfo<'_>) {
    let guarded = STATE.try_with(|state| state.depth.get()).unwrap_or(0) > 0;
    if cfg!(target_family = "wasm") {
        // The record the host turns into a report after the trap: the message, then `at <location>`
        // and `in <operation>` on lines of their own (the TypeScript runtime reads them from the end).
        let mut record = payload_message(info.payload());
        if let Some(l) = info.location() {
            record.push_str(&format!(
                "\n    at {}:{}:{}",
                l.file(),
                l.line(),
                l.column()
            ));
        }
        if let Some(operation) = crate::runtime::running_operation() {
            record.push_str("\n    in ");
            record.push_str(&operation);
        }
        crate::runtime::log_fatal_current("undra::panic", &record);
        previous(info);
        return;
    }
    if !guarded {
        previous(info);
        return;
    }
    let message = payload_message(info.payload());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    let text = capture_backtrace();
    let report = PanicReport {
        message,
        backtrace: if location.is_empty() {
            text.clone()
        } else {
            format!("panicked at {location}\n{text}")
        },
        location,
        thread: thread_name(),
        frames: capture_frames(&text),
    };
    let _ = STATE.try_with(|state| *state.last.borrow_mut() = Some(report));
}

/// The report of a panic that something else caught (`undra-signals` catches a computed's), as the
/// hook recorded it on this thread, or one with just `message` when no guard was active (so no
/// hook recorded one). Takes the recording: a report is made once per panic.
pub(crate) fn caught_elsewhere(message: &str) -> PanicReport {
    let recorded = STATE
        .try_with(|state| state.last.borrow_mut().take())
        .ok()
        .flatten();
    match recorded {
        Some(report) if report.message == message => report,
        _ => PanicReport {
            message: message.to_owned(),
            backtrace: String::new(),
            location: String::new(),
            thread: thread_name(),
            frames: Vec::new(),
        },
    }
}

/// Installs the chained panic hook (once per process).
pub(crate) fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| hook(&*previous, info)));
    });
}

/// Builds the report for a caught panic payload.
fn report_from(payload: Box<dyn Any + Send>) -> PanicReport {
    if let Some(carried) = payload.downcast_ref::<CarriedPanic>() {
        return carried.0.clone();
    }
    let message = payload_message(&*payload);
    let recorded = STATE
        .try_with(|state| state.last.borrow_mut().take())
        .ok()
        .flatten();
    // The hook's recording is used only if its message matches the payload's, so a stale
    // recording (a panic that user code caught itself) can never be attributed to a later
    // panic that bypassed the hook (`resume_unwind`).
    match recorded {
        Some(report) if report.message == message => report,
        _ => {
            let backtrace = capture_backtrace();
            PanicReport {
                message,
                frames: capture_frames(&backtrace),
                backtrace,
                location: String::new(),
                thread: thread_name(),
            }
        }
    }
}

/// Runs `f`, converting a panic into a [`PanicReport`].
pub(crate) fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, PanicReport> {
    install_hook();
    STATE.with(|state| {
        if state.depth.get() == 0 {
            // A recording left over from a panic that user code caught itself must not be
            // attributed to this guard's panic.
            *state.last.borrow_mut() = None;
        }
        state.depth.set(state.depth.get() + 1);
    });
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    STATE.with(|state| state.depth.set(state.depth.get().saturating_sub(1)));
    result.map_err(report_from)
}

/// Drops `value` under the guard, so a panicking `Drop` cannot unwind into the runtime.
pub(crate) fn drop_guarded<T>(value: T) -> Result<(), PanicReport> {
    guarded(move || drop(value))
}

/// Encodes a report as the body of a status 2 reply: `String message, String backtrace`.
pub(crate) fn encode_panic_body(report: &PanicReport) -> Vec<u8> {
    let mut w =
        undra_wire::Writer::with_capacity(8 + report.message.len() + report.backtrace.len());
    w.write_str(&report.message);
    w.write_str(&report.backtrace);
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_passes_through() {
        assert_eq!(guarded(|| 5), Ok(5));
    }

    #[test]
    fn str_and_string_panics_are_reported() {
        let report = guarded(|| -> () { panic!("static message") }).unwrap_err();
        assert_eq!(report.message, "static message");
        assert!(
            report.backtrace.contains("panicked at"),
            "{}",
            report.backtrace
        );
        let n = 7;
        let report = guarded(|| -> () { panic!("formatted {n}") }).unwrap_err();
        assert_eq!(report.message, "formatted 7");
    }

    #[test]
    fn non_string_payload_is_described() {
        let report = guarded(|| -> () { panic::panic_any(42_u8) }).unwrap_err();
        assert_eq!(report.message, "panic with a non-string payload");
    }

    #[test]
    fn nested_guards_report_the_innermost_panic_once() {
        let outer = guarded(|| {
            let inner = guarded(|| -> () { panic!("inner") }).unwrap_err();
            assert_eq!(inner.message, "inner");
            "survived"
        });
        assert_eq!(outer, Ok("survived"));
        // The stale report of the inner panic does not leak into a later, unrelated one.
        let later = guarded(|| -> () { panic!("later") }).unwrap_err();
        assert_eq!(later.message, "later");
    }

    #[test]
    fn carried_panics_keep_their_original_report() {
        let original = PanicReport {
            message: "from another thread".into(),
            backtrace: "frames".into(),
            location: "a.rs:1:2".into(),
            thread: "pool".into(),
            frames: vec![Frame {
                address: 7,
                ..Frame::default()
            }],
        };
        let carried = original.clone();
        let report =
            guarded(move || -> () { panic::resume_unwind(Box::new(CarriedPanic(carried))) })
                .unwrap_err();
        assert_eq!(report, original);
    }

    #[test]
    fn panicking_drop_is_contained() {
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) {
                panic!("drop bomb");
            }
        }
        assert_eq!(drop_guarded(Bomb).unwrap_err().message, "drop bomb");
    }

    #[test]
    fn a_contained_panic_carries_its_location_thread_and_frames() {
        let report = guarded(|| -> () { panic!("where") }).unwrap_err();
        assert!(report.location.contains("guard.rs:"), "{}", report.location);
        assert!(!report.thread.is_empty());
        // The test runtime installs no frame source, so a release build has no addresses; a debug
        // build names the frames from the backtrace text.
        if cfg!(debug_assertions) {
            assert!(
                report.frames.iter().any(|f| f.symbol.is_some()),
                "{:?}",
                report.frames
            );
        }
    }

    #[test]
    fn backtrace_text_is_read_into_frames() {
        let text = "   0: undra_runtime::guard::capture\n             at /src/guard.rs:75:9\n   1: <unknown>\n  12: main\n             at ./m.rs:3\nnoise: not a frame\n";
        let frames = parse_backtrace(text);
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames[0].symbol.as_deref(),
            Some("undra_runtime::guard::capture")
        );
        assert_eq!(frames[0].file.as_deref(), Some("/src/guard.rs"));
        assert_eq!(frames[0].line, Some(75));
        assert_eq!(frames[1].symbol, None);
        assert_eq!(frames[2].file.as_deref(), Some("./m.rs"));
        assert_eq!(frames[2].line, Some(3));
    }

    #[test]
    fn panic_body_encodes_two_strings() {
        let body = encode_panic_body(&PanicReport {
            message: "m".into(),
            backtrace: "bt".into(),
            location: String::new(),
            thread: String::new(),
            frames: Vec::new(),
        });
        assert_eq!(body, [1, 0, 0, 0, b'm', 2, 0, 0, 0, b'b', b't']);
    }
}
