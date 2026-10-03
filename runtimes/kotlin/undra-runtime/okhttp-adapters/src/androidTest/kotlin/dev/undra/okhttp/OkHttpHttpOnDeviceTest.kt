package dev.undra.okhttp

import android.os.Handler
import android.os.Looper
import dev.undra.android.TestHttpServer
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** What only a device can show about OkHttp's Http adapter: it never works on the main thread, and `localhost` resolves. */
class OkHttpHttpOnDeviceTest {
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
        val client = OkHttpClient.Builder().addInterceptor(Interceptor { chain -> threads.add(Thread.currentThread()); chain.proceed(chain.request()) }).build()
        server.fixed("/x", 200, "ok")
        val answer = runBlocking {
            withContext(main) {
                assertTrue("the caller is on the main thread", Looper.myLooper() === Looper.getMainLooper())
                OkHttpHttpAdapter(client).request(HttpRequest(HttpMethod.GET, server.base + "/x", emptyList(), null, null))
            }
        }
        assertEquals("ok", String(answer.body))
        assertEquals(1, threads.size)
        assertFalse("the interceptor, and so the connection, ran on the main thread", threads.single() === Looper.getMainLooper().thread)
        assertTrue(threads.single().name, threads.single().name.contains("OkHttp"))
    }

    @Test
    fun a_request_that_fails_from_the_main_thread_fails_with_a_typed_error_not_a_strictmode_crash() {
        val failure = runBlocking {
            withContext(main) {
                try {
                    OkHttpHttpAdapter(OkHttpClient()).request(HttpRequest(HttpMethod.GET, "http://127.0.0.1:1/", emptyList(), null, 5_000u))
                    null
                } catch (e: HttpError) {
                    e
                }
            }
        }
        assertNotNull(failure)
        assertTrue(failure.toString(), failure is HttpError.Network)
    }

    @Test
    fun the_hostname_localhost_resolves() {
        server.fixed("/x", 200, "by name")
        val answer = runBlocking { OkHttpHttpAdapter(OkHttpClient()).request(HttpRequest(HttpMethod.GET, "http://localhost:${server.port}/x", emptyList(), null, 5_000u)) }
        assertEquals("by name", String(answer.body))
    }

    @Test
    fun androids_own_bookkeeping_headers_are_not_there_to_pass_on() {
        server.fixed("/x", 200, "x")
        val answer = runBlocking { OkHttpHttpAdapter(OkHttpClient()).request(HttpRequest(HttpMethod.GET, server.base + "/x", emptyList(), null, null)) }
        assertTrue(answer.headers.toString(), answer.headers.none { it.name.startsWith("X-Android-", ignoreCase = true) })
    }
}
