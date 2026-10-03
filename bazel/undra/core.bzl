"""`undra_core`: the core of an Undra app, built for each platform it ships to (ADR-061)."""

load("//undra/private:actions.bzl", "RUNNER_ATTRS", "vendor_params", "write_params")

# The Bazel platforms (undra/platforms) each target platform of `undra build` selects the Rust toolchain for. A platform
# with several is built for all of them in one action: iOS is a device slice and a simulator slice, Android one per ABI.
_PLATFORMS = {
    "host": [],
    "web": ["wasm32"],
    "ios": ["ios_arm64", "ios_sim_arm64"],
    "android": ["android_arm64", "android_x86_64"],
}

def _target_platforms_impl(settings, attr):
    names = _PLATFORMS[attr.platform]
    if not names:
        return {"host": {"//command_line_option:platforms": settings["//command_line_option:platforms"]}}
    return {
        name: {"//command_line_option:platforms": [str(Label("//undra/platforms:" + name))]}
        for name in names
    }

_target_platforms = transition(
    implementation = _target_platforms_impl,
    inputs = ["//command_line_option:platforms"],
    outputs = ["//command_line_option:platforms"],
)

def _relative(path, base, label):
    """`path` below `base`, both execution-root-relative directories (`.` is the root)."""
    if path == base:
        return "."
    if base == ".":
        return path
    if not path.startswith(base + "/"):
        fail("{}: undra.toml is in {}, which is not below the Cargo workspace root {} (`workspace`)".format(label, path, base))
    return path[len(base) + 1:]

def _undra_core_impl(ctx):
    platform = ctx.attr.platform
    namespace = ctx.attr.namespace
    release = ctx.attr.release or platform == "web"  # `undra build` always builds the web core optimised

    host = ctx.attr._host_rust[platform_common.ToolchainInfo]
    lines = [
        "mode=build",
        "platform=" + platform,
        "release=" + ("1" if release else "0"),
        "symbols=" + ("1" if ctx.attr.symbols else "0"),
        "cli=" + ctx.executable.cli.path,
        "rustc=" + host.rustc.path,
        "cargo=" + host.cargo.path,
        "sysroot=" + host.sysroot,
    ]
    tool_files = [host.all_files]
    sysroots = {host.sysroot: True}
    for target in ctx.split_attr._target_rust.values():
        info = target[platform_common.ToolchainInfo]
        if info.sysroot not in sysroots:
            sysroots[info.sysroot] = True
            lines.append("sysroot=" + info.sysroot)
        tool_files.append(info.all_files)

    config = ctx.file.config
    config_dir = config.dirname or "."
    app_root = (ctx.file.workspace.dirname or ".") if ctx.file.workspace else config_dir
    lines += [
        "app_root=" + app_root,
        "project=" + _relative(config_dir, app_root, ctx.label),
        "namespace=" + namespace,
    ]

    outputs = []
    out_symbols = None
    if platform == "host":
        macos = ctx.target_platform_has_constraint(ctx.attr._macos[platform_common.ConstraintValueInfo])
        out = ctx.actions.declare_file("{}/lib{}.{}".format(ctx.label.name, namespace, "dylib" if macos else "so"))
        lines.append("out_file=" + out.path)
    elif platform == "web":
        out = ctx.actions.declare_file("{}/{}.wasm".format(ctx.label.name, namespace))
        lines.append("out_file=" + out.path)
    else:
        out = ctx.actions.declare_directory(ctx.label.name)
        lines.append("out_dir=" + out.path)
    outputs.append(out)
    if release and ctx.attr.symbols:
        out_symbols = ctx.actions.declare_directory(ctx.label.name + ".symbols")
        lines.append("out_symbols=" + out_symbols.path)
        outputs.append(out_symbols)

    inputs = [ctx.file.config] + ctx.files.srcs
    if ctx.file.workspace:
        inputs.append(ctx.file.workspace)
    for directory in ctx.attr.extra_path:
        lines.append("path=" + directory)
    if platform in ("ios", "android"):
        lines.append("inherit_path=1")
    extra_tools = []
    if ctx.attr.wasm_opt:
        tool = [f for f in ctx.files.wasm_opt if f.basename == "wasm-opt"]
        if len(tool) != 1:
            fail("`wasm_opt` of {} must name the files of one binaryen (exactly one of them called wasm-opt)".format(ctx.label))
        lines.append("path=" + tool[0].dirname)
        extra_tools.extend(ctx.files.wasm_opt)

    vendor_lines, vendor_files = vendor_params(ctx)
    lines += vendor_lines
    undra_root = ctx.file._undra_manifest.dirname
    lines.append("undra_root=" + undra_root)
    params = write_params(ctx, ctx.label.name + ".params", lines)

    ctx.actions.run(
        executable = ctx.file._runner,
        arguments = [params.path],
        inputs = depset(
            [params, ctx.file._undra_manifest] + inputs + extra_tools,
            transitive = [depset(ctx.files._undra_sources), vendor_files] + tool_files,
        ),
        tools = [ctx.executable.cli],
        outputs = outputs,
        mnemonic = "UndraBuild",
        progress_message = "Building the {} core of {}".format(platform, ctx.label),
        env = {"LC_ALL": "C"},
        use_default_shell_env = platform in ("ios", "android"),
        execution_requirements = ctx.attr.execution_requirements,
    )
    groups = {"core": depset([out])}
    if out_symbols:
        groups["symbols"] = depset([out_symbols])
    return [
        DefaultInfo(files = depset([out])),
        OutputGroupInfo(**groups),
    ]

_undra_core = rule(
    implementation = _undra_core_impl,
    attrs = dict(RUNNER_ATTRS, **{
        "platform": attr.string(values = ["host", "web", "ios", "android"], mandatory = True),
        "namespace": attr.string(mandatory = True),
        "config": attr.label(allow_single_file = True, mandatory = True),
        "workspace": attr.label(allow_single_file = True),
        "srcs": attr.label_list(allow_files = True),
        "release": attr.bool(default = False),
        "symbols": attr.bool(default = True),
        "wasm_opt": attr.label(allow_files = True, cfg = "exec"),
        "execution_requirements": attr.string_dict(),
        "extra_path": attr.string_list(),
        "cli": attr.label(default = Label("@undra//:cli"), executable = True, cfg = "exec"),
        "_undra_sources": attr.label(default = Label("@undra//:sources")),
        "_undra_manifest": attr.label(default = Label("@undra//:Cargo.toml"), allow_single_file = True),
        "_macos": attr.label(default = Label("@platforms//os:macos")),
        "_target_rust": attr.label(
            default = Label("@rules_rust//rust/toolchain:current_rust_toolchain"),
            cfg = _target_platforms,
            providers = [platform_common.ToolchainInfo],
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    }),
    doc = "One platform's build of an Undra core: `undra build --platform <platform>` in a hermetic, offline action.",
)

def undra_core(
        name,
        namespace,
        config = "undra.toml",
        srcs = [],
        workspace = None,
        platforms = ["host", "web"],
        release = False,
        symbols = True,
        wasm_opt = None,
        extra_path = [],
        tags = [],
        visibility = None,
        **kwargs):
    """Builds the core of an Undra app for each of `platforms`, with the same cargo profiles `undra build` uses.

    One target per platform, named `<name>_<platform>` (`core_host`, `core_web`, `core_ios`, `core_android`), and `<name>`, a
    filegroup of all of them. What each makes, the same files `undra build` writes below `build/`:

    * `host`: `<name>_host/lib<namespace>.dylib` (`.so` on Linux), for the JVM and for `undra_bindings`;
    * `web`: `<name>_web/<namespace>.wasm`, the release-wasm profile then `wasm-opt -Oz` when `wasm_opt` is given;
    * `ios`: the directory `<name>_ios` holding `<Namespace>Core.xcframework` (macOS with Xcode only);
    * `android`: the directory `<name>_android` holding `jniLibs/<abi>/lib<namespace>.so` (needs the NDK and cargo-ndk).

    Each target also has an output group `symbols`: the symbol files `undra build --release` writes (a crash report's
    addresses resolve to file and line with them; `undra symbolicate` reads them).

    Args:
        name: the filegroup of every platform's build.
        namespace: the core's `[core] namespace` in undra.toml; the build fails, naming both, if undra.toml says another.
        config: the project's undra.toml.
        srcs: every file of the project the build reads: its Cargo manifests and lock file and the core's sources.
        workspace: the Cargo workspace's root `Cargo.toml` when it is above undra.toml's directory (default: the same directory).
        platforms: any of `host`, `web`, `ios`, `android`.
        release: build `host`, `ios` and `android` with the release profile (LTO, one codegen unit). `web` always is.
        symbols: also write the symbol files of a release build, as `undra build` does by default (the shipped web module differs
            by about 0.1% when it does not: `wasm-opt` sees the names, ADR-046), so the default is the CLI's own bytes.
        wasm_opt: an executable `wasm-opt` (binaryen); without it the web module is not shrunk further, as the CLI says.
        extra_path: directories (absolute) put on the action's `PATH` for the `ios` and `android` builds, whose toolchains are the
            machine's: `cargo-ndk`'s directory, for one. `ANDROID_NDK_HOME` and `DEVELOPER_DIR` come in through `--action_env`.
        tags: tags of the generated targets.
        visibility: the visibility of every generated target.
        **kwargs: passed to the generated rule instances (`execution_requirements`).
    """
    targets = []
    for platform in platforms:
        target = "{}_{}".format(name, platform)
        _undra_core(
            name = target,
            platform = platform,
            namespace = namespace,
            config = config,
            srcs = srcs,
            workspace = workspace,
            release = release,
            symbols = symbols,
            wasm_opt = wasm_opt if platform == "web" else None,
            extra_path = extra_path,
            # The Apple and Android toolchains are the machine's, not Bazel's (ADR-061): they build when asked for by name.
            tags = tags + (["manual", "requires-darwin"] if platform == "ios" else []) + (["manual"] if platform == "android" else []),
            target_compatible_with = ["@platforms//os:macos"] if platform == "ios" else [],
            visibility = visibility,
            **kwargs
        )
        targets.append(":" + target)
    native.filegroup(
        name = name,
        srcs = targets,
        tags = tags + (["manual"] if "ios" in platforms or "android" in platforms else []),
        visibility = visibility,
    )
