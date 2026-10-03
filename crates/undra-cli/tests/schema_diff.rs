//! `undra schema diff` and `undra schema export` as the binary runs them (ADR-062): two schema
//! files, a schema file against a git ref in a scratch repository, the exit status of
//! `--exit-code`, and the file `export` writes from a built core.
//!
//! The two fixture schemas (`tests/fixtures/schema-diff/{old,new}.schema.json`, a to-do core before
//! and after a change that has something of every kind in it) are written from the builders below
//! with `UPDATE_GOLDEN=1`, like the bindgen goldens; the expected report is
//! `tests/golden/schema-diff/old-new.txt`. Without `UPDATE_GOLDEN` the files on disk are what is
//! read, and a test fails when they no longer match the builders.

mod common;

use std::path::{Path, PathBuf};

use common::{TempDir, git, has_tool, init_project, run_err, run_ok, serial, undra, undra_in};
use undra_meta::{
    EnumDef, FieldDef, FunctionDef, InfiniteDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind,
    QueryDef, QueryKind, RecordDef, Schema, SignalDef, StoreDef, TypeRef, VariantDef, ids,
};

// ----- the fixtures -------------------------------------------------------------------------------

fn field(name: &str, ty: TypeRef, default: bool) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        default,
        docs: String::new(),
    }
}

fn variant(name: &str, index: u16, fields: Vec<FieldDef>, tuple: bool) -> VariantDef {
    VariantDef {
        name: name.into(),
        index,
        fields,
        tuple,
        message: None,
        docs: String::new(),
    }
}

fn param(name: &str, ty: TypeRef) -> ParamDef {
    ParamDef {
        name: name.into(),
        ty,
    }
}

fn method(owner: &str, name: &str, params: Vec<ParamDef>, returns: TypeRef) -> MethodDef {
    MethodDef {
        name: name.into(),
        method_id: ids::method_id(owner, name),
        params,
        returns,
        is_async: false,
        takes_ctx: false,
        coalesce: false,
        generic: None,
        docs: String::new(),
    }
}

fn signal(name: &str, id: u32, ty: TypeRef) -> SignalDef {
    SignalDef {
        name: name.into(),
        signal_id: id,
        ty,
        computed: false,
        key: None,
        no_coalesce: false,
        default: false,
    }
}

fn todo_error() -> TypeRef {
    TypeRef::named("TodoError")
}

/// A to-do core, version 1.
fn old_schema() -> Schema {
    let mut s = Schema::new("todo-core");
    s.records.push(RecordDef {
        name: "Todo".into(),
        type_id: ids::type_id("Todo"),
        fields: vec![
            field("id", TypeRef::U32, false),
            field("title", TypeRef::String, false),
            field("done", TypeRef::Bool, true),
        ],
        transparent: false,
        docs: "An item.".into(),
    });
    s.enums.push(EnumDef {
        name: "Filter".into(),
        type_id: ids::type_id("Filter"),
        is_error: false,
        variants: vec![
            variant("All", 0, vec![], false),
            variant("Open", 1, vec![], false),
        ],
        docs: String::new(),
    });
    s.enums.push(EnumDef {
        name: "TodoError".into(),
        type_id: ids::type_id("TodoError"),
        is_error: true,
        variants: vec![VariantDef {
            message: Some("not found".into()),
            ..variant("NotFound", 0, vec![], false)
        }],
        docs: String::new(),
    });
    let mut todos_ctor = method("Todos", "new", vec![], TypeRef::named("Todos"));
    todos_ctor.takes_ctx = true;
    s.objects.push(ObjectDef {
        name: "Todos".into(),
        type_id: ids::type_id("Todos"),
        constructors: vec![todos_ctor],
        methods: vec![
            method(
                "Todos",
                "add",
                vec![param("title", TypeRef::String)],
                TypeRef::result(TypeRef::named("Todo"), todo_error()),
            ),
            method(
                "Todos",
                "toggle",
                vec![param("id", TypeRef::U32)],
                TypeRef::Unit,
            ),
        ],
        store: Some(StoreDef {
            signals: vec![
                SignalDef {
                    key: Some("id".into()),
                    ..signal("items", 0, TypeRef::vec(TypeRef::named("Todo")))
                },
                signal("filter", 1, TypeRef::named("Filter")),
            ],
        }),
        docs: String::new(),
    });
    s.functions.push(FunctionDef {
        name: "export_all".into(),
        method_id: ids::function_id("export_all"),
        params: vec![],
        returns: TypeRef::vec(TypeRef::named("Todo")),
        is_async: false,
        takes_ctx: false,
        generic: None,
        docs: String::new(),
    });
    s.ports.push(PortDef {
        name: "Notifier".into(),
        port_id: ids::port_id("Notifier"),
        kind: PortKind::Async,
        background: false,
        methods: vec![method(
            "Notifier",
            "notify",
            vec![param("text", TypeRef::String)],
            TypeRef::Unit,
        )],
        docs: String::new(),
    });
    s.ports.push(PortDef {
        name: "Progress".into(),
        port_id: ids::port_id("Progress"),
        kind: PortKind::Callback,
        background: false,
        methods: vec![method(
            "Progress",
            "update",
            vec![param("percent", TypeRef::U8)],
            TypeRef::Unit,
        )],
        docs: String::new(),
    });
    s.queries.push(query(
        "todo",
        QueryKind::Query,
        vec![param("id", TypeRef::U32)],
        TypeRef::result(TypeRef::named("Todo"), todo_error()),
    ));
    s.queries.push(query(
        "add_todo",
        QueryKind::Mutation,
        vec![param("title", TypeRef::String)],
        TypeRef::result(TypeRef::named("Todo"), todo_error()),
    ));
    s
}

fn query(name: &str, kind: QueryKind, params: Vec<ParamDef>, returns: TypeRef) -> QueryDef {
    QueryDef {
        name: name.into(),
        query_id: ids::fnv1a32(&format!("query.{name}")),
        kind,
        key: format!("{name}/{{id}}"),
        params,
        returns,
        stale_ms: None,
        persist: false,
        idempotent: false,
        interval_ms: None,
        poll_in_background: false,
        infinite: None::<InfiniteDef>,
    }
}

/// Version 2: one change of every kind the rules tell apart, breaking and additive.
fn new_schema() -> Schema {
    let mut s = old_schema();
    // Todo: a defaulted field (additive) and a required one (breaking).
    s.records[0]
        .fields
        .push(field("due", TypeRef::option(TypeRef::Timestamp), true));
    s.records[0]
        .fields
        .push(field("priority", TypeRef::U8, false));
    // Filter: a case added (breaking: exhaustive matches).
    s.enums[0]
        .variants
        .push(variant("Archived", 2, vec![], false));
    // Todos: a computed signal (additive), `add` takes tags (breaking), `clear` (additive), `toggle` gone (breaking).
    let todos = &mut s.objects[0];
    let store = todos.store.as_mut().unwrap();
    store.signals.push(SignalDef {
        computed: true,
        ..signal("open_count", 2, TypeRef::U32)
    });
    todos.methods[0]
        .params
        .push(param("tags", TypeRef::vec(TypeRef::String)));
    todos
        .methods
        .push(method("Todos", "clear", vec![], TypeRef::Unit));
    todos.methods.remove(1);
    // A new function (additive).
    s.functions.push(FunctionDef {
        name: "import_all".into(),
        method_id: ids::function_id("import_all"),
        params: vec![param("items", TypeRef::vec(TypeRef::named("Todo")))],
        returns: TypeRef::U32,
        is_async: true,
        takes_ctx: false,
        generic: None,
        docs: String::new(),
    });
    // The port the app implements gains a method (breaking); a new callback (additive).
    s.ports[0]
        .methods
        .push(method("Notifier", "dismiss", vec![], TypeRef::Unit));
    s.ports.push(PortDef {
        name: "Cancel".into(),
        port_id: ids::port_id("Cancel"),
        kind: PortKind::Callback,
        background: false,
        methods: vec![method("Cancel", "cancelled", vec![], TypeRef::Unit)],
        docs: String::new(),
    });
    // The query is cached longer (additive); the mutation is gone (breaking).
    s.queries[0].stale_ms = Some(30_000);
    s.queries.remove(1);
    s
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/schema-diff")
}

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/schema-diff/old-new.txt")
}

fn updating() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// The fixtures, written from the builders with `UPDATE_GOLDEN=1`.
fn fixtures() -> (PathBuf, PathBuf) {
    let (old, new) = (
        fixtures_dir().join("old.schema.json"),
        fixtures_dir().join("new.schema.json"),
    );
    if updating() {
        std::fs::create_dir_all(fixtures_dir()).unwrap();
        std::fs::write(&old, old_schema().to_json_pretty() + "\n").unwrap();
        std::fs::write(&new, new_schema().to_json_pretty() + "\n").unwrap();
    }
    (old, new)
}

fn read_schema(path: &Path) -> Schema {
    Schema::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_fixtures_are_what_the_builders_make() {
    let (old, new) = fixtures();
    assert_eq!(
        read_schema(&old),
        old_schema(),
        "UPDATE_GOLDEN=1 refreshes the fixtures"
    );
    assert_eq!(
        read_schema(&new),
        new_schema(),
        "UPDATE_GOLDEN=1 refreshes the fixtures"
    );
}

// ----- two files ----------------------------------------------------------------------------------

#[test]
fn two_schema_files_print_the_report_locked_in_the_golden() {
    let (old, new) = fixtures();
    let out = run_ok(
        undra()
            .args(["-C"])
            .arg(fixtures_dir())
            .args(["schema", "diff"])
            .arg(old.file_name().unwrap())
            .arg(new.file_name().unwrap()),
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if updating() {
        std::fs::create_dir_all(golden().parent().unwrap()).unwrap();
        std::fs::write(golden(), &text).unwrap();
    }
    let expected = std::fs::read_to_string(golden())
        .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", golden().display()));
    assert_eq!(
        text, expected,
        "the report changed; UPDATE_GOLDEN=1 cargo test -p undra-cli --test schema_diff after reading the diff"
    );
    // The report is the rules of SPEC 2.6 applied to every kind of change in the fixtures.
    for expected in [
        "additive  field Todo.due: added (Option<Timestamp>, with a default)",
        "breaking  field Todo.priority: added without a default",
        "breaking  case Filter.Archived: added",
        "additive  signal Todos.open_count: added (u32, computed)",
        "breaking  method Todos.add: parameter `tags: Vec<String>` added",
        "additive  method Todos.clear: added",
        "breaking  method Todos.toggle: removed",
        "additive  function import_all: added",
        "breaking  method Notifier.dismiss: added",
        "additive  callback Cancel: added",
        "additive  query todo: stale time changed from none to 30000 ms",
        "breaking  mutation add_todo: removed",
        "6 breaking, 6 additive.",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
}

#[test]
fn the_exit_status_follows_exit_code_and_only_then() {
    let (old, new) = fixtures();
    let diff = |a: &Path, b: &Path, extra: &[&str]| {
        undra()
            .args(["schema", "diff"])
            .arg(a)
            .arg(b)
            .args(extra)
            .output()
            .unwrap()
    };
    // Breaking: 0 without the flag (a reviewer runs it freely), 1 with it.
    assert!(diff(&old, &new, &[]).status.success());
    let gated = diff(&old, &new, &["--exit-code"]);
    assert_eq!(gated.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&gated.stdout).contains("6 breaking, 6 additive."),
        "the report is printed before the exit"
    );
    // Only additive (the same file twice has nothing; a schema gaining a function is additive): 0.
    assert!(diff(&old, &old, &["--exit-code"]).status.success());
    let scratch = TempDir::new("additive");
    let mut grown = old_schema();
    grown.functions.push(FunctionDef {
        name: "archive_all".into(),
        method_id: ids::function_id("archive_all"),
        params: vec![],
        returns: TypeRef::Unit,
        is_async: false,
        takes_ctx: false,
        generic: None,
        docs: String::new(),
    });
    let grown_path = scratch.path().join("grown.json");
    std::fs::write(&grown_path, grown.to_json_pretty()).unwrap();
    let additive = diff(&old, &grown_path, &["--exit-code"]);
    assert!(additive.status.success(), "{additive:?}");
    assert!(String::from_utf8_lossy(&additive.stdout).contains("0 breaking, 1 additive."));
}

#[test]
fn bad_input_teaches() {
    let (old, _) = fixtures();
    // One file is not a comparison.
    let (code, stderr) = run_err(undra().args(["schema", "diff"]).arg(&old));
    assert_eq!(code, 1);
    assert!(
        stderr.contains("error[undra::C0009]")
            && stderr.contains("compares two schema files, and 1 was given"),
        "{stderr}"
    );
    assert!(
        stderr.contains("= help:") && stderr.contains("--against"),
        "{stderr}"
    );
    // A file that is not a schema names itself.
    let scratch = TempDir::new("bad-schema");
    let bad = scratch.path().join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    let (_, stderr) = run_err(undra().args(["schema", "diff"]).arg(&old).arg(&bad));
    assert!(
        stderr.contains("error[undra::C0002]")
            && stderr.contains("bad.json: the schema is not valid JSON"),
        "{stderr}"
    );
    // A file that is not there is the operating system's complaint, as everywhere.
    let (_, stderr) = run_err(
        undra()
            .args(["schema", "diff"])
            .arg(&old)
            .arg(scratch.path().join("missing.json")),
    );
    assert!(stderr.contains("error[undra::C0010]"), "{stderr}");
}

// ----- against a git ref --------------------------------------------------------------------------

#[test]
fn a_schema_file_against_head_in_a_scratch_repository() {
    if common::skip_unless(has_tool("git", "--version"), "git is not installed") {
        return;
    }
    let (old, new) = fixtures();
    let repo = TempDir::new("against");
    git(repo.path(), &["init", "-q"]);
    // The old schema is committed; the working tree has the new one.
    std::fs::write(
        repo.path().join("schema.json"),
        std::fs::read(&old).unwrap(),
    )
    .unwrap();
    git(repo.path(), &["add", "schema.json"]);
    git(repo.path(), &["commit", "-q", "-m", "the API at v1"]);
    std::fs::write(
        repo.path().join("schema.json"),
        std::fs::read(&new).unwrap(),
    )
    .unwrap();

    let out = run_ok(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against",
        "HEAD",
    ]));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // The same report as the two files, with the sides named as a reviewer knows them.
    let two_files = std::fs::read_to_string(golden()).unwrap();
    let body_of = |report: &str| report.split_once("\n\n").unwrap().1.to_owned();
    assert_eq!(body_of(&text), body_of(&two_files), "{text}");
    assert!(
        text.starts_with("Public API: HEAD:schema.json (schema 0x")
            && text.contains(") -> schema.json (schema 0x"),
        "{text}"
    );

    // `--exit-code` fails on the breaking lines; naming the file explicitly says the same.
    let gated = undra_in(repo.path())
        .arg("-C")
        .arg(repo.path())
        .args([
            "schema",
            "diff",
            "--against",
            "HEAD",
            "schema.json",
            "--exit-code",
        ])
        .output()
        .unwrap();
    assert_eq!(gated.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&gated.stdout).contains("6 breaking, 6 additive."));

    // Once the new schema is committed there is nothing left to say against HEAD.
    git(repo.path(), &["commit", "-q", "-am", "the API at v2"]);
    let same = run_ok(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against",
        "HEAD",
        "--exit-code",
    ]));
    let text = String::from_utf8_lossy(&same.stdout);
    assert!(
        text.contains("No changes: the public API is the same"),
        "{text}"
    );
    // ... and against the commit before it, the report is the first one again.
    let before = run_ok(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against",
        "HEAD~1",
    ]));
    assert!(
        String::from_utf8_lossy(&before.stdout).contains("6 breaking, 6 additive."),
        "{}",
        String::from_utf8_lossy(&before.stdout)
    );
}

#[test]
fn the_default_file_is_schema_json_in_the_project_even_below_the_repository_root() {
    if common::skip_unless(has_tool("git", "--version"), "git is not installed") {
        return;
    }
    let (old, new) = fixtures();
    // `<repo>/<project>/`: the project is a subdirectory, so git is asked for `<project>/schema.json`.
    let project = init_project("against-project", "web");
    let repo = project.dir.path();
    git(repo, &["init", "-q"]);
    std::fs::write(
        project.root.join("schema.json"),
        std::fs::read(&old).unwrap(),
    )
    .unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "v1"]);
    std::fs::write(
        project.root.join("schema.json"),
        std::fs::read(&new).unwrap(),
    )
    .unwrap();

    let out = run_ok(undra_in(repo).arg("-C").arg(&project.root).args([
        "schema",
        "diff",
        "--against",
        "HEAD",
    ]));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("6 breaking, 6 additive."), "{text}");
    assert!(text.starts_with("Public API: HEAD:schema.json "), "{text}");
}

#[test]
fn a_ref_without_the_file_and_a_file_outside_a_repository_say_what_to_do() {
    if common::skip_unless(has_tool("git", "--version"), "git is not installed") {
        return;
    }
    let (old, _) = fixtures();
    let repo = TempDir::new("no-file");
    git(repo.path(), &["init", "-q"]);
    std::fs::write(repo.path().join("README"), "x").unwrap();
    git(repo.path(), &["add", "README"]);
    git(repo.path(), &["commit", "-q", "-m", "no schema yet"]);
    std::fs::write(
        repo.path().join("schema.json"),
        std::fs::read(&old).unwrap(),
    )
    .unwrap();

    let (code, stderr) = run_err(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against",
        "HEAD",
    ]));
    assert_eq!(code, 1);
    assert!(
        stderr.contains("error[undra::C0009]: `HEAD` has no `schema.json`")
            && stderr.contains("undra schema export -o schema.json"),
        "{stderr}"
    );
    // git's own words follow the diagnostic.
    assert!(
        stderr.contains("exists on disk, but not in 'HEAD'"),
        "{stderr}"
    );

    // A ref that git does not know.
    let (_, stderr) = run_err(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against",
        "no-such-ref",
    ]));
    assert!(
        stderr.contains("`no-such-ref` has no `schema.json`"),
        "{stderr}"
    );

    // Not a repository at all.
    let loose = TempDir::new("no-repo");
    std::fs::write(
        loose.path().join("schema.json"),
        std::fs::read(&old).unwrap(),
    )
    .unwrap();
    let (_, stderr) = run_err(undra_in(loose.path()).arg("-C").arg(loose.path()).args([
        "schema",
        "diff",
        "--against",
        "HEAD",
    ]));
    assert!(
        stderr.contains("is not inside a git repository"),
        "{stderr}"
    );

    // An option-looking ref never reaches git; too many files is a comparison of nothing.
    let (_, stderr) = run_err(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against=--output=/tmp/x",
    ]));
    assert!(stderr.contains("not a git ref"), "{stderr}");
    let (_, stderr) = run_err(undra_in(repo.path()).arg("-C").arg(repo.path()).args([
        "schema",
        "diff",
        "--against",
        "HEAD",
        "a.json",
        "b.json",
    ]));
    assert!(
        stderr.contains("one schema file with its version at a git ref"),
        "{stderr}"
    );
}

// ----- export ---------------------------------------------------------------------------------------

#[test]
fn export_writes_the_schema_of_the_built_core_and_diff_reads_it_back() {
    let _serial = serial();
    let project = init_project("schemaexport", "web");
    // Without -o the schema goes to standard output.
    let printed = run_ok(project.undra().args(["schema", "export"]));
    let stdout = String::from_utf8_lossy(&printed.stdout).into_owned();
    let schema = Schema::from_json(&stdout).expect("export prints a schema");
    assert!(
        !schema.records.is_empty() || !schema.objects.is_empty(),
        "{stdout}"
    );
    assert!(
        !stdout.contains("\"docs\""),
        "doc comments are not part of the file unless asked for"
    );

    // With -o it writes the file (relative to -C) and says so.
    let written = run_ok(
        project
            .undra()
            .args(["schema", "export", "-o", "schema.json"]),
    );
    let file = project.root.join("schema.json");
    assert!(
        String::from_utf8_lossy(&written.stdout).starts_with("Wrote ")
            && String::from_utf8_lossy(&written.stdout)
                .contains(&format!("{:#018x}", schema.hash())),
        "{}",
        String::from_utf8_lossy(&written.stdout)
    );
    assert_eq!(read_schema(&file), schema);
    // The same schema again is "unchanged", and the file diffs against itself as the same API.
    let again = run_ok(
        project
            .undra()
            .args(["schema", "export", "-o", "schema.json"]),
    );
    assert!(String::from_utf8_lossy(&again.stdout).starts_with("Unchanged:"));
    let diff = run_ok(project.undra().args([
        "schema",
        "diff",
        "schema.json",
        "schema.json",
        "--exit-code",
    ]));
    assert!(String::from_utf8_lossy(&diff.stdout).contains("No changes"));

    // The bindings generated from it are the bindings of the core: the hash is the same.
    let bindgen = run_ok(project.undra().args(["bindgen", "--check"]));
    assert!(
        String::from_utf8_lossy(&bindgen.stdout).contains(&format!("{:#018x}", schema.hash())),
        "{}",
        String::from_utf8_lossy(&bindgen.stdout)
    );

    // --docs keeps the comments of the core's source.
    let docs = run_ok(project.undra().args(["schema", "export", "--docs"]));
    assert!(
        String::from_utf8_lossy(&docs.stdout).contains("\"docs\""),
        "the template core documents its items"
    );
    // A schema with docs and one without are the same API.
    let with_docs = Schema::from_json(&String::from_utf8_lossy(&docs.stdout)).unwrap();
    assert_eq!(with_docs.hash(), schema.hash());
}

#[test]
fn export_needs_a_project() {
    let dir = TempDir::new("export-no-project");
    let (code, stderr) = run_err(undra().args(["schema", "export"]).current_dir(dir.path()));
    assert_eq!(code, 1);
    assert!(stderr.contains("error[undra::C0001]"), "{stderr}");
}
