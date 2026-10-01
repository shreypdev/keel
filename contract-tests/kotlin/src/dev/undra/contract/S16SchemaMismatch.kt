package dev.undra.contract

import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.Counter
import dev.undra.playground.core.add
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraNative
import dev.undra.runtime.UndraTransportException
import dev.undra.runtime.UndraSchemaMismatchException
import dev.undra.runtime.LoadOptions

/**
 * S16: a core built from another schema is refused before it starts. This is the scenario that loads the
 * core for everyone: the failing load comes first, then the one that has to succeed right after it.
 */
fun s16SchemaMismatch(boot: Bootstrap) {
    val generated = UndraIds.SCHEMA_HASH

    // 1. A wrong expected hash fails with the runtime's error, naming both hashes, before the core is initialised.
    val mismatch = expectFails<UndraSchemaMismatchException>("a load with a schema hash that is off by one") {
        UndraCore.load(LoadOptions(expectedSchemaHash = generated xor 1uL))
    }
    expectEq("UndraSchemaMismatchException.expected", generated xor 1uL, mismatch.expected)
    expectEq("UndraSchemaMismatchException.got", generated, mismatch.got)
    val message = mismatch.message ?: ""
    for (hash in listOf(generated xor 1uL, generated)) {
        check("0x${hash.toString(16)}" in message) { "the message does not name 0x${hash.toString(16)}: $message" }
    }
    expectNoCoreLoaded()

    // 2. The next load, with the right hash, succeeds: the failed one left nothing half-initialised.
    val world = boot.load()
    check(UndraCore.shared === world.core) { "UndraCore.shared is not the core that just loaded" }

    // 3. The hash in the bindings, the hash the core reports in its statistics and the one it exports agree.
    expectEq("stats().schema_hash", generated, world.stats().schemaHash)
    expectEq("undra_schema_hash", generated, UndraNative.schemaHash().toULong())

    // 4. The exported schema names what the playground declares, and the standard ports.
    val schema = Json.parseObject(String(UndraNative.schemaJson(), Charsets.UTF_8))
    val names = { section: String -> (schema[section] as? List<*> ?: fail("the schema has no \"$section\"")).map { (it as Map<*, *>)["name"] } }
    val objects = names("objects")
    for (type in listOf("Todos", "Counter", "BigList", "Bench", "Probe")) check(type in objects) { "the schema has no object $type: $objects" }
    val queries = names("queries")
    for (query in listOf("remote_todos", "post_remote_todo", "patch_remote_todo")) check(query in queries) { "the schema has no query $query: $queries" }
    val ports = names("ports")
    for (port in listOf("Clock", "Connectivity", "Fs", "Http", "Kv", "Lifecycle", "Log", "Rng", "SecureStore", "Timer")) {
        check(port in ports) { "the schema has no standard port $port: $ports" }
    }
}

/**
 * S16.5: with no core loaded, `UndraCore.shared` does not throw on access: it is a closed placeholder whose generated
 * calls fail as `Unavailable`, and `UndraCore.current` stays null. The failed load of step 1 left nothing behind.
 */
private fun expectNoCoreLoaded() {
    check(UndraCore.current == null) { "the failed load left a core behind: ${UndraCore.current}" }
    val shared = UndraCore.shared
    check(UndraCore.current == null) { "UndraCore.shared became a loaded core" }
    val call = expectFails<UndraCallError.Unavailable>("add(1, 2) with no core loaded") { add(1, 2) }
    expectEq("the transport reason of add(1, 2) with no core loaded", UndraTransportException.Reason.CLOSED, call.transport.reason)
    check("UndraCore.load" in (call.message ?: "")) { "the failure does not say to load a core: ${call.message}" }
    val constructor = expectFails<UndraCallError.Unavailable>("Counter.create() with no core loaded") { Counter.create() }
    expectEq("the transport reason of Counter.create() with no core loaded", UndraTransportException.Reason.CLOSED, constructor.transport.reason)
    shared.report(call, "Counter.increment")
    check(UndraCore.current == null) { "the placeholder became the loaded core after a report" }
}
