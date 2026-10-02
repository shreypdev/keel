//! Build B of the migration scenarios (ADR-037; `src/updates.rs`): `cfg(playground_v2)` when the
//! `migration-v2` feature is on or `UNDRA_PLAYGROUND_V2=1` is in the environment (which is how a
//! contract runner asks `undra build` for it: the CLI passes no Cargo features to the core).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(playground_v2)");
    println!("cargo::rerun-if-env-changed=UNDRA_PLAYGROUND_V2");
    let feature = std::env::var_os("CARGO_FEATURE_MIGRATION_V2").is_some();
    let env = std::env::var("UNDRA_PLAYGROUND_V2").is_ok_and(|v| v == "1");
    if feature || env {
        println!("cargo::rustc-cfg=playground_v2");
    }
}
