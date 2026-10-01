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
const CASES_AFTER_NO_COALESCE: &[&str] = &["stdlib", "recursive"];

#[test]
fn every_golden_case_is_listed() {
    let mut listed: Vec<&str> = HASHES_BEFORE_NO_COALESCE
        .iter()
        .map(|(case, _)| *case)
        .collect();
    listed.extend(CASES_AFTER_NO_COALESCE);
    assert_eq!(listed, common::CASES);
}

#[test]
fn a_schema_without_no_coalesce_hashes_as_before_the_field_existed() {
    for &(case, before) in HASHES_BEFORE_NO_COALESCE {
        let mut schema = common::case(case);
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
    }
}
