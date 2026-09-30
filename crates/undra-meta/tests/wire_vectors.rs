//! Checks the FNV helpers against the shared cross-language vectors in
//! `contract-tests/wire-vectors.json`.

use undra_meta::ids;

fn vectors() -> Vec<serde_json::Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contract-tests/wire-vectors.json"
    );
    let text = std::fs::read_to_string(path).expect("contract-tests/wire-vectors.json exists");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("vectors are valid JSON");
    doc["vectors"]
        .as_array()
        .expect("`vectors` is an array")
        .clone()
}

/// Extracts `arg` from a vector `type` of the form `fnv1a32("arg")`.
fn fnv_argument<'a>(ty: &'a str, function: &str) -> Option<&'a str> {
    ty.strip_prefix(function)?
        .strip_prefix("(\"")?
        .strip_suffix("\")")
}

#[test]
fn fnv_vectors_in_the_shared_file_match() {
    let mut checked32 = 0;
    let mut checked64 = 0;
    for vector in vectors() {
        let ty = vector["type"].as_str().unwrap_or_default();
        let value = vector["value"].as_str().unwrap_or_default();
        let hex = vector["hex"].as_str().unwrap_or_default();
        if let Some(arg) = fnv_argument(ty, "fnv1a32") {
            let expected: u32 = value.parse().expect("decimal u32 value");
            assert_eq!(ids::fnv1a32(arg), expected, "{}", vector["name"]);
            assert_eq!(hex_le(&ids::fnv1a32(arg).to_le_bytes()), hex);
            checked32 += 1;
        } else if let Some(arg) = fnv_argument(ty, "fnv1a64") {
            let expected: u64 = value.parse().expect("decimal u64 value");
            assert_eq!(ids::fnv1a64(arg.as_bytes()), expected, "{}", vector["name"]);
            assert_eq!(ids::fnv1a64_str(arg), expected);
            assert_eq!(hex_le(&ids::fnv1a64(arg.as_bytes()).to_le_bytes()), hex);
            checked64 += 1;
        }
    }
    assert!(checked32 >= 1, "no fnv1a32 vector found");
    assert!(checked64 >= 1, "no fnv1a64 vector found");
}

#[test]
fn the_two_named_vectors() {
    assert_eq!(ids::fnv1a32("Calculator.add"), 2_353_348_832);
    assert_eq!(ids::method_id("Calculator", "add"), 2_353_348_832);
    assert_eq!(ids::fnv1a64(b"undra"), 6_367_360_722_358_687_308);
}

fn hex_le(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
