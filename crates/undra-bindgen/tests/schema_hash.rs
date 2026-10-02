//! ADR-031 decision 6: `SignalDef.no_coalesce` is serialized only when it is `true`, so adding the
//! field changed the schema hash of no schema that does not use it. The hashes below are the ones
//! the golden schemas had before the field existed (their generated `UndraIds` carried them); with
//! every `no_coalesce` flag cleared, each golden schema must still hash to exactly that.

mod common;

/// `(case, schema hash before ADR-031)`.
const HASHES_BEFORE_NO_COALESCE: &[(&str, u64)] = &[
    ("records", 0x3e31_4305_379c_f9a4),
    ("enums", 0x2533_f604_43b3_d23b),
    ("errors", 0x8005_bee6_ac51_e549),
    ("objects", 0x892e_6143_2c31_cead),
    ("stores", 0x96c5_53ec_019d_f03c),
    ("ports", 0xde22_6901_32b0_e949),
    ("queries", 0xbfc1_afa6_30b0_b63d),
    ("full", 0x3b1b_d106_2551_c158),
];

/// The cases written after ADR-031: they never had a hash without the field, so there is nothing
/// to compare them with. `stdlib` is built from the live registrations of `undra-ports`, whose
/// standard surface ADR-049 changed on purpose (`StorageError`, the storage ports' signatures,
/// two `FsError` variants): it had `0x6b46_38c4_a5e3_4313` before, and no longer has a hash from
/// before the field to compare with.
/// `object_graph` and `callbacks` (ADR-040, ADR-041) are new: they use the types and port kind that
/// no earlier schema had.
const CASES_AFTER_NO_COALESCE: &[&str] = &["stdlib", "recursive", "object_graph", "callbacks"];

#[test]
fn every_golden_case_is_listed() {
    let mut listed: Vec<&str> = HASHES_BEFORE_NO_COALESCE
        .iter()
        .map(|(case, _)| *case)
        .collect();
    listed.extend(CASES_AFTER_NO_COALESCE);
    let mut cases = common::CASES.to_vec();
    listed.sort_unstable();
    cases.sort_unstable();
    assert_eq!(listed, cases);
}

/// The `stdlib` case as it was before ports-v2 (ADR-047, ADR-048) added the opt-in standard items
/// (registered here because the dev-dependency enables `undra-ports`' features) and an app record
/// and three methods that refer to them.
fn without_ports_v2(mut schema: undra_meta::Schema) -> undra_meta::Schema {
    const OPT_IN: &[&str] = &[
        "WebSocket",
        "Sse",
        "Db",
        "WsOpened",
        "WsMessage",
        "WsError",
        "SseEvent",
        "SseError",
        "DbMigration",
        "DbOpened",
        "DbValue",
        "DbExecuted",
        "DbRows",
        "DbConstraint",
        "DbError",
        "Feed",
    ];
    schema
        .records
        .retain(|r| !OPT_IN.contains(&r.name.as_str()));
    schema.enums.retain(|e| !OPT_IN.contains(&e.name.as_str()));
    schema.ports.retain(|p| !OPT_IN.contains(&p.name.as_str()));
    for object in &mut schema.objects {
        object.methods.retain(|m| {
            !(object.name == "Syncer" && ["local", "listen", "push"].contains(&m.name.as_str()))
        });
    }
    schema
}

#[test]
fn a_schema_without_no_coalesce_hashes_as_before_the_field_existed() {
    for &(case, before) in HASHES_BEFORE_NO_COALESCE {
        let mut schema = common::case(case);
        if case == "stdlib" {
            schema = without_ports_v2(schema);
        }
        let mut flagged = 0;
        for store in schema.objects.iter_mut().filter_map(|o| o.store.as_mut()) {
            for signal in &mut store.signals {
                flagged += usize::from(signal.no_coalesce);
                signal.no_coalesce = false;
            }
        }
        assert!(
            !schema.canonical_json().contains("no_coalesce"),
            "{case}: a false flag is not serialized"
        );
        assert_eq!(
            schema.hash(),
            before,
            "{case}: the schema hash of a schema without no_coalesce signals changed"
        );
        if flagged > 0 {
            // The flag is part of the schema when it is set: the cores differ in what they deliver.
            assert_ne!(common::case(case).hash(), before, "{case}");
        }
        if case == "stdlib" {
            // ports-v2 is additive: its items are the whole difference.
            assert_ne!(
                common::case(case).hash(),
                before,
                "{case} has the opt-in items"
            );
        }
    }
}
