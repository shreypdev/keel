package dev.undra.testkit

import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.testkit.testing.Suite
import dev.undra.testkit.testing.assertEq
import dev.undra.testkit.testing.assertTrue
import dev.undra.testkit.testing.fixture
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

// testkit/conformance/fakes.json is what the Rust fakes answer (generated from them, checked in CI). Every kit replays it against its own
// fakes, so the four implementations cannot drift apart.
private val doc = (parseJson(fixture("conformance/fakes.json")) as Json.Obj).fields

private fun section(name: String): List<Map<String, Json>> = (doc[name] as Json.Arr).items.map { (it as Json.Obj).fields }

private fun Json?.str(): String = (this as Json.Str).value

private fun Json?.long(): Long = (this as Json.Num).raw.toLong()

private fun Json?.list(): List<Json> = (this as Json.Arr).items

private fun Json?.map(): Map<String, Json> = (this as Json.Obj).fields

/** The JSON the conformance file holds for [result], re-read to compare structure rather than text. */
private fun jsonOf(text: String): Json = parseJson(text)

private fun errorJson(e: Throwable): String = when (e) {
    is FsError.NotFound -> "{\"error\":\"not_found\"}"
    is FsError.Denied -> "{\"error\":\"denied\"}"
    is FsError.Io -> "{\"error\":\"io\",\"message\":${jsonString(e.reason)}}"
    is HttpError.Network -> "{\"error\":\"network\",\"message\":${jsonString(e.reason)}}"
    is HttpError.Timeout -> "{\"error\":\"timeout\"}"
    is HttpError.Cancelled -> "{\"error\":\"cancelled\"}"
    is HttpError.InvalidUrl -> "{\"error\":\"invalid_url\",\"message\":${jsonString(e.reason)}}"
    else -> throw e
}

private fun render(j: Json): String = when (j) {
    is Json.Str -> jsonString(j.value)
    is Json.Num -> j.raw
    is Json.Bool -> j.value.toString()
    is Json.Null -> "null"
    is Json.Arr -> j.items.joinToString(",", "[", "]") { render(it) }
    is Json.Obj -> j.fields.entries.sortedBy { it.key }.joinToString(",", "{", "}") { "${jsonString(it.key)}:${render(it.value)}" }
}

private fun same(expected: Json?, actualJson: String, what: String) = assertEq(render(expected ?: Json.Null), render(jsonOf(actualJson)), what)

private fun hexList(items: List<String>) = items.joinToString(",", "[", "]") { jsonString(it) }

class ConformanceTests : Suite() {
    init {
        case("SeededRng gives the same bytes for the same seed") {
            for (c in section("rng")) {
                val seed = when (val s = c["seed"]) {
                    is Json.Str -> parseHex64(s.value)!!
                    else -> s.long().toULong()
                }
                val rng = SeededRng(seed)
                val got = c["fills"].list().map { rng.fill(it.long().toUInt()).toHex() }
                same(c["results"], hexList(got), "seed $seed")
            }
        }
        case("FakeClock fires timers in deadline order with the clock at each deadline") {
            for (c in section("clock")) {
                val clock = FakeClock(c["now_ms"].long())
                fun state() = "{\"now_ms\":${clock.nowMs},\"monotonic_ns\":${clock.monotonicNs}}"
                for (step in c["steps"].list().map { it.map() }) {
                    when (step["op"].str()) {
                        "timer" -> clock.set(step["id"].long().toUInt(), step["delay_ms"].long().toULong())
                        "advance" -> {
                            val fired = clock.advance(step["ms"].long())
                            same(step["fired"], fired.joinToString(",", "[", "]"), "fired")
                            same(step["state"], state(), "state after advance")
                        }
                        "set_now" -> {
                            clock.setNowMs(step["ms"].long())
                            same(step["state"], state(), "state after set_now")
                        }
                    }
                }
            }
        }
        case("MemKv stores bytes ordered by UTF-8, not UTF-16") {
            runBlocking {
                for (c in section("store")) {
                    val kv = MemKv()
                    for (step in c["steps"].list().map { it.map() }) {
                        when (step["op"].str()) {
                            "set" -> kv.set(step["key"].str(), step["value"].str().fromHex()!!)
                            "delete" -> kv.delete(step["key"].str())
                            "list" -> same(step["result"], hexList(kv.list(step["prefix"].str())), "list ${step["prefix"].str()}")
                            "get" -> same(step["result"], kv.get(step["key"].str())?.let { jsonString(it.toHex()) } ?: "null", "get ${step["key"].str()}")
                        }
                    }
                }
            }
        }
        case("MemFs has the semantics the platform adapters share") {
            runBlocking {
                for (c in section("fs")) {
                    val fs = MemFs()
                    for (step in c["steps"].list().map { it.map() }) {
                        val path = (step["path"] ?: step["dir"]).str()
                        val got = try {
                            when (step["op"].str()) {
                                "write" -> { fs.write(path, step["data"].str().fromHex()!!); "null" }
                                "read" -> jsonString(fs.read(path).toHex())
                                "delete" -> { fs.delete(path); "null" }
                                else -> hexList(fs.list(path))
                            }
                        } catch (e: FsError) {
                            errorJson(e)
                        }
                        same(step["result"], got, "${step["op"].str()} $path")
                    }
                }
            }
        }
        case("FakeHttp answers the first matching rule and fails the rest") {
            runBlocking {
                for (c in section("http")) {
                    val fakes = Fakes()
                    Seed.fromJson("{\"http\":${render(c["rules"]!!)}}").apply(fakes)
                    for (r in c["requests"].list().map { it.map() }) {
                        val method = HttpMethod.valueOf(r["method"].str().uppercase())
                        val got = try {
                            val resp = fakes.http.request(HttpRequest(method, r["url"].str()))
                            val headers = resp.headers.joinToString(",", "[", "]") { "[${jsonString(it.name)},${jsonString(it.value)}]" }
                            "{\"status\":${resp.status},\"headers\":$headers,\"body\":${jsonString(resp.body.toHex())}}"
                        } catch (e: HttpError) {
                            errorJson(e)
                        }
                        same(r["result"], got, "${r["method"].str()} ${r["url"].str()}")
                    }
                }
            }
        }
        case("the example seed reads and seeds the fakes") {
            val seed = Seed.fromJson(fixture("fixtures/seed.json"))
            assertEq(1_700_000_000_000L, seed.nowMs)
            assertEq(42uL, seed.rngSeed)
            val fakes = Fakes()
            seed.apply(fakes)
            assertEq("hello", fakes.kv.value("greeting")!!.decodeToString())
            assertEq("00ff", fakes.kv.value("blob")!!.toHex())
            assertEq("t-123", fakes.secureStore.value("token")!!.decodeToString())
            assertEq("hello", fakes.fs.contents("notes/a.txt")!!.decodeToString())
            assertEq("a0a39b71b74ace56", fakes.rng.fill(8u).toHex())
            assertEq(1_700_000_000_000L, fakes.clock.nowMs)
            runBlocking {
                val resp = fakes.http.request(HttpRequest(HttpMethod.GET, "https://api.test/lists/inbox/todos"))
                assertEq(200, resp.status.toInt())
                assertEq("application/json", resp.headers.single().value)
            }
            assertTrue(fakes.http.calls.size == 1)
        }
        case("a seed names the path of the first bad value") {
            fun path(text: String): String = try {
                Seed.fromJson(text)
                "no error"
            } catch (e: SeedException) {
                e.path
            }
            assertEq("kv.a", path("{\"kv\": {\"a\": 1}}"))
            assertEq("http[0].status", path("{\"http\": [{\"status\": \"x\"}]}"))
            assertEq("http[0].method", path("{\"http\": [{\"method\": \"fetch\"}]}"))
            assertEq("connectivity.kind", path("{\"connectivity\": {\"online\": true, \"kind\": \"5g\"}}"))
            assertEq("lifecycle", path("{\"lifecycle\": 3}"))
            assertEq("$", path("[]"))
            assertEq("version", path("{\"version\": 2}"))
            assertEq("$", path("{"))
            val applied = assertThrowsSeed { Seed.fromJson("{\"fs\": {\"../x\": \"y\"}}").apply(Fakes()) }
            assertEq("fs.../x", applied)
        }
    }

    private fun assertThrowsSeed(block: () -> Unit): String = try {
        block()
        "no error"
    } catch (e: SeedException) {
        e.path
    }

    @Test
    fun allCases() = assertPassed()
}
