//! Encoding parity: the exact bytes of every record, enum and error of the standard ports.
//!
//! The Swift, Kotlin and TypeScript runtimes hand-write codecs for these types. The golden
//! vectors below are spelled out byte by byte (the same vectors the platform test suites
//! assert), so a reordered field, a renumbered variant or a changed width fails here before it
//! fails on a device.

use proptest::prelude::*;
use undra_ports::{
    AppState, BackgroundReport, FsError, Header, HttpError, HttpMethod, HttpRequest, HttpResponse,
    NetKind, PanicFrame, PanicReport, StorageError, encode_connectivity_changed_event,
    encode_lifecycle_changed_event,
};
use undra_wire::{Bytes, Decode, Encode, Writer};

/// `"0000 01000000 75"` to bytes; whitespace is ignored.
fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    assert_eq!(digits.len() % 2, 0, "odd number of hex digits in {text:?}");
    digits
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(pair, 16).unwrap_or_else(|_| panic!("bad hex {pair:?}"))
        })
        .collect()
}

/// Asserts that `value` encodes to exactly `expected`, that `expected` decodes back to `value`,
/// and that every strict prefix of `expected` (and `expected` plus a stray byte) is a decode
/// error rather than a panic or a wrong value.
fn assert_codec<T>(value: &T, expected: &str)
where
    T: Encode + Decode + PartialEq + std::fmt::Debug,
{
    let bytes = hex(expected);
    assert_eq!(value.encode_to_vec(), bytes, "encoding of {value:?}");
    assert_eq!(
        T::decode_exact(&bytes).as_ref(),
        Ok(value),
        "decoding {expected}"
    );
    for cut in 0..bytes.len() {
        assert!(
            T::decode_exact(&bytes[..cut]).is_err(),
            "a {cut}-byte prefix of {expected} must not decode"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(
        T::decode_exact(&trailing).is_err(),
        "trailing bytes after {expected} must be rejected"
    );
}

// ---- enums -------------------------------------------------------------------------------------

#[test]
fn http_method_is_a_u16_index_in_declaration_order() {
    let all = [
        (HttpMethod::Get, "0000"),
        (HttpMethod::Post, "0100"),
        (HttpMethod::Put, "0200"),
        (HttpMethod::Delete, "0300"),
        (HttpMethod::Patch, "0400"),
        (HttpMethod::Head, "0500"),
        (HttpMethod::Options, "0600"),
    ];
    for (method, bytes) in all {
        assert_codec(&method, bytes);
    }
    assert!(HttpMethod::decode_exact(&hex("0700")).is_err());
}

#[test]
fn net_kind_is_a_u16_index_in_declaration_order() {
    let all = [
        (NetKind::Wifi, "0000"),
        (NetKind::Cellular, "0100"),
        (NetKind::Wired, "0200"),
        (NetKind::Unknown, "0300"),
        (NetKind::None, "0400"),
    ];
    for (kind, bytes) in all {
        assert_codec(&kind, bytes);
    }
    assert!(NetKind::decode_exact(&hex("0500")).is_err());
}

#[test]
fn app_state_is_active_inactive_background() {
    assert_codec(&AppState::Active, "0000");
    assert_codec(&AppState::Inactive, "0100");
    assert_codec(&AppState::Background, "0200");
    assert!(AppState::decode_exact(&hex("0300")).is_err());
}

// ---- errors ------------------------------------------------------------------------------------

#[test]
fn http_error_variants() {
    assert_codec(&HttpError::Network("x".into()), "0000 01000000 78");
    assert_codec(&HttpError::Timeout, "0100");
    assert_codec(&HttpError::Cancelled, "0200");
    assert_codec(&HttpError::InvalidUrl("u".into()), "0300 01000000 75");
    assert!(HttpError::decode_exact(&hex("0400")).is_err());
}

#[test]
fn fs_error_variants() {
    assert_codec(&FsError::NotFound, "0000");
    assert_codec(&FsError::Denied, "0100");
    assert_codec(&FsError::Io("e".into()), "0200 01000000 65");
    assert_codec(&FsError::Full, "0300");
    assert_codec(&FsError::Unavailable("e".into()), "0400 01000000 65");
    assert!(FsError::decode_exact(&hex("0500")).is_err());
}

#[test]
fn storage_error_variants() {
    assert_codec(&StorageError::Unavailable("e".into()), "0000 01000000 65");
    assert_codec(&StorageError::Full, "0100");
    assert_codec(&StorageError::Locked, "0200");
    assert_codec(&StorageError::Corrupt("e".into()), "0300 01000000 65");
    assert_codec(&StorageError::Io("e".into()), "0400 01000000 65");
    assert!(StorageError::decode_exact(&hex("0500")).is_err());
}

// ---- records -----------------------------------------------------------------------------------

#[test]
fn header_is_name_then_value() {
    assert_codec(&Header::new("k", "v"), "01000000 6b 01000000 76");
    assert_codec(&Header::new("", ""), "00000000 00000000");
}

#[test]
fn http_request_is_method_url_headers_body_timeout() {
    let minimal = HttpRequest {
        method: HttpMethod::Get,
        url: "u".into(),
        headers: vec![Header::new("a", "b")],
        body: None,
        timeout_ms: None,
    };
    assert_codec(
        &minimal,
        "0000 01000000 75 01000000 01000000 61 01000000 62 00 00",
    );
    let full = HttpRequest {
        method: HttpMethod::Patch,
        url: "u".into(),
        headers: vec![],
        body: Some(Bytes(vec![1, 2])),
        timeout_ms: Some(5000),
    };
    assert_codec(
        &full,
        "0400 01000000 75 00000000 01 02000000 0102 01 88130000",
    );
    // An empty body is `Some`, not `None`.
    let empty_body = HttpRequest {
        method: HttpMethod::Post,
        url: String::new(),
        headers: vec![],
        body: Some(Bytes(vec![])),
        timeout_ms: Some(0),
    };
    assert_codec(
        &empty_body,
        "0100 00000000 00000000 01 00000000 01 00000000",
    );
}

#[test]
fn http_response_is_status_headers_body() {
    assert_codec(
        &HttpResponse::new(200, vec![1, 2, 3]),
        "c800 00000000 03000000 010203",
    );
    assert_codec(
        &HttpResponse::new(404, Vec::new()).with_header("k", ""),
        "9401 01000000 01000000 6b 00000000 00000000",
    );
    assert_codec(
        &HttpResponse::new(u16::MAX, Vec::new()),
        "ffff 00000000 00000000",
    );
}

// ---- ADR-046 -----------------------------------------------------------------------------------

#[test]
fn panic_frame_is_address_symbol_file_line() {
    assert_codec(
        &PanicFrame {
            address: 0x1122,
            symbol: None,
            file: None,
            line: None,
        },
        "2211000000000000 00 00 00",
    );
    assert_codec(
        &PanicFrame {
            address: 1,
            symbol: Some("f".into()),
            file: Some("a.rs".into()),
            line: Some(7),
        },
        "0100000000000000 01 01000000 66 01 04000000 612e7273 01 07000000",
    );
}

#[test]
fn panic_report_is_nine_fields_in_declaration_order() {
    let report = PanicReport {
        message: "m".into(),
        location: "l".into(),
        operation: "o".into(),
        thread: "t".into(),
        frames: vec![PanicFrame {
            address: 2,
            symbol: None,
            file: None,
            line: Some(1),
        }],
        namespace: "n".into(),
        core_version: "1.0".into(),
        schema_hash: 0x0102,
        image_id: "ab".into(),
    };
    assert_codec(
        &report,
        "01000000 6d 01000000 6c 01000000 6f 01000000 74 \
         01000000 0200000000000000 00 00 01 01000000 \
         01000000 6e 03000000 312e30 0201000000000000 02000000 6162",
    );
    assert_codec(
        &PanicReport::default(),
        "00000000 00000000 00000000 00000000 00000000 00000000 00000000 0000000000000000 00000000",
    );
}

#[test]
fn background_report_is_a_bool_and_three_counts() {
    assert_codec(
        &BackgroundReport {
            finished: true,
            replayed: 1,
            refetched: 2,
            still_pending: 3,
        },
        "01 01000000 02000000 03000000",
    );
    assert!(BackgroundReport::decode_exact(&hex("02 00000000 00000000 00000000")).is_err());
}

#[test]
fn strings_are_utf8_with_a_byte_length() {
    // "é" is two bytes, the wave emoji four: the prefix counts bytes, not characters.
    let header = Header::new("caf\u{e9}", "\u{1F30A}");
    assert_codec(&header, "05000000 636166c3a9 04000000 f09f8c8a");
}

#[test]
fn header_lists_keep_order_and_duplicates() {
    let response = HttpResponse::new(200, Vec::new())
        .with_header("Set-Cookie", "a=1")
        .with_header("Set-Cookie", "b=2");
    let bytes = response.encode_to_vec();
    assert_eq!(HttpResponse::decode_exact(&bytes), Ok(response));
    assert_eq!(&bytes[..2 + 4], hex("c800 02000000").as_slice());
}

// ---- event payloads ----------------------------------------------------------------------------

#[test]
fn event_payloads_are_the_arguments_in_order() {
    // `Connectivity.changed(online: bool, kind: NetKind)`, as `emitConnectivity` /
    // `ConnectivityAdapter.encodeChanged` write it.
    assert_eq!(
        encode_connectivity_changed_event(true, NetKind::Cellular),
        hex("01 0100")
    );
    assert_eq!(
        encode_connectivity_changed_event(false, NetKind::None),
        hex("00 0400")
    );
    // `Lifecycle.changed(state: AppState)`.
    assert_eq!(
        encode_lifecycle_changed_event(AppState::Inactive),
        hex("0100")
    );
}

// ---- properties --------------------------------------------------------------------------------

fn any_method() -> impl Strategy<Value = HttpMethod> {
    prop_oneof![
        Just(HttpMethod::Get),
        Just(HttpMethod::Post),
        Just(HttpMethod::Put),
        Just(HttpMethod::Delete),
        Just(HttpMethod::Patch),
        Just(HttpMethod::Head),
        Just(HttpMethod::Options),
    ]
}

fn any_header() -> impl Strategy<Value = Header> {
    (any::<String>(), any::<String>()).prop_map(|(name, value)| Header { name, value })
}

fn any_bytes() -> impl Strategy<Value = Bytes> {
    prop::collection::vec(any::<u8>(), 0..96).prop_map(Bytes)
}

fn any_request() -> impl Strategy<Value = HttpRequest> {
    (
        any_method(),
        any::<String>(),
        prop::collection::vec(any_header(), 0..5),
        prop::option::of(any_bytes()),
        prop::option::of(any::<u32>()),
    )
        .prop_map(|(method, url, headers, body, timeout_ms)| HttpRequest {
            method,
            url,
            headers,
            body,
            timeout_ms,
        })
}

fn any_response() -> impl Strategy<Value = HttpResponse> {
    (
        any::<u16>(),
        prop::collection::vec(any_header(), 0..5),
        any_bytes(),
    )
        .prop_map(|(status, headers, body)| HttpResponse {
            status,
            headers,
            body,
        })
}

fn any_http_error() -> impl Strategy<Value = HttpError> {
    prop_oneof![
        any::<String>().prop_map(HttpError::Network),
        Just(HttpError::Timeout),
        Just(HttpError::Cancelled),
        any::<String>().prop_map(HttpError::InvalidUrl),
    ]
}

fn any_fs_error() -> impl Strategy<Value = FsError> {
    prop_oneof![
        Just(FsError::NotFound),
        Just(FsError::Denied),
        any::<String>().prop_map(FsError::Io),
        Just(FsError::Full),
        any::<String>().prop_map(FsError::Unavailable),
    ]
}

fn any_storage_error() -> impl Strategy<Value = StorageError> {
    prop_oneof![
        any::<String>().prop_map(StorageError::Unavailable),
        Just(StorageError::Full),
        Just(StorageError::Locked),
        any::<String>().prop_map(StorageError::Corrupt),
        any::<String>().prop_map(StorageError::Io),
    ]
}

fn any_net_kind() -> impl Strategy<Value = NetKind> {
    prop_oneof![
        Just(NetKind::Wifi),
        Just(NetKind::Cellular),
        Just(NetKind::Wired),
        Just(NetKind::Unknown),
        Just(NetKind::None),
    ]
}

fn any_app_state() -> impl Strategy<Value = AppState> {
    prop_oneof![
        Just(AppState::Active),
        Just(AppState::Inactive),
        Just(AppState::Background),
    ]
}

/// The encoded size of a record is what its fields say it is: nothing is padded or aligned.
fn header_len(h: &Header) -> usize {
    4 + h.name.len() + 4 + h.value.len()
}

proptest! {
    #[test]
    fn requests_round_trip(request in any_request()) {
        let bytes = request.encode_to_vec();
        prop_assert_eq!(HttpRequest::decode_exact(&bytes), Ok(request.clone()));
        let expected = 2
            + 4 + request.url.len()
            + 4 + request.headers.iter().map(header_len).sum::<usize>()
            + 1 + request.body.as_ref().map_or(0, |b| 4 + b.len())
            + 1 + request.timeout_ms.map_or(0, |_| 4);
        prop_assert_eq!(bytes.len(), expected);
    }

    #[test]
    fn responses_round_trip(response in any_response()) {
        let bytes = response.encode_to_vec();
        prop_assert_eq!(HttpResponse::decode_exact(&bytes), Ok(response.clone()));
        let expected = 2
            + 4 + response.headers.iter().map(header_len).sum::<usize>()
            + 4 + response.body.len();
        prop_assert_eq!(bytes.len(), expected);
    }

    #[test]
    fn errors_and_enums_round_trip(
        http in any_http_error(),
        fs in any_fs_error(),
        storage in any_storage_error(),
        kind in any_net_kind(),
        state in any_app_state(),
        method in any_method(),
        header in any_header(),
    ) {
        prop_assert_eq!(HttpError::decode_exact(&http.encode_to_vec()), Ok(http));
        prop_assert_eq!(FsError::decode_exact(&fs.encode_to_vec()), Ok(fs));
        prop_assert_eq!(StorageError::decode_exact(&storage.encode_to_vec()), Ok(storage));
        prop_assert_eq!(NetKind::decode_exact(&kind.encode_to_vec()), Ok(kind));
        prop_assert_eq!(AppState::decode_exact(&state.encode_to_vec()), Ok(state));
        prop_assert_eq!(HttpMethod::decode_exact(&method.encode_to_vec()), Ok(method));
        prop_assert_eq!(Header::decode_exact(&header.encode_to_vec()), Ok(header));
    }

    #[test]
    fn strict_prefixes_of_a_request_never_decode(request in any_request()) {
        let bytes = request.encode_to_vec();
        // A sample of cut points keeps the case fast; `assert_codec` covers every cut for the
        // golden vectors.
        for cut in [0, 1, bytes.len() / 3, bytes.len() / 2, bytes.len().saturating_sub(1)] {
            if cut < bytes.len() {
                prop_assert!(HttpRequest::decode_exact(&bytes[..cut]).is_err());
            }
        }
    }

    #[test]
    fn random_bytes_never_panic_a_decoder(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        let _ = HttpRequest::decode_exact(&bytes);
        let _ = HttpResponse::decode_exact(&bytes);
        let _ = HttpError::decode_exact(&bytes);
        let _ = FsError::decode_exact(&bytes);
        let _ = StorageError::decode_exact(&bytes);
        let _ = HttpMethod::decode_exact(&bytes);
        let _ = NetKind::decode_exact(&bytes);
        let _ = AppState::decode_exact(&bytes);
        let _ = Header::decode_exact(&bytes);
    }

    #[test]
    fn event_encoders_match_encoding_the_arguments(online in any::<bool>(), kind in any_net_kind(), state in any_app_state()) {
        let mut w = Writer::new();
        online.encode(&mut w);
        kind.encode(&mut w);
        prop_assert_eq!(encode_connectivity_changed_event(online, kind), w.into_vec());
        prop_assert_eq!(encode_lifecycle_changed_event(state), state.encode_to_vec());
    }
}

// ---- parity with the platform runtimes' sources ------------------------------------------------
//
// The runtimes number the variants themselves. These tests read their sources (when the crate is
// built inside the workspace) and compare the numbering with the Rust declaration order, so a
// renumbering on either side is caught even if nobody updated the golden vectors.

fn runtime_source(relative: &str) -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtimes")
        .join(relative);
    std::fs::read_to_string(path).ok()
}

const KOTLIN_RECORDS: &str =
    "kotlin/undra-runtime/runtime/src/main/kotlin/dev/undra/runtime/adapters/StandardRecords.kt";
const SWIFT_RECORDS: &str = "swift/UndraRuntime/Sources/UndraRuntime/Core/StandardRecords.swift";
const TS_TYPES: &str = "ts/@undra/runtime/src/adapters/types.ts";
/// `NetKind` and `AppState` are defined with the host events (ADR-052's amendment of 2026-10-02): types.ts re-exports them.
const TS_EVENTS: &str = "ts/@undra/runtime/src/adapters/events.ts";
const TS_CODECS: &str = "ts/@undra/runtime/src/adapters/codecs.ts";

/// `(variant name, index)` of a type as `undra-meta` registered it.
fn rust_variants(type_name: &str) -> Vec<(String, u16)> {
    // Referencing a registered type links the crate's registrations into this test binary.
    let _ = HttpMethod::Get;
    let schema = undra_meta::collect_schema("undra-ports");
    let def = schema
        .enums
        .iter()
        .find(|e| e.name == type_name)
        .unwrap_or_else(|| panic!("{type_name} is not registered"));
    def.variants
        .iter()
        .map(|v| (v.name.clone(), v.index))
        .collect()
}

/// The lines of the block that starts at the first line containing `header` and ends at the next
/// line that starts in column 0 with `}` (the closing brace of a top-level declaration).
fn block<'a>(source: &'a str, header: &str) -> Vec<&'a str> {
    let mut lines = source
        .lines()
        .skip_while(|line| !line.contains(header))
        .peekable();
    assert!(lines.peek().is_some(), "no line contains {header:?}");
    let mut out = Vec::new();
    for line in lines {
        out.push(line);
        if line.starts_with('}') {
            break;
        }
    }
    out
}

/// Normalises a variant name for comparison across languages: `INVALID_URL`, `invalidUrl` and
/// `InvalidUrl` all become `invalidurl`.
fn norm(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

fn rust_normalised(type_name: &str) -> Vec<(String, u16)> {
    rust_variants(type_name)
        .into_iter()
        .map(|(name, index)| (norm(&name), index))
        .collect()
}

/// Kotlin unit enum entries: `    GET(0u),`.
fn kotlin_enum(source: &str, header: &str) -> Vec<(String, u16)> {
    block(source, header)
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            let (name, rest) = line.split_once('(')?;
            let index = rest
                .strip_suffix("u),")
                .or_else(|| rest.strip_suffix("u)"))?;
            let index = index.parse().ok()?;
            name.chars()
                .all(|c| c.is_ascii_uppercase() || c == '_')
                .then(|| (norm(name), index))
        })
        .collect()
}

/// Swift enum cases: `    case get = 0`.
fn swift_enum(source: &str, header: &str) -> Vec<(String, u16)> {
    block(source, header)
        .iter()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("case ")?;
            let (name, index) = rest.split_once(" = ")?;
            Some((norm(name), index.trim().parse().ok()?))
        })
        .collect()
}

/// Kotlin decode arms: `                1 -> Timeout`.
fn kotlin_error(source: &str, header: &str) -> Vec<(String, u16)> {
    block(source, header)
        .iter()
        .filter_map(|line| {
            let (index, rest) = line.trim().split_once(" -> ")?;
            let index: u16 = index.parse().ok()?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            Some((norm(&name), index))
        })
        .collect()
}

/// Swift decode cases: `        case 1:` followed by `            return .timeout`.
fn swift_error(source: &str, header: &str) -> Vec<(String, u16)> {
    let lines = block(source, header);
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(index) = line
            .trim()
            .strip_prefix("case ")
            .and_then(|r| r.strip_suffix(':'))
            .and_then(|r| r.parse::<u16>().ok())
        else {
            continue;
        };
        // The variant is named by the first `return .name` after the case label.
        if let Some(name) = lines[i + 1..]
            .iter()
            .find_map(|l| l.trim().strip_prefix("return ."))
        {
            let name: String = name.chars().take_while(|c| c.is_alphanumeric()).collect();
            out.push((norm(&name), index));
        }
    }
    out
}

/// TypeScript decode cases: `      case 1:` followed by `        return new HttpError.Timeout();`.
fn ts_error(source: &str, header: &str, class: &str) -> Vec<(String, u16)> {
    let lines = block(source, header);
    let mut out = Vec::new();
    let marker = format!("new {class}.");
    for (i, line) in lines.iter().enumerate() {
        let Some(index) = line
            .trim()
            .strip_prefix("case ")
            .and_then(|r| r.strip_suffix(':'))
            .and_then(|r| r.parse::<u16>().ok())
        else {
            continue;
        };
        if let Some(rest) = lines[i + 1..]
            .iter()
            .find_map(|l| l.split_once(&marker).map(|(_, rest)| rest))
        {
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
            out.push((norm(&name), index));
        }
    }
    out
}

/// A TypeScript `as const` list: `export const HTTP_METHODS = ["get", "post"] as const;`.
fn ts_list(source: &str, constant: &str) -> Vec<(String, u16)> {
    let line = source
        .lines()
        .find(|l| l.contains(&format!("export const {constant} =")))
        .unwrap_or_else(|| panic!("no {constant} in types.ts"));
    let inner = line.split_once('[').unwrap().1.split_once(']').unwrap().0;
    inner
        .split(',')
        .map(|item| item.trim().trim_matches('"'))
        .enumerate()
        .map(|(i, name)| (norm(name), u16::try_from(i).unwrap()))
        .collect()
}

/// Asserts that the runtime's numbering equals Rust's, treating `renames` (`(rust, runtime)`
/// normalised spellings) as the same variant.
fn assert_same(what: &str, runtime: Vec<(String, u16)>, rust: &str, renames: &[(&str, &str)]) {
    let expected: Vec<(String, u16)> = rust_normalised(rust)
        .into_iter()
        .map(|(name, index)| {
            let name = renames
                .iter()
                .find(|(from, _)| *from == name)
                .map_or(name, |(_, to)| (*to).to_owned());
            (name, index)
        })
        .collect();
    assert!(
        !runtime.is_empty(),
        "{what}: parsed no variants (format changed?)"
    );
    assert_eq!(
        runtime, expected,
        "{what} disagrees with undra-ports::{rust}"
    );
}

#[test]
fn kotlin_numbering_matches() {
    let Some(kotlin) = runtime_source(KOTLIN_RECORDS) else {
        eprintln!("runtimes/ not found; skipping the Kotlin numbering check");
        return;
    };
    assert_same(
        "Kotlin HttpMethod",
        kotlin_enum(&kotlin, "public enum class HttpMethod"),
        "HttpMethod",
        &[],
    );
    assert_same(
        "Kotlin NetKind",
        kotlin_enum(&kotlin, "public enum class NetKind"),
        "NetKind",
        &[],
    );
    assert_same(
        "Kotlin AppState",
        kotlin_enum(&kotlin, "public enum class AppState"),
        "AppState",
        &[],
    );
    assert_same(
        "Kotlin HttpError",
        kotlin_error(&kotlin, "public sealed class HttpError"),
        "HttpError",
        &[],
    );
    assert_same(
        "Kotlin FsError",
        kotlin_error(&kotlin, "public sealed class FsError"),
        "FsError",
        &[],
    );
    assert_same(
        "Kotlin StorageError",
        kotlin_error(&kotlin, "public sealed class StorageError"),
        "StorageError",
        &[],
    );
}

#[test]
fn swift_numbering_matches() {
    let Some(swift) = runtime_source(SWIFT_RECORDS) else {
        eprintln!("runtimes/ not found; skipping the Swift numbering check");
        return;
    };
    assert_same(
        "Swift HttpMethod",
        swift_enum(&swift, "public enum HttpMethod"),
        "HttpMethod",
        &[],
    );
    // Swift spells `None` as `disconnected` so that `NetKind?` has no ambiguous `.none`.
    assert_same(
        "Swift NetKind",
        swift_enum(&swift, "public enum NetKind"),
        "NetKind",
        &[("none", "disconnected")],
    );
    assert_same(
        "Swift AppState",
        swift_enum(&swift, "public enum UndraAppState"),
        "AppState",
        &[],
    );
    assert_same(
        "Swift HttpError",
        swift_error(&swift, "public enum HttpError"),
        "HttpError",
        &[],
    );
    assert_same(
        "Swift FsError",
        swift_error(&swift, "public enum FsError"),
        "FsError",
        &[],
    );
    assert_same(
        "Swift StorageError",
        swift_error(&swift, "public enum StorageError"),
        "StorageError",
        &[],
    );
}

#[test]
fn typescript_numbering_matches() {
    let (Some(types), Some(events), Some(codecs)) = (
        runtime_source(TS_TYPES),
        runtime_source(TS_EVENTS),
        runtime_source(TS_CODECS),
    ) else {
        eprintln!("runtimes/ not found; skipping the TypeScript numbering check");
        return;
    };
    assert_same(
        "TS HTTP_METHODS",
        ts_list(&types, "HTTP_METHODS"),
        "HttpMethod",
        &[],
    );
    assert_same(
        "TS NET_KINDS",
        ts_list(&events, "NET_KINDS"),
        "NetKind",
        &[],
    );
    assert_same(
        "TS APP_STATES",
        ts_list(&events, "APP_STATES"),
        "AppState",
        &[],
    );
    assert_same(
        "TS HttpError",
        ts_error(&codecs, "export const HttpErrorCodec", "HttpError"),
        "HttpError",
        &[],
    );
    assert_same(
        "TS FsError",
        ts_error(&codecs, "export const FsErrorCodec", "FsError"),
        "FsError",
        &[],
    );
    assert_same(
        "TS StorageError",
        ts_error(&codecs, "export const StorageErrorCodec", "StorageError"),
        "StorageError",
        &[],
    );
}
