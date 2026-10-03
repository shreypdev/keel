"""Kotlin libraries over generated bindings: `undra_kt_jvm_library` and `undra_android_library` (ADR-061)."""

load("@rules_kotlin//kotlin:jvm.bzl", "kt_jvm_library")

_RUNTIME = Label("@undra//kotlin:runtime")

def undra_kt_jvm_library(name, bindings, deps = [], runtime = _RUNTIME, **kwargs):
    """Compiles the Kotlin bindings of an `undra_bindings` target as a JVM library.

    The library depends on the Kotlin runtime (kotlinx-coroutines comes with it) and on `deps`. A JVM that uses it loads the
    core's host library (`undra_core`'s `host` output) by its path, or from `java.library.path`. In a `java_test`:

        data = [":core_host"],
        jvm_flags = ["-Dundra.native.<namespace>.path=$(rootpath :core_host)"],

    Args:
        name: the library.
        bindings: an `undra_bindings` target that lists `kotlin` among its languages.
        deps: more dependencies.
        runtime: the Kotlin runtime library (default: the one of the Undra checkout the build is configured with).
        **kwargs: passed to `kt_jvm_library` (`visibility`, `tags`, ..).
    """
    srcjar = name + "_srcjar"
    native.filegroup(
        name = srcjar,
        srcs = [bindings],
        output_group = "kotlin_srcjar",
        tags = kwargs.get("tags", []),
    )
    kt_jvm_library(
        name = name,
        srcs = [":" + srcjar],
        deps = [runtime] + deps,
        **kwargs
    )
