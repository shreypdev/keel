"""`undra_swift_library`: the generated Swift bindings as a `swift_library` over the Swift runtime (ADR-061)."""

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("@rules_swift//swift:swift_interop_hint.bzl", "swift_interop_hint")
load("@rules_swift//swift:swift_library.bzl", "swift_library")

_RUNTIME = Label("@undra//swift:runtime")

def undra_swift_library(name, bindings, module_name, ffi_module, deps = [], runtime = _RUNTIME, **kwargs):
    """Compiles the Swift bindings of an `undra_bindings` target.

    `rules_swift` takes source files, not a directory, so the `undra_bindings` target lists the files it extracts from the Swift
    tree (`swift_files`), and this macro compiles those: the Swift files as `module_name` (the `swift_module` of undra.toml's
    `[bindings]`), and the C module that declares the core's entry point (`<namespace>_undra_api`) as `ffi_module`. An app links
    the core itself (`undra_core`'s `ios` output, the XCFramework) into the binary that uses this library. Builds on macOS only.

    Args:
        name: the library.
        bindings: an `undra_bindings` target that lists `swift` and `swift_files`.
        module_name: the Swift module's name.
        ffi_module: the C module of the entry, `<Namespace>CoreFFI` (the directory `Sources/<ffi_module>` of the Swift tree).
        deps: more dependencies.
        runtime: the Swift runtime library (default: the one of the Undra checkout the build is configured with).
        **kwargs: passed to `swift_library` (`visibility`, `tags`, ..).
    """
    tags = kwargs.pop("tags", []) + ["requires-darwin"]
    compatible = kwargs.pop("target_compatible_with", ["@platforms//os:macos", "@platforms//os:ios"])
    for group in ["swift_sources", "swift_ffi_sources", "swift_ffi_headers", "swift_ffi_modulemap"]:
        native.filegroup(
            name = "{}_{}".format(name, group),
            srcs = [bindings],
            output_group = group,
            tags = tags,
            testonly = kwargs.get("testonly", False),
        )

    swift_interop_hint(
        name = name + "_ffi_hint",
        module_map = ":{}_swift_ffi_modulemap".format(name),
        module_name = ffi_module,
        tags = tags,
    )
    # The generated C source includes its header as "<ns>_undra.h", from the `include` directory of its tree (SwiftPM adds that
    # directory itself): the declared files keep the tree's layout below `<bindings>_swift_files/`.
    label = native.package_relative_label(bindings)
    include = "{}{}_swift_files/Sources/{}/include".format(label.package + "/" if label.package else "", label.name, ffi_module)
    cc_library(
        name = name + "_ffi",
        srcs = [":{}_swift_ffi_sources".format(name)],
        hdrs = [":{}_swift_ffi_headers".format(name), ":{}_swift_ffi_modulemap".format(name)],
        aspect_hints = [":" + name + "_ffi_hint"],
        copts = ["-I$(BINDIR)/" + include],
        tags = tags,
    )
    swift_library(
        name = name,
        srcs = [":{}_swift_sources".format(name)],
        copts = ["-swift-version", "6"],
        module_name = module_name,
        deps = [":" + name + "_ffi", runtime] + deps,
        tags = tags,
        **kwargs
    )
