"""What every Undra action shares: the Rust toolchain, the vendored crates, and the runner script."""

RUST_TOOLCHAIN_TYPE = "@rules_rust//rust:toolchain_type"

# Attributes the rules that run `run.sh` declare.
RUNNER_ATTRS = {
    "_runner": attr.label(
        default = Label("//undra/private:run.sh"),
        allow_single_file = True,
        cfg = "exec",
    ),
    "_vendor": attr.label(
        default = Label("@undra_vendor//:crates"),
        cfg = "exec",
    ),
    # The toolchain Bazel resolves for the *execution* platform: what runs Cargo, build scripts and proc macros. The one the
    # rule's own toolchain resolution gives is for its *target* platform (wasm32, iOS, ..), and holds that target's standard
    # library only.
    "_host_rust": attr.label(
        default = Label("@rules_rust//rust/toolchain:current_rust_toolchain"),
        cfg = "exec",
        providers = [platform_common.ToolchainInfo],
    ),
}

def toolchain_params(ctx):
    """The lines of the parameter file that name the Rust toolchain, and the files they refer to.

    Returns:
        a `(lines, files)` pair: the `rustc=`, `cargo=` and `sysroot=` lines, and a depset of everything they name.
    """
    target = ctx.toolchains[RUST_TOOLCHAIN_TYPE]
    host = ctx.attr._host_rust[platform_common.ToolchainInfo]
    lines = [
        "rustc=" + host.rustc.path,
        "cargo=" + host.cargo.path,
        "sysroot=" + host.sysroot,
    ]
    if target.sysroot != host.sysroot:
        lines.append("sysroot=" + target.sysroot)
    files = depset(transitive = [host.all_files, target.all_files])
    return lines, files

def vendor_params(ctx):
    """The `vendor=` line and the vendored archives (a depset of files)."""
    vendor = ctx.attr._vendor.files.to_list()
    manifest = [f for f in vendor if f.basename == "manifest.txt"]
    if len(manifest) != 1:
        fail("@undra_vendor has no manifest: declare `undra.vendor(lockfiles = [..])` in MODULE.bazel")
    return ["vendor=" + manifest[0].path], ctx.attr._vendor.files

def write_params(ctx, name, lines):
    """Writes the parameter file `run.sh` reads."""
    params = ctx.actions.declare_file(name)
    ctx.actions.write(params, "\n".join(lines) + "\n")
    return params
