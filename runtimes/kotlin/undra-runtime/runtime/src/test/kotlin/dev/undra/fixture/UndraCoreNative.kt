package dev.undra.fixture

import dev.undra.runtime.NativeApi
import dev.undra.runtime.NativeCallbacks
import dev.undra.runtime.NativeLibrary

/**
 * The JNI natives of `undra-ffi`'s fixture core (`crates/undra-ffi/tests/fixture`, namespace `undra_fixture`),
 * declared exactly as bindgen generates them for every core (ADR-044): the fixture's `JNI_OnLoad` registers its
 * natives on this class, `dev/undra/fixture/UndraCoreNative`.
 *
 * `NativeShapeTests` pins its shape. `NativeSmokeTests` runs against the library when it can be loaded: build it
 * with `cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml` and run the tests with
 * `UNDRA_NATIVE_LIB_DIR=crates/undra-ffi/tests/fixture/target/debug` (or
 * `-Dundra.native.undra_fixture.path=<libundra_fixture.dylib>`).
 */
object UndraCoreNative : NativeApi {
    /** The fixture core's namespace. */
    const val NAMESPACE: String = "undra_fixture"

    override val namespace: String = NAMESPACE

    private val loadFailure: Throwable? = NativeLibrary.load(namespace)

    override val isAvailable: Boolean get() = loadFailure == null
    override val unavailableReason: Throwable? get() = loadFailure

    override external fun abiVersion(): Int
    override external fun schemaHash(): Long
    override external fun schemaJson(): ByteArray
    override external fun init(cfg: ByteArray, cb: NativeCallbacks): Int
    override external fun call(payload: ByteArray): Int
    override external fun callSync(payload: ByteArray): ByteArray
    override external fun cancel(callId: Int)
    override external fun streamCredit(callId: Int, credit: Int)
    override external fun observe(handle: Long, signalId: Int, on: Boolean)
    override external fun release(handle: Long)
    override external fun portReply(payload: ByteArray)
    override external fun event(portId: Int, methodId: Int, payload: ByteArray)
    override external fun timerFired(timerId: Int)
    override external fun snapshot(): ByteArray
    override external fun restore(snapshot: ByteArray): Int
    override external fun statsJson(): String
    override external fun shutdown()
}
