"""`undra_cli`: the `undra` command line, built from the Undra checkout by the pinned Rust toolchain."""

load("//undra/private:actions.bzl", "RUNNER_ATTRS", "RUST_TOOLCHAIN_TYPE", "toolchain_params", "vendor_params", "write_params")

def _undra_cli_impl(ctx):
    if not ctx.files.sources:
        fail(("//{pkg}:{name}: no Undra checkout is configured. Declare one in MODULE.bazel:\n" +
              "    undra = use_extension(\"@undra_rules//:extensions.bzl\", \"undra\")\n" +
              "    undra.source(path = \"../..\")   # a checkout of the Undra repository (or urls = [..] for an archive)\n" +
              "    use_repo(undra, \"undra\", \"undra_vendor\")").format(pkg = ctx.label.package, name = ctx.label.name))
    out = ctx.actions.declare_file(ctx.label.name)
    tool_lines, tool_files = toolchain_params(ctx)
    vendor_lines, vendor_files = vendor_params(ctx)
    params = write_params(ctx, ctx.label.name + ".params", [
        "mode=cli",
        "undra_root=" + ctx.file.workspace_manifest.dirname,
        "out_file=" + out.path,
    ] + tool_lines + vendor_lines)
    ctx.actions.run(
        executable = ctx.file._runner,
        arguments = [params.path],
        inputs = depset([params], transitive = [depset(ctx.files.sources), tool_files, vendor_files]),
        outputs = [out],
        mnemonic = "UndraCli",
        progress_message = "Building the undra command line",
        env = {"LC_ALL": "C"},
    )
    return [DefaultInfo(files = depset([out]), executable = out)]

undra_cli = rule(
    implementation = _undra_cli_impl,
    attrs = dict(RUNNER_ATTRS, **{
        "sources": attr.label_list(
            doc = "The files of the Undra checkout the CLI is built from (its crates and Cargo manifests).",
            allow_files = True,
        ),
        "workspace_manifest": attr.label(
            doc = "The checkout's workspace Cargo.toml.",
            allow_single_file = True,
        ),
    }),
    toolchains = [RUST_TOOLCHAIN_TYPE],
    executable = True,
    doc = "Builds the `undra` binary from a checkout of the Undra repository, offline, with the Rust toolchain of the build.",
)
