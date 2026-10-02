//! Id stability: every port id and method id of the standard ports, as hard-coded hex, as
//! computed by `undra_meta::ids`, as the macros registered them in the schema, and as the
//! Kotlin runtime hard-codes them.
//!
//! The hex literals are the point: renaming a trait or a method, or reordering nothing but
//! renaming, changes an id and breaks the three platform runtimes. This test fails first.

use undra_meta::ids::{fnv1a32, port_id, port_method_id};
use undra_meta::{PortKind, collect_schema};
use undra_ports::{
    Clock, Connectivity, Diagnostics, Fs, Http, Kv, Lifecycle, Log, Rng, SecureStore, Timer, fakes,
};
use undra_runtime::Port;

/// `(trait, port id, kind, [(method, method id)])`, in the declaration order of SPEC 8.
type PortRow = (&'static str, u32, PortKind, &'static [(&'static str, u32)]);

const TABLE: &[PortRow] = &[
    (
        "Clock",
        0xcd99_c48e,
        PortKind::Sync,
        &[("now_ms", 0xccc9_4d90), ("monotonic_ns", 0x2cb2_b4bf)],
    ),
    ("Rng", 0x2513_5bf5, PortKind::Sync, &[("fill", 0x2832_b8ed)]),
    ("Log", 0x575f_f24a, PortKind::Sync, &[("log", 0xd49d_5649)]),
    (
        "Http",
        0x1ebe_b908,
        PortKind::Async,
        &[("request", 0x6b14_df26)],
    ),
    (
        "Kv",
        0x5389_110d,
        PortKind::Async,
        &[
            ("get", 0xf050_bb1a),
            ("set", 0x6242_7856),
            ("delete", 0x60a3_86b9),
            ("list", 0x32f1_d03a),
        ],
    ),
    (
        "SecureStore",
        0xc01f_5bea,
        PortKind::Async,
        &[
            ("get", 0x5703_6f6f),
            ("set", 0xe91e_017b),
            ("delete", 0xd57d_b4e2),
            ("list", 0xf5ba_b8c9),
        ],
    ),
    (
        "Fs",
        0x4ea3_4cab,
        PortKind::Async,
        &[
            ("read", 0x01fd_be44),
            ("write", 0x6b70_d47f),
            ("delete", 0xa90a_826b),
            ("list", 0x4fba_8678),
        ],
    ),
    (
        "Timer",
        0x00c2_cdd9,
        PortKind::Sync,
        &[("set", 0x923a_766c)],
    ),
    (
        "Connectivity",
        0x1fef_f6ff,
        PortKind::Event,
        &[("changed", 0xb4f2_a010)],
    ),
    (
        "Lifecycle",
        0x81c0_afd4,
        PortKind::Event,
        &[("changed", 0x0bc8_2569)],
    ),
    (
        "Diagnostics",
        0xab68_cd7c,
        PortKind::Sync,
        &[("panicked", 0xbd14_7e2e)],
    ),
];

/// The opt-in ports (ADR-047, ADR-048), each behind the cargo feature named first. The platform
/// runtimes hard-code their ids whatever a core enables.
const OPT_IN: &[(&str, PortRow)] = &[
    (
        "websocket",
        (
            "WebSocket",
            0x7388_b95f,
            PortKind::Async,
            &[
                ("connect", 0x8347_7638),
                ("send", 0x117b_2158),
                ("receive", 0x8f31_f08f),
                ("close", 0x6015_4b86),
            ],
        ),
    ),
    (
        "sse",
        (
            "Sse",
            0x75d2_ef19,
            PortKind::Async,
            &[
                ("open", 0xc003_3c14),
                ("next", 0x4035_cbed),
                ("close", 0x5bfe_2c88),
            ],
        ),
    ),
    (
        "db",
        (
            "Db",
            0x559e_da82,
            PortKind::Async,
            &[
                ("open", 0xee6f_26db),
                ("execute", 0xffac_2f0a),
                ("query", 0x3a4d_eefd),
                ("begin", 0xae2b_a428),
                ("commit", 0xf866_d5ae),
                ("rollback", 0x3e7b_24b3),
                ("close", 0xde3d_c7ed),
            ],
        ),
    ),
];

/// Whether this test binary was built with `feature`.
fn enabled(feature: &str) -> bool {
    (feature == "websocket" && cfg!(feature = "websocket"))
        || (feature == "sse" && cfg!(feature = "sse"))
        || (feature == "db" && cfg!(feature = "db"))
}

/// Every port row: the ten, then the three opt-in ones.
fn all_rows() -> impl Iterator<Item = &'static PortRow> {
    TABLE.iter().chain(OPT_IN.iter().map(|(_, row)| row))
}

/// The rows the schema of this build registers.
fn registered_rows() -> Vec<&'static PortRow> {
    TABLE
        .iter()
        .chain(
            OPT_IN
                .iter()
                .filter(|(f, _)| enabled(f))
                .map(|(_, row)| row),
        )
        .collect()
}

#[test]
fn port_ids_are_the_hard_coded_values() {
    assert_eq!(<dyn Clock as Port>::PORT_ID, 0xcd99_c48e);
    assert_eq!(<dyn Rng as Port>::PORT_ID, 0x2513_5bf5);
    assert_eq!(<dyn Log as Port>::PORT_ID, 0x575f_f24a);
    assert_eq!(<dyn Http as Port>::PORT_ID, 0x1ebe_b908);
    assert_eq!(<dyn Kv as Port>::PORT_ID, 0x5389_110d);
    assert_eq!(<dyn SecureStore as Port>::PORT_ID, 0xc01f_5bea);
    assert_eq!(<dyn Fs as Port>::PORT_ID, 0x4ea3_4cab);
    assert_eq!(<dyn Timer as Port>::PORT_ID, 0x00c2_cdd9);
    assert_eq!(<dyn Connectivity as Port>::PORT_ID, 0x1fef_f6ff);
    assert_eq!(<dyn Lifecycle as Port>::PORT_ID, 0x81c0_afd4);
    assert_eq!(<dyn Diagnostics as Port>::PORT_ID, 0xab68_cd7c);
}

#[test]
fn port_names_and_kinds_are_the_traits() {
    assert_eq!(<dyn Clock as Port>::NAME, "Clock");
    assert_eq!(<dyn SecureStore as Port>::NAME, "SecureStore");
    assert_eq!(<dyn Clock as Port>::KIND, PortKind::Sync);
    assert_eq!(<dyn Rng as Port>::KIND, PortKind::Sync);
    assert_eq!(<dyn Log as Port>::KIND, PortKind::Sync);
    assert_eq!(<dyn Timer as Port>::KIND, PortKind::Sync);
    assert_eq!(<dyn Http as Port>::KIND, PortKind::Async);
    assert_eq!(<dyn Kv as Port>::KIND, PortKind::Async);
    assert_eq!(<dyn SecureStore as Port>::KIND, PortKind::Async);
    assert_eq!(<dyn Fs as Port>::KIND, PortKind::Async);
    assert_eq!(<dyn Connectivity as Port>::KIND, PortKind::Event);
    assert_eq!(<dyn Lifecycle as Port>::KIND, PortKind::Event);
    assert_eq!(<dyn Diagnostics as Port>::KIND, PortKind::Sync);
}

#[test]
fn every_id_is_the_fnv1a32_of_its_name() {
    for (name, expected_port, _, methods) in all_rows() {
        assert_eq!(
            fnv1a32(&format!("port.{name}")),
            *expected_port,
            "port.{name}"
        );
        assert_eq!(port_id(name), *expected_port, "{name}");
        for (method, expected) in *methods {
            assert_eq!(
                fnv1a32(&format!("{name}.{method}")),
                *expected,
                "{name}.{method}"
            );
            assert_eq!(port_method_id(name, method), *expected, "{name}.{method}");
        }
    }
}

#[test]
fn ids_are_unique_across_the_standard_ports() {
    let mut ports: Vec<u32> = all_rows().map(|row| row.1).collect();
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(ports.len(), TABLE.len() + OPT_IN.len(), "port ids collide");
    // Method ids only need to be unique within a port, but the shared ones (get/set/delete/list)
    // must differ between Kv and SecureStore because the trait name is part of the hash.
    let kv: Vec<u32> = TABLE[4].3.iter().map(|m| m.1).collect();
    let secure: Vec<u32> = TABLE[5].3.iter().map(|m| m.1).collect();
    assert!(kv.iter().all(|id| !secure.contains(id)));
    for (name, _, _, methods) in all_rows() {
        let mut ids: Vec<u32> = methods.iter().map(|m| m.1).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), methods.len(), "{name}: method ids collide");
    }
}

#[test]
fn the_registered_schema_agrees_with_the_table() {
    // Referencing the fakes links the crate's registrations into this test binary.
    let _ = fakes::Fakes::new();
    let schema = collect_schema("undra-ports");
    let rows = registered_rows();
    assert_eq!(schema.ports.len(), rows.len(), "unexpected port count");
    for (name, expected_port, kind, methods) in rows {
        let port = schema
            .ports
            .iter()
            .find(|p| p.name == *name)
            .unwrap_or_else(|| panic!("port {name} is not registered"));
        assert_eq!(port.port_id, *expected_port, "{name}");
        assert_eq!(port.kind, *kind, "{name}");
        assert_eq!(port.methods.len(), methods.len(), "{name}: method count");
        for (method, expected) in *methods {
            let def = port
                .methods
                .iter()
                .find(|m| m.name == *method)
                .unwrap_or_else(|| panic!("{name}.{method} is not registered"));
            assert_eq!(def.method_id, *expected, "{name}.{method}");
            assert_eq!(
                def.is_async,
                *kind == PortKind::Async,
                "{name}.{method}: is_async"
            );
            assert!(!def.takes_ctx);
        }
    }
}

// ---- parity with the Kotlin runtime, which hard-codes the same ids -----------------------------

/// Reads `runtimes/kotlin/.../StandardPorts.kt`. `None` when this crate is built outside the
/// workspace (a packaged crate has no runtimes next to it).
fn kotlin_standard_ports() -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../runtimes/kotlin/undra-runtime/runtime/src/main/kotlin/dev/undra/runtime/adapters/StandardPorts.kt",
    );
    std::fs::read_to_string(path).ok()
}

/// The literal of `const val <name>: UInt = 0x........u` inside `object <object>`.
fn kotlin_constant(source: &str, object: &str, name: &str) -> u32 {
    let start = source
        .find(&format!("public object {object} {{"))
        .unwrap_or_else(|| panic!("StandardPorts.kt has no object {object}"));
    let body = &source[start..];
    let end = body
        .find("\n    }\n")
        .unwrap_or_else(|| panic!("object {object} is not terminated"));
    let body = &body[..end];
    let needle = format!("const val {name}: UInt = 0x");
    let at = body
        .find(&needle)
        .unwrap_or_else(|| panic!("{object}.{name} is missing in StandardPorts.kt"));
    let digits: String = body[at + needle.len()..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    u32::from_str_radix(&digits, 16).unwrap_or_else(|e| panic!("{object}.{name}: {e}"))
}

#[test]
fn the_kotlin_runtime_hard_codes_the_same_ids() {
    let Some(source) = kotlin_standard_ports() else {
        eprintln!("runtimes/ not found next to this crate; skipping the Kotlin parity check");
        return;
    };
    for (name, expected_port, _, methods) in all_rows() {
        assert_eq!(
            kotlin_constant(&source, name, "PORT_ID"),
            *expected_port,
            "Kotlin {name}.PORT_ID"
        );
        for (method, expected) in *methods {
            let constant = method.to_ascii_uppercase();
            assert_eq!(
                kotlin_constant(&source, name, &constant),
                *expected,
                "Kotlin {name}.{constant}"
            );
        }
    }
}
