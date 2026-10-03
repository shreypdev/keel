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

def below(path, root):
    """`path` relative to the directory `root`, both short paths (`""` is the root of the main repository).

    Returns:
        the relative path, or `None` when `path` is not below `root`.
    """
    if not root:
        return None if path.startswith("../") else path
    if path.startswith(root + "/"):
        return path[len(root) + 1:]
    return None

def stage_manifest(ctx, name, root, files, what):
    """Writes the list of files an action copies into its private stage, and nothing else.

    `run.sh` copies exactly these files (following the sandbox's symlinks), so an action reads only what it declares, whatever
    the spawn strategy: a sandbox, `--spawn_strategy=local` or remote execution.

    Args:
        ctx: the rule context.
        name: the manifest's file name.
        root: the short path of the directory the files are copied relative to.
        files: the files (a list of `File`).
        what: what to call `root` in an error message.

    Returns:
        a `(manifest, root_exec_path)` pair: the manifest, one `<path below root>|<exec path>` line per file, and the execution
        path of `root` for the files that are sources (`run.sh` copies those in one `tar`).
    """
    lines = {}
    root_exec = None
    for f in files:
        rel = below(f.short_path, root)
        if rel == None:
            fail("{label}: {file} is outside {root}, {what}: an action sees only the files below it".format(
                label = ctx.label,
                file = f.short_path,
                what = what,
                root = root or "the repository root",
            ))
        lines["{}|{}".format(rel, f.path)] = True
        if root_exec == None and f.is_source:
            root_exec = f.path[:len(f.path) - len(rel)].rstrip("/") or "."
    manifest = ctx.actions.declare_file(name)
    ctx.actions.write(manifest, "\n".join(sorted(lines.keys())) + "\n")
    return manifest, root_exec or "."

def short_dirname(f):
    """The directory of `f` as a short path (`""` for the root of the main repository)."""
    path = f.short_path
    return path[:path.rfind("/")] if "/" in path else ""
