package dev.undra.runtime

import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * The entry point of one core (ADR-044): what the generated `Undra<Namespace>` object of the core's bindings
 * delegates to. It loads the core with the bindings' schema hash and the core's own natives, and remembers it, so
 * the generated classes of those bindings use their own core even when the process holds several.
 *
 * ```kotlin
 * // Generated, in the bindings' Core.kt:
 * object UndraPlaygroundCore {
 *     const val NAMESPACE: String = "playground_core"
 *     private val entry = CoreEntry(NAMESPACE, UndraIds.SCHEMA_HASH) { UndraCoreNative }
 *     fun load(options: LoadOptions = LoadOptions()): UndraCore = entry.load(options)
 *     val core: UndraCore get() = entry.core
 * }
 * ```
 *
 * Apps use the generated object, not this class.
 *
 * @property namespace the core's namespace (`[core] namespace` in `undra.toml`).
 * @property schemaHash the schema hash the bindings were generated from (`UndraIds.SCHEMA_HASH`).
 * @param native the core's JNI natives (its generated `UndraCoreNative`). Called only by an in-process [load], so
 *   the core's library is not loaded by a process that only connects to `undra dev`.
 */
public class CoreEntry(
    public val namespace: String,
    public val schemaHash: ULong,
    private val native: () -> NativeApi,
) {
    private val loaded = AtomicReference<UndraCore?>(null)
    private val loading = AtomicBoolean(false)
    private val placeholder: UndraCore by lazy { UnloadedCore(namespace) }
    private val placeholderWarned = AtomicBoolean(false)

    /**
     * Loads the core as [options] say (in this process unless they say [Mode.REMOTE]) and makes it [core]. An
     * unset [LoadOptions.expectedSchemaHash] is filled in with [schemaHash], and an unset [LoadOptions.namespace] with
     * [namespace] (the default stores are kept under it). The first core loaded in the process
     * also becomes [UndraCore.shared].
     *
     * @throws UndraSchemaMismatchException if the core was built from another schema.
     * @throws UndraModeException if [options] contradict each other.
     * @throws UndraException if the core cannot be started or reached, or this core is already loaded (close it
     *   first) or being loaded on another thread.
     */
    public fun load(options: LoadOptions = LoadOptions()): UndraCore {
        if (!loading.compareAndSet(false, true)) {
            throw UndraException("the Undra core `$namespace` is being loaded on another thread; load it once, at app startup")
        }
        try {
            if (loaded.get()?.isOpen == true) {
                throw UndraException(
                    "the Undra core `$namespace` is already loaded; use it (Undra<Namespace>.core), or close it before loading it again",
                )
            }
            val core = UndraCore.start(options.withSchemaHashDefault(schemaHash).withNamespaceDefault(namespace), native)
            loaded.set(core)
            return core
        } finally {
            loading.set(false)
        }
    }

    /**
     * The core [load] returned while it is not closed; otherwise (before a load succeeds, or after the core was
     * closed) a closed placeholder whose calls fail with [UndraCallError.Unavailable] and whose first use logs what
     * to do. It never blocks and never loads anything.
     */
    public val core: UndraCore
        get() {
            val current = loaded.get()
            if (current != null && current.isOpen) return current
            if (placeholderWarned.compareAndSet(false, true)) {
                UndraLog.error(
                    "the Undra core `$namespace` was used while it is not loaded (before its load succeeds, or after it " +
                        "was closed); calls on it fail with UndraCallError.Unavailable. Load it at app startup with " +
                        "Undra<Namespace>.load(), before creating any of its objects.",
                )
            }
            return placeholder
        }

    override fun toString(): String = "CoreEntry($namespace, schema 0x${schemaHash.toString(16)})"

    private companion object {
        val UndraCore.isOpen: Boolean get() = connectionState.value !is ConnectionState.Closed
    }
}
