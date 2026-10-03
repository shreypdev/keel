package dev.undra.runtime

/**
 * Loads the native library of a core by its namespace (ADR-044). The generated `UndraCoreNative` of each core
 * calls [load] once, when it is first touched.
 *
 * The library is `lib<namespace>.so` (Android: from the APK's `jniLibs/<abi>/`), `lib<namespace>.dylib` or
 * `<namespace>.dll`, found on `java.library.path`. The system property `undra.native.<namespace>.path`, when set,
 * is the path of the library file and wins over the search (for tests and development builds). A relative path is
 * taken from the working directory, which is what a Bazel test's `$(rootpath ..)` of the core's host build needs:
 *
 * ```
 * java -Dundra.native.playground_core.path=/path/to/build/host/libplayground_core.dylib ...
 * ```
 *
 * Loading runs the core's `JNI_OnLoad`, which registers its natives on its `UndraCoreNative` class through the
 * class loader that loaded this runtime, so the runtime and the generated bindings must share a class loader
 * (they do on Android and on a plain JVM classpath).
 */
public object NativeLibrary {
    /**
     * Loads the library of the core [namespace]; returns `null` on success, or the [UnsatisfiedLinkError] or
     * [SecurityException] that loading it threw (nothing is thrown), so a process without the library can still
     * load the core in `Mode.REMOTE`. Loading a library that is already loaded does nothing.
     */
    public fun load(namespace: String): Throwable? =
        try {
            val path = System.getProperty(pathProperty(namespace))
            if (path != null && path.isNotEmpty()) {
                // `System.load` takes absolute paths only.
                System.load(java.io.File(path).absolutePath)
            } else {
                System.loadLibrary(namespace)
            }
            null
        } catch (e: UnsatisfiedLinkError) {
            e
        } catch (e: SecurityException) {
            e
        }

    /** The system property holding the library file of the core [namespace]: `undra.native.<namespace>.path`. */
    internal fun pathProperty(namespace: String): String = "undra.native.$namespace.path"
}
