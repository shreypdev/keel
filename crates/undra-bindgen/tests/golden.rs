//! Golden-file tests: every case under `tests/golden/<case>/` holds a
//! `schema.json` and the exact output of the three generators. Run with
//! `UPDATE_GOLDEN=1` to regenerate the schemas from `tests/common` and every
//! expected file (`UPDATE_GOLDEN_LANG=kotlin,ts` limits that to some languages).

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use undra_bindgen::GeneratedFile;
use undra_meta::Schema;

fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn updating() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// With `UPDATE_GOLDEN=1`, `UPDATE_GOLDEN_LANG=kotlin,ts` limits the regeneration to those languages
/// (`swift`, `kotlin`, `ts`); the others are neither written nor checked. Unset: all three.
fn skipped_while_updating(language: &str) -> bool {
    // `swift-observable-object` is the second Swift tree: it follows `swift`.
    let language = language
        .strip_suffix("-observable-object")
        .unwrap_or(language);
    updating()
        && std::env::var("UPDATE_GOLDEN_LANG")
            .is_ok_and(|only| !only.split(',').any(|l| l.trim() == language))
}

fn read_tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn first_difference(expected: &str, actual: &str) -> String {
    for (i, (e, a)) in expected.lines().zip(actual.lines()).enumerate() {
        if e != a {
            return format!("line {}:\n  expected: {e}\n  actual:   {a}", i + 1);
        }
    }
    format!(
        "one output is a prefix of the other (expected {} lines, actual {} lines)",
        expected.lines().count(),
        actual.lines().count()
    )
}

fn check_tree(case: &str, language: &str, files: &[GeneratedFile]) {
    let dir = golden_root().join(case).join(language);
    if skipped_while_updating(language) {
        return;
    }
    if updating() {
        let _ = fs::remove_dir_all(&dir);
        GeneratedFile::write_all(files, &dir).unwrap();
        return;
    }
    let expected = read_tree(&dir);
    let actual: BTreeMap<String, String> = files
        .iter()
        .map(|f| (f.path.clone(), f.contents.clone()))
        .collect();
    let missing: Vec<&String> = actual
        .keys()
        .filter(|k| !expected.contains_key(*k))
        .collect();
    let stale: Vec<&String> = expected
        .keys()
        .filter(|k| !actual.contains_key(*k))
        .collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "{case}/{language}: files differ from the golden set (not in golden: {missing:?}, no longer generated: {stale:?}); run UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden"
    );
    for (path, contents) in &actual {
        let want = &expected[path];
        assert!(
            want == contents,
            "{case}/{language}/{path} differs from the golden file: {}\nrun UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden to accept",
            first_difference(want, contents)
        );
    }
}

fn run(case: &str) {
    let dir = golden_root().join(case);
    let schema_path = dir.join("schema.json");
    if updating() {
        fs::create_dir_all(&dir).unwrap();
        fs::write(&schema_path, common::case(case).to_json_pretty() + "\n").unwrap();
    }
    let json = fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", schema_path.display()));
    let schema = Schema::from_json(&json).unwrap();
    let generator = common::generator_for(case, &schema);
    let show = |e: Vec<undra_bindgen::BindgenError>| {
        e.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    };
    check_tree(
        case,
        "swift",
        &generator
            .swift(&schema)
            .unwrap_or_else(|e| panic!("{}", show(e))),
    );
    // The iOS 15 / 16 mode (ADR-045): the cases with stores, query handles or a Duration are locked in it too.
    if common::FLOOR_CASES.contains(&case) {
        check_tree(
            case,
            "swift-observable-object",
            &common::floor_generator_for(case, &schema)
                .swift(&schema)
                .unwrap_or_else(|e| panic!("{}", show(e))),
        );
    }
    check_tree(
        case,
        "kotlin",
        &generator
            .kotlin(&schema)
            .unwrap_or_else(|e| panic!("{}", show(e))),
    );
    check_tree(
        case,
        "ts",
        &generator
            .typescript(&schema)
            .unwrap_or_else(|e| panic!("{}", show(e))),
    );
}

macro_rules! golden_cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                run(stringify!($name));
            }
        )*
    };
}

golden_cases!(
    object_graph,
    callbacks,
    records,
    enums,
    errors,
    objects,
    stores,
    ports,
    queries,
    full,
    stdlib,
    recursive,
    newtypes,
    generics,
    decimal,
    polling,
    infinite,
    lazy,
    generic_functions,
    generic_objects
);

#[test]
fn every_case_has_a_test() {
    // The macro above and `common::CASES` must list the same cases.
    let listed: Vec<&str> = common::CASES.to_vec();
    assert_eq!(
        listed,
        [
            "object_graph",
            "callbacks",
            "records",
            "enums",
            "errors",
            "objects",
            "stores",
            "ports",
            "queries",
            "full",
            "stdlib",
            "recursive",
            "newtypes",
            "generics",
            "decimal",
            "polling",
            "infinite",
            "lazy",
            "generic_functions",
            "generic_objects"
        ]
    );
}

#[test]
fn schema_json_matches_builders() {
    if updating() {
        return;
    }
    for case in common::CASES {
        let path = golden_root().join(case).join("schema.json");
        let json = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", path.display()));
        let on_disk = Schema::from_json(&json).unwrap();
        assert_eq!(
            on_disk,
            common::case(case),
            "{case}/schema.json is out of date; run UPDATE_GOLDEN=1"
        );
    }
}

#[test]
fn every_golden_schema_is_valid() {
    if updating() {
        return;
    }
    for case in common::CASES {
        let schema = common::case(case);
        let result = undra_bindgen::validate(&schema);
        assert!(
            result.is_ok(),
            "{case}: {}",
            result
                .err()
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
