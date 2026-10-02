//! The built-in `Clock`, `Rng` and `Log` port bindings of the wasm shell (SPEC 7, last
//! paragraph), written against a small [`Platform`] trait so their wire behaviour is unit-tested
//! on the host. On `wasm32` the platform is the `"undra"` import module (`now_ms`, `random`,
//! `log`); a web app needs no adapter code for these three ports.
//!
//! The shell offers each call to the host first: a JS runtime that registered its own `Clock`,
//! `Rng` or `Log` port answers it, and only an "unavailable" answer falls through to the
//! built-in, so all ports remain overridable.

use undra_runtime::undra_meta::ids;
use undra_runtime::undra_wire::payload::{PortReply, PortStatus};
use undra_runtime::undra_wire::{Reader, Writer};

/// Port ids (SPEC 8: `fnv1a32("port.<Trait>")`).
pub(crate) const CLOCK_PORT: u32 = ids::port_id("Clock");
pub(crate) const RNG_PORT: u32 = ids::port_id("Rng");
pub(crate) const LOG_PORT: u32 = ids::port_id("Log");
/// Method ids (SPEC 8: `fnv1a32("<Trait>.<method>")`).
pub(crate) const CLOCK_NOW_MS: u32 = ids::port_method_id("Clock", "now_ms");
pub(crate) const CLOCK_MONOTONIC_NS: u32 = ids::port_method_id("Clock", "monotonic_ns");
pub(crate) const RNG_FILL: u32 = ids::port_method_id("Rng", "fill");
pub(crate) const LOG_LOG: u32 = ids::port_method_id("Log", "log");

/// The most bytes one `Rng.fill` call may ask for (the TypeScript runtime's limit).
pub(crate) const MAX_RNG_BYTES: u32 = 1 << 24;

/// How many extra bytes `Rng.fill` asks the host for, to tell a host that filled the buffer from
/// one that could not (ADR-049 decision 2.5).
const CANARY_LEN: usize = 16;

/// What the extra bytes hold before the host fills them. A CSPRNG leaves them equal to this, or
/// all zero, with probability 2^-127; a host without one (no WebCrypto: its `random` import throws,
/// and the runtime's guard swallows the throw before it can cross the wasm boundary) leaves them
/// as they are, and one that "fills" with zeros zeroes them.
const CANARY: [u8; CANARY_LEN] = *b"undra-rng-canary";

/// What the built-ins need from the embedding environment.
pub(crate) trait Platform {
    /// Milliseconds since the Unix epoch (`Date.now()`).
    fn now_ms(&self) -> f64;
    /// Fills `out` with random bytes (`crypto.getRandomValues`). A host without a cryptographic
    /// random source must leave `out` untouched (the TypeScript `random` import throws, ADR-049);
    /// the built-in `Rng.fill` then answers "unavailable" instead of predictable bytes.
    fn random(&self, out: &mut [u8]);
    /// Hands a log record to the host: `bytes` is `target String, message String` (SPEC 7).
    fn log(&self, level: u8, bytes: &[u8]);
}

/// The last value `monotonic_ns` returned, so it never goes backwards even if `Date.now()` does.
#[derive(Default)]
pub(crate) struct Monotonic(core::sync::atomic::AtomicU64);

impl Monotonic {
    /// A clock that has not been read yet.
    #[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
    pub(crate) const fn new() -> Monotonic {
        Monotonic(core::sync::atomic::AtomicU64::new(0))
    }

    fn next(&self, now_ms: f64) -> u64 {
        // `as` saturates: a negative or NaN clock reads as 0.
        let ns = (now_ms * 1_000_000.0) as u64;
        let previous = self.0.fetch_max(ns, core::sync::atomic::Ordering::Relaxed);
        ns.max(previous)
    }
}

/// `target String, message String`: the bytes of the `log` import.
pub(crate) fn log_record(target: &str, message: &str) -> Vec<u8> {
    let mut w = Writer::with_capacity(8 + target.len() + message.len());
    w.write_str(target);
    w.write_str(message);
    w.into_vec()
}

fn reply(port_call_id: u32, status: PortStatus, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(5 + body.len());
    PortReply {
        port_call_id,
        status,
        body,
    }
    .encode(&mut w);
    w.into_vec()
}

/// Answers a call to one of the built-in ports with a complete `PortReply` payload, or `None`
/// when the call is not one of theirs (or its arguments do not decode), so the caller reports the
/// port as unavailable.
pub(crate) fn answer(
    platform: &impl Platform,
    monotonic: &Monotonic,
    port_id: u32,
    method_id: u32,
    port_call_id: u32,
    args: &[u8],
) -> Option<Vec<u8>> {
    let ok = |body: &[u8]| Some(reply(port_call_id, PortStatus::Ok, body));
    match (port_id, method_id) {
        (CLOCK_PORT, CLOCK_NOW_MS) => ok(&(platform.now_ms() as i64).to_le_bytes()),
        (CLOCK_PORT, CLOCK_MONOTONIC_NS) => ok(&monotonic.next(platform.now_ms()).to_le_bytes()),
        (RNG_PORT, RNG_FILL) => {
            let mut r = Reader::new(args);
            let len = r.read_u32().ok()?;
            r.finish().ok()?;
            if len > MAX_RNG_BYTES {
                return None;
            }
            // Randomness never degrades silently (ADR-049, PO-11): the host fills `len` bytes and a
            // canary; a canary it did not fill means it has no random source, and the port is
            // unavailable. `Rng` has no error channel, so the core's proxy then panics with E0062:
            // a loud failure instead of colliding idempotency keys.
            let len = len as usize;
            let mut bytes = vec![0; len + CANARY_LEN];
            bytes[len..].copy_from_slice(&CANARY);
            platform.random(&mut bytes);
            let canary = &bytes[len..];
            if canary == CANARY || canary.iter().all(|b| *b == 0) {
                // Said once per call, before the proxy's E0062 panic: the record names the cause.
                platform.log(
                    4,
                    &log_record(
                        "undra::rng",
                        "Rng.fill: no cryptographic random source (the web needs WebCrypto); the Rng port answers unavailable",
                    ),
                );
                return None;
            }
            bytes.truncate(len);
            let mut body = Writer::with_capacity(4 + bytes.len());
            body.write_bytes(&bytes);
            ok(body.as_slice())
        }
        (LOG_PORT, LOG_LOG) => {
            let mut r = Reader::new(args);
            let level = r.read_u8().ok()?;
            let record = r.read_rest();
            // Validate before forwarding: the host parses these bytes as two wire strings.
            let mut check = Reader::new(record);
            check.read_str().ok()?;
            check.read_str().ok()?;
            check.finish().ok()?;
            platform.log(level, record);
            ok(&[])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        now: Cell<f64>,
        logs: RefCell<Vec<(u8, Vec<u8>)>>,
    }

    impl Platform for Fake {
        fn now_ms(&self) -> f64 {
            self.now.get()
        }
        fn random(&self, out: &mut [u8]) {
            for (i, b) in out.iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(7).wrapping_add(1);
            }
        }
        fn log(&self, level: u8, bytes: &[u8]) {
            self.logs.borrow_mut().push((level, bytes.to_vec()));
        }
    }

    fn ask(fake: &Fake, mono: &Monotonic, port: u32, method: u32, args: &[u8]) -> Option<Vec<u8>> {
        answer(fake, mono, port, method, 9, args)
    }

    #[test]
    fn ids_match_the_typescript_adapters() {
        // Pinned against runtimes/ts/@undra/runtime/src/adapters (fnv1a32 of the names).
        assert_eq!(LOG_PORT, 0x575f_f24a);
        assert_eq!(LOG_LOG, 0xd49d_5649);
        assert_eq!(CLOCK_PORT, 0xcd99_c48e);
        assert_eq!(CLOCK_NOW_MS, 0xccc9_4d90);
        assert_eq!(CLOCK_MONOTONIC_NS, 0x2cb2_b4bf);
        assert_eq!(RNG_PORT, 0x2513_5bf5);
        assert_eq!(RNG_FILL, 0x2832_b8ed);
    }

    #[test]
    fn clock_now_is_an_i64_of_the_platform_millis() {
        let fake = Fake::default();
        fake.now.set(1_700_000_000_123.0);
        let mono = Monotonic::default();
        let got = ask(&fake, &mono, CLOCK_PORT, CLOCK_NOW_MS, &[]).unwrap();
        assert_eq!(&got[..5], [9, 0, 0, 0, 0]);
        assert_eq!(
            i64::from_le_bytes(got[5..].try_into().unwrap()),
            1_700_000_000_123
        );
    }

    #[test]
    fn monotonic_never_goes_backwards() {
        let fake = Fake::default();
        let mono = Monotonic::default();
        let read = |fake: &Fake, mono: &Monotonic| {
            let got = ask(fake, mono, CLOCK_PORT, CLOCK_MONOTONIC_NS, &[]).unwrap();
            u64::from_le_bytes(got[5..].try_into().unwrap())
        };
        fake.now.set(10.0);
        assert_eq!(read(&fake, &mono), 10_000_000);
        fake.now.set(4.0); // the wall clock jumped back
        assert_eq!(read(&fake, &mono), 10_000_000);
        fake.now.set(11.5);
        assert_eq!(read(&fake, &mono), 11_500_000);
        fake.now.set(f64::NAN);
        assert_eq!(read(&fake, &mono), 11_500_000);
    }

    #[test]
    fn rng_fill_returns_bytes_and_rejects_bad_requests() {
        let fake = Fake::default();
        let mono = Monotonic::default();
        let got = ask(&fake, &mono, RNG_PORT, RNG_FILL, &4_u32.to_le_bytes()).unwrap();
        assert_eq!(&got[..5], [9, 0, 0, 0, 0]);
        assert_eq!(&got[5..], [4, 0, 0, 0, 1, 8, 15, 22]);
        let empty = ask(&fake, &mono, RNG_PORT, RNG_FILL, &0_u32.to_le_bytes()).unwrap();
        assert_eq!(&empty[5..], [0, 0, 0, 0]);
        // Too large, truncated or with trailing bytes: not answered.
        assert!(
            ask(
                &fake,
                &mono,
                RNG_PORT,
                RNG_FILL,
                &(MAX_RNG_BYTES + 1).to_le_bytes()
            )
            .is_none()
        );
        assert!(ask(&fake, &mono, RNG_PORT, RNG_FILL, &[1, 0]).is_none());
        assert!(ask(&fake, &mono, RNG_PORT, RNG_FILL, &[1, 0, 0, 0, 0]).is_none());
    }

    /// A host without a random source (the TS `random` import throws and nothing is written) or
    /// one that writes zeros: `Rng.fill` is unavailable, never zeros with status 0 (ADR-049, PO-11).
    #[test]
    fn rng_fill_is_unavailable_when_the_host_has_no_random_source() {
        struct Untouched;
        impl Platform for Untouched {
            fn now_ms(&self) -> f64 {
                0.0
            }
            fn random(&self, _out: &mut [u8]) {}
            fn log(&self, _level: u8, _bytes: &[u8]) {}
        }
        struct Zeros;
        impl Platform for Zeros {
            fn now_ms(&self) -> f64 {
                0.0
            }
            fn random(&self, out: &mut [u8]) {
                out.fill(0);
            }
            fn log(&self, _level: u8, _bytes: &[u8]) {}
        }
        let mono = Monotonic::default();
        for len in [0_u32, 1, 16, 1000] {
            let args = len.to_le_bytes();
            assert!(
                answer(&Untouched, &mono, RNG_PORT, RNG_FILL, 1, &args).is_none(),
                "{len}"
            );
            assert!(
                answer(&Zeros, &mono, RNG_PORT, RNG_FILL, 1, &args).is_none(),
                "{len}"
            );
        }
    }

    #[test]
    fn log_forwards_level_and_the_two_strings() {
        let fake = Fake::default();
        let mono = Monotonic::default();
        let mut args = vec![3];
        args.extend(log_record("t", "hello"));
        let got = ask(&fake, &mono, LOG_PORT, LOG_LOG, &args).unwrap();
        assert_eq!(got, [9, 0, 0, 0, 0]);
        assert_eq!(*fake.logs.borrow(), [(3, log_record("t", "hello"))]);
        // Malformed strings are not forwarded.
        assert!(ask(&fake, &mono, LOG_PORT, LOG_LOG, &[3, 9, 9]).is_none());
        assert_eq!(fake.logs.borrow().len(), 1);
    }

    #[test]
    fn other_ports_and_methods_are_not_answered() {
        let fake = Fake::default();
        let mono = Monotonic::default();
        assert!(ask(&fake, &mono, 1, 2, &[]).is_none());
        assert!(ask(&fake, &mono, CLOCK_PORT, RNG_FILL, &[]).is_none());
    }
}
