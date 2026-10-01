package dev.undra.android

import android.os.Handler
import android.os.Looper
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import java.io.ByteArrayOutputStream
import java.net.URL
import java.util.concurrent.CopyOnWriteArrayList
import java.util.zip.GZIPOutputStream
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** What only a device can show about the Http adapter: it never works on the main thread, and Android's own conveniences. */
class HttpOnDeviceTest {
    private lateinit var server: TestHttpServer

    @Before
    fun setUp() {
        server = TestHttpServer()
    }

    @After
    fun tearDown() {
        server.close()
    }

    /** A dispatcher that runs on the main thread, standing in for `Dispatchers.Main` (which needs another artifact). */
    private val main = object : CoroutineDispatcher() {
        private val handler = Handler(Looper.getMainLooper())

        override fun dispatch(context: CoroutineContext, block: Runnable) {
            handler.post(block)
        }
    }

    @Test
    fun a_request_made_from_the_main_thread_does_its_network_work_on_another_thread() {
        val threads = CopyOnWriteArrayList<Thread>()
        val adapter = AndroidHttpAdapter(30_000, 60_000, 1 shl 20) { url: URL ->
            threads.add(Thread.currentThread())
            url.openConnection()
        }
        server.fixed("/x", 200, "ok")
        val answer = runBlocking {
            withContext(main) {
                assertTrue("the caller is on the main thread", Looper.myLooper() === Looper.getMainLooper())
                adapter.request(HttpRequest(HttpMethod.GET, server.base + "/x", emptyList(), null, null))
            }
        }
        assertEquals("ok", String(answer.body))
        assertEquals(1, threads.size)
        assertFalse("opening the connection happened on the main thread", threads.single() === Looper.getMainLooper().thread)
        assertTrue(threads.single().name, threads.single().name.contains("DefaultDispatcher"))
    }

    @Test
    fun a_request_that_fails_from_the_main_thread_fails_with_a_typed_error_not_a_strictmode_crash() {
        val adapter = AndroidHttpAdapter()
        val failure = runBlocking {
            withContext(main) {
                try {
                    adapter.request(HttpRequest(HttpMethod.GET, "http://127.0.0.1:1/", emptyList(), null, 5_000u))
                    null
                } catch (e: dev.undra.runtime.adapters.HttpError) {
                    e
                }
            }
        }
        assertNotNull(failure)
        assertTrue(failure.toString(), failure is dev.undra.runtime.adapters.HttpError.Network)
    }

    @Test
    fun a_gzip_encoded_answer_arrives_decoded() {
        val plain = "compress me ".repeat(500).toByteArray()
        val zipped = ByteArrayOutputStream().also { out -> GZIPOutputStream(out).use { it.write(plain) } }.toByteArray()
        server.route("/gz") { ex ->
            val accepts = ex.request.header("Accept-Encoding").orEmpty().contains("gzip")
            if (accepts) ex.respond(200, listOf("Content-Encoding" to "gzip"), zipped) else ex.respond(200, body = plain)
        }
        val answer = runBlocking { AndroidHttpAdapter().request(HttpRequest(HttpMethod.GET, server.base + "/gz", emptyList(), null, null)) }
        assertEquals(String(plain), String(answer.body))
        assertFalse("the Content-Encoding header describes bytes the core never sees", answer.headers.contains(Header("Content-Encoding", "gzip")))
    }

    @Test
    fun the_hostname_localhost_resolves() {
        server.fixed("/x", 200, "by name")
        val answer = runBlocking { AndroidHttpAdapter().request(HttpRequest(HttpMethod.GET, "http://localhost:${server.port}/x", emptyList(), null, 5_000u)) }
        assertEquals("by name", String(answer.body))
    }

    @Test
    fun androids_own_bookkeeping_headers_are_not_passed_on() {
        server.fixed("/x", 200, "x")
        val answer = runBlocking { AndroidHttpAdapter().request(HttpRequest(HttpMethod.GET, server.base + "/x", emptyList(), null, null)) }
        assertTrue(answer.headers.toString(), answer.headers.none { it.name.startsWith("X-Android-", ignoreCase = true) })
    }
}
