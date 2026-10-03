"""`undra_ts_library`: the generated TypeScript bindings, compiled and packaged for Node and bundlers (ADR-061)."""

load("@aspect_rules_js//js:defs.bzl", "js_run_binary")
load("@aspect_rules_js//npm:defs.bzl", "npm_link_package", "npm_package")

_RUNTIME = Label("@undra//ts:runtime")
_TSC = Label("@npm_typescript//:tsc")
_RUNTIME_PACKAGE = "@undra/runtime"

def undra_ts_library(name, bindings, package, version = "0.1.0", deps = [], runtime = _RUNTIME, **kwargs):
    """Compiles the TypeScript bindings of an `undra_bindings` target into an npm package linked into `node_modules`.

    `tsc` runs over the generated tree (its own `tsconfig.json`: strict, `noUncheckedIndexedAccess`, ..) against the compiled
    runtime, so a bindings tree that does not typecheck fails the build. Three targets come out of it:

    * `<name>`: the compiled package (`npm_package`: `dist/*.js`, `dist/*.d.ts`, `package.json`);
    * `node_modules/<package>`: it linked into `node_modules` of the calling package, which is where `js_test`, `js_binary` and
      `ts_project` targets in this package or below it resolve `import ".." from "<package>"` from;
    * `node_modules/@undra/runtime`: the compiled runtime, linked once per package, which the bindings import.

    Args:
        name: the compiled package.
        bindings: an `undra_bindings` target that lists `ts` among its languages.
        package: the package's name, `@<ts_scope>/<ts_package>` of undra.toml's `[bindings]`.
        version: the package's version.
        deps: more targets the package needs at run time (npm links).
        runtime: the `@undra/runtime` package (default: the one of the Undra checkout the build is configured with).
        **kwargs: `visibility` and `tags` of the targets.
    """
    runtime_link = "node_modules/" + _RUNTIME_PACKAGE
    if not native.existing_rule(runtime_link):
        npm_link_package(
            name = runtime_link,
            src = runtime,
            **kwargs
        )

    tree = name + "_tree"
    native.filegroup(
        name = tree,
        srcs = [bindings],
        output_group = "ts",
        **kwargs
    )

    # The package manifest is the generated one: its `exports` point at `dist/`, which is where `tsc` writes.
    manifest = name + "_manifest"
    native.genrule(
        name = manifest,
        srcs = [":" + tree],
        outs = [name + "_package/package.json"],
        cmd = "cp $(execpath :{tree})/package.json $@".format(tree = tree),
        **kwargs
    )

    package_dir = native.package_name()
    out_dir = "{}{}_package/dist".format(package_dir + "/" if package_dir else "", name)
    js_run_binary(
        name = name + "_tsc",
        tool = _TSC,
        srcs = [":" + tree, ":" + runtime_link] + deps,
        args = [
            "--project",
            # `tsc` runs in the root of the output tree, so paths are relative to it: `rootpath`, not `execpath`.
            "$(rootpath :{tree})/tsconfig.json".format(tree = tree),
            "--outDir",
            out_dir,
        ],
        out_dirs = [name + "_package/dist"],
        mnemonic = "UndraTsc",
        progress_message = "Compiling the TypeScript bindings %{label}",
        **kwargs
    )

    npm_package(
        name = name,
        srcs = [":" + name + "_tsc", ":" + manifest],
        package = package,
        version = version,
        replace_prefixes = {name + "_package/": ""},
        **kwargs
    )
    npm_link_package(
        name = "node_modules/" + package,
        src = ":" + name,
        **kwargs
    )
