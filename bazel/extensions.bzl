"""The `undra` module extension: where the Undra sources and the crates they need come from."""

load("//undra/private:binaryen.bzl", "undra_binaryen")
load("//undra/private:cargo_vendor.bzl", "undra_vendor")
load("//undra/private:source.bzl", "undra_source")

_source = tag_class(
    doc = "Where Undra is: a checkout (`path`) or a source archive (`urls`, `integrity`, `strip_prefix`).",
    attrs = {
        "path": attr.string(doc = "A checkout of the Undra repository, relative to the root module or absolute."),
        "urls": attr.string_list(),
        "integrity": attr.string(),
        "strip_prefix": attr.string(),
    },
)

_vendor = tag_class(
    doc = "The Cargo.lock files whose crates.io packages are downloaded (by checksum) for the offline builds.",
    attrs = {
        "lockfiles": attr.label_list(allow_files = True, doc = "The application's Cargo.lock, when it has one."),
    },
)

def _impl(mctx):
    source = None
    lockfiles = []
    for module in mctx.modules:
        for tag in module.tags.source:
            if source == None or module.is_root:
                source = tag
        for tag in module.tags.vendor:
            lockfiles.extend(tag.lockfiles)

    runtimes = {}
    if source != None:
        runtimes = _runtime_builds()
    undra_source(
        name = "undra",
        path = source.path if source else "",
        urls = source.urls if source else [],
        integrity = source.integrity if source else "",
        strip_prefix = source.strip_prefix if source else "",
        runtimes = runtimes,
    )
    undra_vendor(
        name = "undra_vendor",
        lockfiles = lockfiles + ([Label("@undra//:Cargo.lock")] if source != None else []),
    )
    undra_binaryen(name = "undra_binaryen")
    return mctx.extension_metadata(reproducible = True)

def _runtime_builds():
    # The BUILD file of each language runtime's package in @undra (templates, so a package that is not used is not loaded).
    return {
        "kotlin": Label("//undra/private/runtimes:kotlin.BUILD"),
    }

undra = module_extension(
    implementation = _impl,
    tag_classes = {
        "source": _source,
        "vendor": _vendor,
    },
    doc = "Configures where the Undra crates and runtimes come from.",
)
