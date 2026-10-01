package dev.undra.runtime

import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds

/** Where the core runs relative to the app. */
public enum class Mode {
    /** In this process, through the JNI shim ([UndraNative]). The production mode. */
    INPROC,

    /**
     * In another process (`undra dev`), over a WebSocket. **A development mode**: every call is a network
     * round trip, and `callSync` and `construct` block the calling thread until the reply arrives, so
     * do not use it to ship an app.
     */
    REMOTE,
}

/**
 * How [UndraCore.load] starts a core.
 *
 * @property mode [Mode.INPROC] (default) or [Mode.REMOTE].
 * @property remoteUrl the `ws://` or `wss://` URL of the `undra dev` server; required for [Mode.REMOTE]
 *   and rejected for [Mode.INPROC].
 * @property adapters platform implementations of ports, by port id (`UndraIds.Ports.<Trait>.PORT_ID`),
 *   made with the generated `<trait>PortImpl(...)` functions. They take precedence over the default
 *   adapters.
 * @property expectedSchemaHash the schema hash of the bindings (`UndraIds.SCHEMA_HASH`); loading fails
 *   with [UndraSchemaMismatchException] if the core reports another one.
 * @property defaultAdapters install the JVM default adapters (`dev.undra.runtime.adapters.JvmAdapters`) for
 *   every standard port not in [adapters]. On Android only the portable ones (Clock, Rng, Log, Timer) are
 *   installed; Http, Kv, SecureStore and Fs come from the `android-adapters` module.
 * @property remoteTimeout how long a blocking call (`callSync`, `construct`) and the connection
 *   handshake wait for the remote core before giving up.
 */
public class LoadOptions(
    public val mode: Mode = Mode.INPROC,
    public val remoteUrl: String? = null,
    public val adapters: Map<UInt, PortImpl> = emptyMap(),
    public val expectedSchemaHash: ULong,
    public val defaultAdapters: Boolean = true,
    public val remoteTimeout: Duration = 30.seconds,
) {
    override fun toString(): String =
        "LoadOptions(mode=$mode, remoteUrl=$remoteUrl, adapters=${adapters.keys.sorted()}, " +
            "expectedSchemaHash=0x${expectedSchemaHash.toString(16)}, defaultAdapters=$defaultAdapters, remoteTimeout=$remoteTimeout)"
}
