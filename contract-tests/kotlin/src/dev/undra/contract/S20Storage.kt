package dev.undra.contract

import dev.undra.playground.core.RemoteTodo
import dev.undra.playground.core.RemoteTodosQueryHandle
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.add
import dev.undra.playground.core.storageStatus
import dev.undra.runtime.UndraCore
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.encodeToByteArray

/**
 * S20, the native variant (scenarios.md, platform notes): storage failures are typed (ADR-049). Steps 1, 2, 4 (what the
 * harness's failed first read of the queue, when S16 loaded the core, did) and 5; step 3 needs a fresh core and is not
 * run on native.
 */
fun s20Storage(w: World) {
    w.configureRemoteOnce()
    val core = w.core
    val list = "s20"
    val url = "${World.BASE_URL}/lists/$list/todos"
    val panicsBefore = w.stats().panics
    val logBefore = w.log.records.size
    val statusBefore = storageStatus()
    val key = Persisted.cacheKey(UndraIds.Queries.REMOTE_TODOS, Codecs.string.encodeToByteArray(list))
    val item = RemoteTodo(1u, "Stored", false)
    w.server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Stored","done":false}]""")

    try {
        // 1. Every write fails Full: the item shows, nothing of it is stored, one WARN says so, nothing panicked.
        w.kv.fail(MemoryKv.Kind.SET, error = StorageError.Full)
        val handle = RemoteTodosQueryHandle.create(list)
        try {
            awaitEq("the s20 item", listOf(item)) { handle.data.value }
            Thread.sleep(300) // the write is debounced 250 ms
            awaitUntil("the failed write to be counted") { storageStatus().writeFailed > statusBefore.writeFailed }
            check(w.kv.value(key) == null) { "$key is in the Kv although every write failed" }
            check(w.kv.operations.none { it.key == key && it.isSet }) { "a write of $key succeeded" }
            check(w.kv.operations.any { it.key == key && it.kind == MemoryKv.Kind.SET && it.failure == StorageError.Full }) {
                "no write of $key was attempted (and failed Full): ${w.kv.operations.filter { it.key.startsWith("undra.query.cache2.") }}"
            }
            val warnings = w.log.records.drop(logBefore).filter { it.level == 3 && it.target == "undra::query" && "storage is full" in it.message }
            expectEq("WARN records of undra::query saying the storage is full", 1, warnings.size)
            expectEq("panics after the failed writes", panicsBefore, w.stats().panics)
            expectEq("a call after the failed writes", 3, add(1, 2))

            // 2. The Kv heals: a refetch stores the entry, in format 2, next to the description of its type.
            w.kv.heal()
            handle.invalidate()
            val stored = awaitValue("the s20 entry to be stored") { w.kv.value(key) }
            expectEq("the format of the stored entry", listOf<Byte>(2, 0), stored.take(2))
            val fingerprint = Persisted.fingerprint(stored, 10) ?: fail("the stored entry has no fingerprint: ${stored.size} bytes")
            val typesKey = Persisted.typesKey(fingerprint)
            check(w.kv.value(typesKey) != null) { "the Kv holds no $typesKey for the stored entry" }
            awaitEq("the data after the refetch", listOf(item)) { handle.data.value }
        } finally {
            handle.close()
        }

        // 3. Not run on native: the core is not reloaded (scenarios.md, platform notes).

        // 4. The harness failed the first read of the queue (Locked, at load). The queue is readable now, it was read
        // again, and nothing wrote it in between.
        check(storageStatus().queueReadable) { "the queue is still unreadable" }
        val operations = w.kv.operations
        val failed = operations.indexOfFirst { it.kind == MemoryKv.Kind.GET && it.key == Persisted.QUEUE_KEY && it.failure == StorageError.Locked }
        check(failed >= 0) { "the failed read of ${Persisted.QUEUE_KEY} at load is not in the Kv's log" }
        val reread = (failed + 1 until operations.size).firstOrNull { operations[it].kind == MemoryKv.Kind.GET && operations[it].key == Persisted.QUEUE_KEY }
            ?: fail("${Persisted.QUEUE_KEY} was never read again after the failed read")
        check(operations[reread].failure == null) { "the second read of the queue failed too: ${operations[reread]}" }
        check(operations.subList(failed + 1, reread).none { it.kind == MemoryKv.Kind.SET && it.key == Persisted.QUEUE_KEY }) {
            "${Persisted.QUEUE_KEY} was written while it could not be read: ${operations.subList(failed + 1, reread)}"
        }
    } finally {
        w.kv.heal()
    }

    // 5. Nothing panicked, and this is the core the scenario started with.
    expectEq("panics during the scenario", panicsBefore, w.stats().panics)
    check(UndraCore.current === core) { "the core was reloaded during the scenario" }
}
