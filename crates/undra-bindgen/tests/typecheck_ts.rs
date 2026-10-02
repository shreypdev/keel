//! Type-checks the generated TypeScript against the real wire layer and the
//! real standard types of `@undra/runtime` (its declarations are built from
//! source on every run) and the hand-written declaration of the base API in
//! `tests/fixtures/ts-base/index.d.ts`. Skipped, with a message on stderr,
//! when no TypeScript compiler is installed; `UNDRA_REQUIRE_TOOLCHAINS=1` turns
//! the skip into a failure.

mod common;

use common::{install_ts_runtime, scratch, skip, ts_runtime_declarations, tsc};
use undra_bindgen::GeneratedFile;

/// Writes `files` as a package next to the installed runtime and type-checks it
/// with the package's own `tsconfig.json`.
fn typecheck(name: &str, files: &[GeneratedFile]) -> Result<(), String> {
    let declarations = ts_runtime_declarations().expect("declarations were built");
    let root = scratch(&format!("ts-{name}"));
    GeneratedFile::write_all(files, &root).unwrap();
    install_ts_runtime(&root, declarations, None);
    let mut cmd = tsc().expect("tsc was found earlier");
    let output = cmd
        .args(["--noEmit", "-p"])
        .arg(root.join("tsconfig.json"))
        .output()
        .unwrap();
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn typecheck_case(case: &str) {
    if ts_runtime_declarations().is_none() {
        skip("no TypeScript compiler (tsc) found");
        return;
    }
    let schema = common::case(case);
    let generator = common::generator_for(case, &schema);
    let files = generator.typescript(&schema).unwrap();
    if let Err(diagnostics) = typecheck(case, &files) {
        panic!("the generated TypeScript of `{case}` does not type-check:\n{diagnostics}");
    }
}

macro_rules! ts_cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                typecheck_case(stringify!($name));
            }
        )*
    };
}

ts_cases!(
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
    lazy
);

#[test]
fn js_number_output_type_checks() {
    if ts_runtime_declarations().is_none() {
        skip("no TypeScript compiler (tsc) found");
        return;
    }
    let schema = common::case("records");
    let mut generator = common::generator_for("records", &schema);
    generator.ts_js_number = true;
    let files = generator.typescript(&schema).unwrap();
    if let Err(diagnostics) = typecheck("records-js-number", &files) {
        panic!("the js_number TypeScript does not type-check:\n{diagnostics}");
    }
}

/// A stream whose arguments take an object and a callback (no golden case has one): the object is
/// written after `requireOwn`, the callback lent with the runtime's `lend`.
#[test]
fn a_stream_that_takes_objects_and_callbacks_type_checks() {
    if ts_runtime_declarations().is_none() {
        skip("no TypeScript compiler (tsc) found");
        return;
    }
    let mut schema = common::case("callbacks");
    let uploader = schema
        .objects
        .iter_mut()
        .find(|o| o.name == "Uploader")
        .expect("the callbacks case has an Uploader");
    uploader.methods.push(common::method(
        "Uploader",
        "follow",
        "Follows `watch`, telling `listener`.",
        vec![
            common::param("watch", common::obj("Watch")),
            common::param("listener", common::cb("UploadListener")),
        ],
        undra_meta::TypeRef::Stream(Box::new(undra_meta::TypeRef::U32)),
        false,
    ));
    let generator = common::generator_for("callbacks", &schema);
    let files = generator.typescript(&schema).unwrap();
    let objects = &files
        .iter()
        .find(|f| f.path == "src/objects.ts")
        .unwrap()
        .contents;
    assert!(objects.contains("w.writeU64(requireOwn(this.core, watch));"));
    assert!(objects.contains("w.writeU64(lend(this.core, listener, UploadListenerCallback));"));
    if let Err(diagnostics) = typecheck("callbacks-stream", &files) {
        panic!("a stream with object and callback arguments does not type-check:\n{diagnostics}");
    }
}

/// A newtype is a type of its own in TypeScript: the brand says which one. Each `@ts-expect-error` below must be
/// an error (tsc reports an unused directive otherwise), and everything else must compile.
#[test]
fn newtypes_are_nominal_in_typescript() {
    if ts_runtime_declarations().is_none() {
        skip("no TypeScript compiler (tsc) found");
        return;
    }
    let schema = common::case("newtypes");
    let generator = common::generator_for("newtypes", &schema);
    let mut files = generator.typescript(&schema).unwrap();
    files.push(GeneratedFile {
        path: "src/misuse.ts".to_owned(),
        contents: r#"import { type Account, Boss, Meters, Nickname, Owner, TodoId, UserId } from "./types.js";

export const user: UserId = UserId("a");
export const owner: Owner = Owner(user);
export const boss: Boss = Boss(owner);
// @ts-expect-error a `TodoId` is not a `UserId`
export const wrongId: UserId = TodoId("a");
// @ts-expect-error a plain string is not a `UserId`
export const plain: UserId = "a";
// @ts-expect-error a newtype of a newtype is a type of its own
export const notBoss: Boss = owner;
// @ts-expect-error `Boss` wraps an `Owner`, not the `UserId` inside it
export const skipped: Boss = Boss(user);
export const nobody: Nickname = null;
export const named: Nickname = Nickname("x");
// @ts-expect-error a plain string is not a `Nickname`
export const raw: Nickname = "x";
export const scores = new Map<UserId, Meters>([[user, Meters(1.5)]]);
// @ts-expect-error a `TodoId` is not a key of that map
scores.get(TodoId("a"));
// A branded value is still the value it wraps.
export const text: string = user;
export const metres: number = Meters(2) + 1;
export const idOf = (account: Account): UserId => account.id;
"#
        .to_owned(),
    });
    if let Err(diagnostics) = typecheck("newtypes-nominal", &files) {
        panic!("the newtypes are not nominal:\n{diagnostics}");
    }
}
