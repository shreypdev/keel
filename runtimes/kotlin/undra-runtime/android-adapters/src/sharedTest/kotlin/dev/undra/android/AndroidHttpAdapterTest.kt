package dev.undra.android

import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * `AndroidHttpAdapter` against the contract every Http adapter of the Kotlin runtime meets (`HttpAdapterContract`, in
 * `../adapter-contracts`, where OkHttp's adapter runs it too: ADR-060): a real HTTP server on the loopback interface, on the
 * desktop JVM (unit tests) and on the device (instrumented tests, where `HttpURLConnection` is Android's own).
 */
class AndroidHttpAdapterTest : HttpAdapterContract() {
    override fun create(maxResponseBytes: Int?, idleTimeoutMs: Int?): HttpUnderTest {
        val adapter = AndroidHttpAdapter(
            idleTimeoutMs = idleTimeoutMs ?: AndroidHttpAdapter.DEFAULT_IDLE_TIMEOUT_MS,
            maxResponseBytes = maxResponseBytes ?: AndroidHttpAdapter.DEFAULT_MAX_RESPONSE_BYTES,
        )
        return object : HttpUnderTest {
            override suspend fun request(request: HttpRequest): HttpResponse = adapter.request(request)

            override fun portImpl(): PortImpl = adapter.portImpl()
        }
    }

    override val maxRedirects: Int get() = Redirects.MAX_HOPS

    @Test
    fun the_default_limits_are_the_documented_ones() {
        assertEquals(30_000, AndroidHttpAdapter.DEFAULT_CONNECT_TIMEOUT_MS)
        assertEquals(60_000, AndroidHttpAdapter.DEFAULT_IDLE_TIMEOUT_MS)
        assertEquals(64 * 1024 * 1024, AndroidHttpAdapter.DEFAULT_MAX_RESPONSE_BYTES)
    }
}
