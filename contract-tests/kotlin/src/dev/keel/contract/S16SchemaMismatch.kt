package dev.keel.contract

import dev.keel.playground.core.KeelIds
import dev.keel.runtime.KeelCore
import dev.keel.runtime.KeelException
import dev.keel.runtime.KeelNative
import dev.keel.runtime.KeelSchemaMismatchException
import dev.keel.runtime.LoadOptions

/**
 * S16: a core built from another schema is refused before it starts. This is the scenario that loads the
 * core for everyone: the failing load comes first, then the one that has to succeed right after it.
 */
fun s16SchemaMismatch(boot: Bootstrap) {
    val generated = KeelIds.SCHEMA_HASH

    // 1. A wrong expected hash fails with the runtime's error, naming both hashes, before the core is initialised.
    val mismatch = expectFails<KeelSchemaMismatchException>("a load with a schema hash that is off by one") {
        KeelCore.load(LoadOptions(expectedSchemaHash = generated xor 1uL))
    }
    expectEq("KeelSchemaMismatchException.expected", generated xor 1uL, mismatch.expected)
    expectEq("KeelSchemaMismatchException.got", generated, mismatch.got)
    val message = mismatch.message ?: ""
    for (hash in listOf(generated xor 1uL, generated)) {
        check("0x${hash.toString(16)}" in message) { "the message does not name 0x${hash.toString(16)}: $message" }
    }
    val unloaded = expectFails<KeelException>("KeelCore.shared after a failed load") { KeelCore.shared }
    check("no KeelCore has been loaded" in (unloaded.message ?: "")) { "the failed load left a shared core behind: ${unloaded.message}" }

    // 2. The next load, with the right hash, succeeds: the failed one left nothing half-initialised.
    val world = boot.load()
    check(KeelCore.shared === world.core) { "KeelCore.shared is not the core that just loaded" }

    // 3. The hash in the bindings, the hash the core reports in its statistics and the one it exports agree.
    expectEq("stats().schema_hash", generated, world.stats().schemaHash)
    expectEq("keel_schema_hash", generated, KeelNative.schemaHash().toULong())

    // 4. The exported schema names what the playground declares, and the standard ports.
    val schema = Json.parseObject(String(KeelNative.schemaJson(), Charsets.UTF_8))
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
