//! Panic reports for the app's crash reporter (ADR-046 decision 4).
//!
//! Every panic the runtime contains is reported twice: as the FATAL `undra::panic` log record it
//! always was, and as one structured `PanicReport` handed to the standard `Diagnostics` port
//! (`undra-ports`), which the platform runtimes forward to `LoadOptions.onPanic`. The report is
//! encoded here, by hand, with the layout `undra_ports::PanicReport` declares (a test of
//! `undra-ports` decodes this encoder's output with the type), because `undra-ports` depends on
//! this crate and not the other way round.
//!
//! What identifies the core (its namespace and version) is a [`CoreIdentity`] the generated shim
//! submits through `inventory` (`undra_ffi::export_core!`). What reads the stack and the image
//! (instruction addresses relative to the image, the Mach-O UUID or ELF build id) is `unsafe`
//! work, so it lives in `undra-ffi`, which installs it with [`install_frame_source`]; a runtime
//! with none (a test runtime, the dev runner) reports no addresses and an empty image id.

use std::sync::OnceLock;

use undra_meta::ids;
use undra_wire::Writer;

#[cfg(not(target_family = "wasm"))]
use crate::guard::Frame;
use crate::guard::PanicReport;

/// The port id of the standard `Diagnostics` port (`fnv1a32("port.Diagnostics")`).
pub const DIAGNOSTICS_PORT: u32 = ids::port_id("Diagnostics");

/// The method id of `Diagnostics.panicked`.
pub const PANICKED_METHOD: u32 = ids::port_method_id("Diagnostics", "panicked");

/// What names the core a report comes from: submitted once per core image with `inventory`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreIdentity {
    /// The core's namespace (`[core] namespace`, ADR-044).
    pub namespace: &'static str,
    /// The core's version (the app core crate's).
    pub version: &'static str,
}

inventory::collect!(CoreIdentity);

/// Reads the stack and the image of the running process, for the frames of a report. Implemented
/// by `undra-ffi`, the only crate that may use `unsafe`.
pub trait FrameSource: Send + Sync + 'static {
    /// The instruction addresses of the calling thread's stack, innermost first, each as an offset
    /// into the image that contains it (so they do not depend on where the loader put the image).
    /// At most `max` of them; an address in no known image is left out.
    fn capture(&self, max: usize) -> Vec<u64>;

    /// The identity of the image the core's code is in, lowercase hex without separators: the
    /// Mach-O `LC_UUID` (32 digits) or the ELF GNU build id. Empty when it cannot be read.
    fn image_id(&self) -> String;
}

static SOURCE: OnceLock<&'static dyn FrameSource> = OnceLock::new();

/// Installs the frame source of this image. The first call wins; later ones are ignored.
pub fn install_frame_source(source: &'static dyn FrameSource) {
    let _ = SOURCE.set(source);
}

/// The addresses of the calling thread's stack (empty without a [`FrameSource`]).
#[cfg(not(target_family = "wasm"))]
pub(crate) fn capture_addresses(max: usize) -> Vec<u64> {
    SOURCE
        .get()
        .map_or_else(Vec::new, |source| source.capture(max))
}

fn image_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        SOURCE
            .get()
            .map(|source| source.image_id())
            .unwrap_or_default()
    })
}

/// The namespace and version of the core this image holds (empty strings when the shim submitted
/// none: a test runtime, a hand-written embedding).
pub(crate) fn identity() -> CoreIdentity {
    inventory::iter::<CoreIdentity>
        .into_iter()
        .next()
        .copied()
        .unwrap_or(CoreIdentity {
            namespace: "",
            version: "",
        })
}

/// Whether `symbol` belongs to the machinery between a panic and the hook that reports it.
#[cfg(not(target_family = "wasm"))]
fn is_machinery(symbol: &str) -> bool {
    [
        "std::panicking::",
        "core::panicking::",
        "rust_begin_unwind",
        "std::sys::backtrace::",
        "std::backtrace::",
        "undra_runtime::guard::",
    ]
    .iter()
    .any(|prefix| symbol.starts_with(prefix))
}

/// Adds the addresses to the frames named from the backtrace text (a debug build), aligned from
/// the outermost frame (both stacks were taken inside the hook and share their bottom), and drops
/// the panic machinery from the top.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn name_frames(mut parsed: Vec<Frame>, addresses: &[u64], max: usize) -> Vec<Frame> {
    let (n, m) = (parsed.len(), addresses.len());
    for i in 0..n.min(m) {
        parsed[n - 1 - i].address = addresses[m - 1 - i];
    }
    let skip = parsed
        .iter()
        .take(24)
        .rposition(|f| f.symbol.as_deref().is_some_and(is_machinery))
        .map_or(0, |at| at + 1);
    parsed.drain(..skip.min(n));
    parsed.truncate(max);
    parsed
}

fn write_option_str(w: &mut Writer, value: Option<&str>) {
    match value {
        Some(text) => {
            w.write_u8(1);
            w.write_str(text);
        }
        None => w.write_u8(0),
    }
}

/// Encodes the arguments of `Diagnostics.panicked(report)`: one `PanicReport` (the layout of
/// `undra_ports::PanicReport`), for a panic the runtime with `schema_hash` contained while it
/// was running `operation`.
pub(crate) fn encode_report(report: &PanicReport, operation: &str, schema_hash: u64) -> Vec<u8> {
    let who = identity();
    let mut w = Writer::with_capacity(
        96 + report.message.len() + report.location.len() + report.frames.len() * 16,
    );
    w.write_str(&report.message);
    w.write_str(&report.location);
    w.write_str(operation);
    w.write_str(&report.thread);
    w.write_len(u32::try_from(report.frames.len()).unwrap_or(u32::MAX));
    for frame in &report.frames {
        w.write_u64(frame.address);
        write_option_str(&mut w, frame.symbol.as_deref());
        write_option_str(&mut w, frame.file.as_deref());
        match frame.line {
            Some(line) => {
                w.write_u8(1);
                w.write_u32(line);
            }
            None => w.write_u8(0),
        }
    }
    w.write_str(who.namespace);
    w.write_str(who.version);
    w.write_u64(schema_hash);
    w.write_str(image_id());
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(symbol: &str) -> Frame {
        Frame {
            symbol: Some(symbol.to_owned()),
            ..Frame::default()
        }
    }

    #[test]
    fn the_ids_are_the_derivations_every_platform_hard_codes() {
        assert_eq!(DIAGNOSTICS_PORT, ids::fnv1a32("port.Diagnostics"));
        assert_eq!(PANICKED_METHOD, ids::fnv1a32("Diagnostics.panicked"));
    }

    #[test]
    fn frames_get_addresses_from_the_bottom_and_lose_the_machinery() {
        let parsed = vec![
            frame("std::backtrace::Backtrace::create"),
            frame("undra_runtime::guard::hook"),
            frame("std::panicking::rust_panic_with_hook"),
            frame("core::panicking::panic_fmt"),
            frame("app::explode"),
            frame("undra_runtime::runtime::dispatch"),
        ];
        // The address list was taken a frame or two deeper or shallower; the bottom is shared.
        let addresses = [10, 20, 30, 40, 50, 60, 70];
        let named = name_frames(parsed, &addresses, 48);
        let symbols: Vec<_> = named.iter().map(|f| f.symbol.as_deref().unwrap()).collect();
        assert_eq!(
            symbols,
            ["app::explode", "undra_runtime::runtime::dispatch"]
        );
        assert_eq!(named[0].address, 60);
        assert_eq!(named[1].address, 70);
    }

    #[test]
    fn a_report_encodes_in_the_layout_of_the_record() {
        let report = PanicReport {
            message: "boom".into(),
            backtrace: String::new(),
            location: "a.rs:1:2".into(),
            thread: "t".into(),
            frames: vec![Frame {
                address: 0x10,
                symbol: Some("f".into()),
                file: None,
                line: Some(9),
            }],
        };
        let bytes = encode_report(&report, "Todos.add", 0xAB);
        let mut expected = Writer::new();
        for text in ["boom", "a.rs:1:2", "Todos.add", "t"] {
            expected.write_str(text);
        }
        expected.write_len(1);
        expected.write_u64(0x10);
        expected.write_u8(1);
        expected.write_str("f");
        expected.write_u8(0);
        expected.write_u8(1);
        expected.write_u32(9);
        expected.write_str("");
        expected.write_str("");
        expected.write_u64(0xAB);
        expected.write_str("");
        assert_eq!(bytes, expected.into_vec());
    }
}
