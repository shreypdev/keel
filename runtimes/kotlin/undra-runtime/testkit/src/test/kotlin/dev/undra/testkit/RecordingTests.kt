package dev.undra.testkit

import dev.undra.testkit.testing.Suite
import dev.undra.testkit.testing.assertEq
import dev.undra.testkit.testing.assertThrows
import dev.undra.testkit.testing.assertTrue
import dev.undra.testkit.testing.fixture
import org.junit.jupiter.api.Test

class RecordingTests : Suite() {
    init {
        for (name in listOf("recording-all-kinds.json", "session-todos.json", "ports-remote-todos.json")) {
            case("$name reads and writes back byte for byte") {
                val text = fixture("fixtures/$name")
                assertEq(text, Recording.fromJson(text).toJson())
            }
        }
        case("every kind reads with its typed fields") {
            val r = Recording.fromJson(fixture("fixtures/recording-all-kinds.json"))
            assertEq(0xdeadbeefuL, r.schemaHash)
            assertEq("ios", r.platform)
            val kinds = r.events.map { it.kind::class.simpleName }.toSet()
            assertEq(setOf("Call", "Cancel", "ChangeSet", "PortEvent", "Observe", "PortCall", "PortReply", "Release", "Reply", "StreamItem", "TimerFired"), kinds)
            val page = r.events.map { it.kind }.filterIsInstance<RecordedKind.Call>().map { it.target }.filterIsInstance<RecordedTarget.Page>().single()
            assertEq(RecordedTarget.Page(0x2_0000_0003L, 0u, 50u), page)
            val observe = r.events.map { it.kind }.filterIsInstance<RecordedKind.Observe>().single()
            assertEq(UInt.MAX_VALUE, observe.signal)
        }
        case("the standard ports carry a name and only them") {
            assertEq("Http.request", standardName(portId("Http"), methodId("Http", "request")))
            assertEq("SecureStore.get", standardName(portId("SecureStore"), methodId("SecureStore", "get")))
            assertEq(null, standardName(1u, 2u))
        }
        case("strings are escaped only where JSON requires, like the other writers") {
            val text = Recording(1uL, "a\"b\\c\nd\u0001é", null, emptyList()).toJson()
            assertTrue(text.contains("\"source\": \"a\\\"b\\\\c\\nd\\u0001é\""), text)
            assertEq("a\"b\\c\nd\u0001é", Recording.fromJson(text).source)
            assertTrue(text.endsWith("\"events\": []\n}\n"))
        }
        case("what it cannot read is a typed error naming the field") {
            val head = "{\"format\":\"undra.recording\",\"version\":1,\"schema_hash\":\"0x1\",\"source\":\"t\",\"events\":"
            assertTrue(assertThrows<RecordingException> { Recording.fromJson("nope") }.message!!.contains("not valid JSON"))
            assertTrue(assertThrows<RecordingException> { Recording.fromJson("{\"format\":\"other\",\"version\":1}") }.message!!.contains("not a recording"))
            assertTrue(assertThrows<RecordingException> { Recording.fromJson("{\"format\":\"undra.recording\",\"version\":2}") }.message!!.contains("version 2"))
            val bad = assertThrows<RecordingException> { Recording.fromJson("$head[{\"t\":0,\"kind\":\"cancel\",\"call\":\"x\"}]}") }
            assertEq(0, bad.event)
            assertEq("call", bad.field)
            assertEq("kind", assertThrows<RecordingException> { Recording.fromJson("$head[{\"t\":0,\"kind\":\"warp\"}]}") }.field)
            assertEq("body", assertThrows<RecordingException> { Recording.fromJson("$head[{\"t\":0,\"kind\":\"reply\",\"call\":1,\"status\":\"ok\",\"body\":\"zz\"}]}") }.field)
            assertEq("handle", assertThrows<RecordingException> { Recording.fromJson("$head[{\"t\":0,\"kind\":\"release\",\"handle\":\"7\"}]}") }.field)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
