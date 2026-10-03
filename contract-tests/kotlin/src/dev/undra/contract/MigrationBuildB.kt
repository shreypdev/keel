package dev.undra.contract

import dev.undra.playground.core.QueryStatus
import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.RemoteTodo
import dev.undra.playground.core.StorageStatus
import dev.undra.playground.core.UndraCoreNative
import dev.undra.playground.core.UndraIds
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.UndraRestoreException
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The build-B process of the two-build steps (scenarios.md, "Two builds"; S14 steps 8 and 9, S15 steps 12 to 14, S35 step 10).
 *
 * `run.sh` runs it after the main run, in a second JVM (`UNDRA_CONTRACT_PHASE=B`) whose `java.library.path` holds build
 * B's `libplayground_core`. Build B has no generated bindings: the core is loaded with the hash it reports
 * (`undra_schema_hash`) and driven through `UndraCore`'s raw API with ids made by `fnv1a32` (the generated
 * `StorageStatus`, `RemoteConfig`, `QueryStatus` and `RemoteTodo` are used only as codecs: their layouts are the same in
 * both builds). It reads what build A handed over ([Handover]) and prints only `SCENARIO S14 FAIL ...` /
 * `SCENARIO S15 FAIL ...` / `SCENARIO S35 FAIL ...` lines (and an informational `MIGRATION ... ok`), so `check.sh`, which
 * reads the last line of an id, keeps build A's `PASS` unless build B fails. A library that is build A's is a failure of all
 * three. S14's and S15's steps share the first core; S35's step 10 gets a new one, loaded after the first is closed, as it
 * needs a fresh runtime.
 *
 * @return the number of scenarios whose build-B steps failed.
 */
fun migrationBuildB(): Int {
    val failures = sortedSetOf<String>()
    fun failed(id: String, reason: String) {
        failures += id
        val title = when (id) {
            "S14" -> "offline queue replay"
            "S15" -> "snapshot and restore"
            else -> "query handles across a restore"
        }
        println("SCENARIO $id FAIL $title: build B: ${reason.replace('\n', ' ')}")
    }

    if (!UndraCoreNative.isAvailable) {
        val why = "the native core library could not be loaded: ${UndraCoreNative.unavailableReason}"
        failed("S14", why)
        failed("S15", why)
        failed("S35", why)
        return failures.size
    }
    val reported = UndraCoreNative.schemaHash().toULong()
    if (reported == UndraIds.SCHEMA_HASH) {
        failed("S14", "the core library is build A (schema hash 0x${reported.toString(16)}), not build B")
        failed("S15", "the core library is build A, not build B")
        failed("S35", "the core library is build A, not build B")
        return failures.size
    }
    val queue: Handover.Queue?
    val snapshots: Handover.Snapshots?
    val queryHandles: Handover.QueryHandles?
    try {
        queue = Handover.readQueue()
        snapshots = Handover.readSnapshots()
        queryHandles = Handover.readQueryHandles()
    } catch (e: Exception) {
        failed("S14", "the handover from build A does not read: $e")
        failed("S15", "the handover from build A does not read: $e")
        failed("S35", "the handover from build A does not read: $e")
        return failures.size
    }

    // 8. Build B is loaded with exactly build A's Kv contents (no injected failure), offline.
    val server = FakeServer()
    val kv = MemoryKv(queue?.kv ?: emptyMap())
    val log = CapturingLog()
    server.fail(HttpMethod.POST, BuildB.NOTES, HttpError.Network("offline"))
    val core = try {
        UndraCore.load(
            LoadOptions(
                expectedSchemaHash = reported,
                adapters = mapOf(
                    StandardPorts.Clock.PORT_ID to ManualClock().portImpl(),
                    StandardPorts.Http.PORT_ID to server.portImpl(),
                    StandardPorts.Kv.PORT_ID to kv.portImpl(),
                    StandardPorts.Log.PORT_ID to log.portImpl(),
                ),
            ),
            // Build B's library has build A's namespace, so the bindings' natives object binds it.
            UndraCoreNative,
        )
    } catch (e: Exception) {
        failed("S14", "build B did not load: $e")
        failed("S15", "build B did not load: $e")
        failed("S35", "build B did not load: $e")
        return failures.size
    }
    try {
        val connectivity = ConnectivityEvents(core)
        connectivity.changed(online = false, kind = NetKind.NONE)
        try {
            configureRemote(core)
        } catch (e: Exception) {
            failed("S14", "configure_remote in build B failed: $e")
        }
        if ("S14" !in failures) {
            try {
                val handed = queue ?: fail("the Kv contents of build A's S14 step 7 are missing (did build A's S14 pass?)")
                queueSteps(core, connectivity, server, kv, handed.idempotencyKey)
            } catch (e: Throwable) {
                failed("S14", e.message ?: e.toString())
            }
        }
        try {
            val handed = snapshots ?: fail("the snapshots of build A's S15 step 11 are missing (did build A's S15 pass?)")
            snapshotSteps(core, log, handed)
        } catch (e: Throwable) {
            failed("S15", e.message ?: e.toString())
        }
    } finally {
        core.close()
    }
    // S35 step 10 runs on a new core (a fresh runtime: the one above holds what S14's and S15's steps made).
    try {
        val handed = queryHandles ?: fail("the snapshot and the handles of build A's S35 step 2 are missing (did build A's S35 pass?)")
        queryHandleSteps(reported, handed)
    } catch (e: Throwable) {
        failed("S35", e.message ?: e.toString())
    }
    return failures.size
}

/** The ids and routes build B is driven by. */
private object BuildB {
    val CONFIGURE_REMOTE: UInt = Fnv.fnv1a32("fn.configure_remote")
    val STORAGE_STATUS: UInt = Fnv.fnv1a32("fn.storage_status")
    val ADD: UInt = Fnv.fnv1a32("fn.add")
    val PROFILE_DESCRIBE: UInt = Fnv.fnv1a32("Profile.describe")
    val QUERY_REFETCH: UInt = Fnv.fnv1a32("query.refetch")
    const val NOTES: String = "${World.BASE_URL}/lists/s14m/notes"
}

/** S14 steps 8 and 9. */
private fun queueSteps(core: UndraCore, connectivity: ConnectivityEvents, server: FakeServer, kv: MemoryKv, idempotencyKey: String) {
    // 8. Build B reads the queue: save_note migrates by parameter name, tag_note is a dead letter, nothing is sent.
    awaitUntil("build B to read build A's queue (pending 1, migrated 1, dead-lettered 1)") {
        val status = storageStatus(core)
        status.queueReadable && status.pending == 1u && status.migrated == 1uL && status.deadLettered == 1uL
    }
    val status = storageStatus(core)
    expectEq("the dead letters in build B", 1, status.deadLetters.size)
    val letter = status.deadLetters.single()
    check(letter.startsWith("tag_note: ") && "does not migrate" in letter) { "the dead letter does not say tag_note's input does not migrate: $letter" }
    expectEq("requests build B made while offline", 0, server.requests.size)

    // 9. Online, the migrated save_note replays once with build A's idempotency key; the dead letter stays.
    server.respond(HttpMethod.POST, BuildB.NOTES, 201, "{}")
    connectivity.changed(online = true, kind = NetKind.WIFI)
    awaitUntil("the migrated save_note to replay") { storageStatus(core).pending == 0u && server.count(HttpMethod.POST, BuildB.NOTES) > 0 }
    holdsFor("no second replay", 200) { server.count(HttpMethod.POST, BuildB.NOTES) == 1 }
    val post = server.requestsFor(HttpMethod.POST, BuildB.NOTES).single()
    expectEq("the body of the replayed save_note (pinned is None)", "save:a", post.bodyText())
    expectEq("the Idempotency-Key of the replay", idempotencyKey, post.header("Idempotency-Key"))
    val after = storageStatus(core)
    expectEq("pending after the replay", 0u, after.pending)
    expectEq("the dead letters after the replay", status.deadLetters, after.deadLetters)
    check(kv.value(Persisted.DEAD_LETTER_KEY) != null) { "the Kv holds no ${Persisted.DEAD_LETTER_KEY}" }
    println("MIGRATION S14 build B ok: $letter")
}

/** S15 steps 12 to 14. */
private fun snapshotSteps(core: UndraCore, log: CapturingLog, handed: Handover.Snapshots) {
    val profile = handed.profileHandle

    // 12. P restores by name: the same handle describes build B's Profile, with the default theme.
    core.restore(handed.profile)
    expectEq("the Profile restored into build B", "name=ada;visits=2;theme=", describe(core, profile))

    // 13. L is refused as incompatible (Legacy.score changed from i32 to String), with an ERROR record naming both.
    val logBefore = log.records.size
    val refused = expectFails<UndraRestoreException>("restore of L, whose Legacy.score cannot migrate") { core.restore(handed.legacy) }
    expectEq("the restore code of L in build B", UndraRestoreException.INCOMPATIBLE, refused.code)
    check(refused.isIncompatible) { "UndraRestoreException($refused) is not incompatible" }
    val errors = log.records.drop(logBefore).filter { it.level >= 4 }
    check(errors.any { "Legacy" in it.message && "score" in it.message }) { "no ERROR record names Legacy and score: ${errors.map { it.message }}" }
    expectEq("the Profile after the refused restore", "name=ada;visits=2;theme=", describe(core, profile))
    expectEq("a call after the refused restore", 42, add(core, 40, 2))

    // 14. A snapshot from before layout 2 is malformed in build B too, and the core is unchanged.
    val layout1 = expectFails<UndraRestoreException>("restore of the 8-byte snapshot of the layout before ADR-037") { core.restore(ByteArray(8)) }
    expectEq("the restore code of a layout-1 snapshot in build B", UndraRestoreException.BAD_SNAPSHOT, layout1.code)
    expectEq("the Profile after the malformed snapshot", "name=ada;visits=2;theme=", describe(core, profile))
    println("MIGRATION S15 build B ok")
}

private fun storageStatus(core: UndraCore): StorageStatus =
    StorageStatus.decodeAll(core.callSync(CallTarget.FreeFunction(BuildB.STORAGE_STATUS), BuildB.STORAGE_STATUS, ByteArray(0)))

private fun describe(core: UndraCore, profile: Long): String =
    Codecs.string.decodeAll(core.callSync(CallTarget.ObjectMethod(Handle(profile), BuildB.PROFILE_DESCRIBE), BuildB.PROFILE_DESCRIBE, ByteArray(0)))

private fun add(core: UndraCore, a: Int, b: Int): Int {
    val w = UndraWriter()
    w.writeI32(a)
    w.writeI32(b)
    return Codecs.i32.decodeAll(core.callSync(CallTarget.FreeFunction(BuildB.ADD), BuildB.ADD, w.toByteArray()))
}

/** What a restore of a snapshot adds to a fresh core in S35 step 10: the stores `Counter` and `Library`, its two page servers, and the three query handles build B honours. */
private const val S35_RE_ISSUED: Long = 7L

/**
 * A handle observed through the raw API that this process did not construct (the handle came from build A's snapshot): a mirror
 * registration that records the entries delivered for it, and `observe` on every signal, as [RawStore] does for a handle it made.
 */
private class Watch(private val core: UndraCore, val handle: Long) {
    /** The entries delivered so far, oldest first. */
    val entries = CopyOnWriteArrayList<RawStore.Entry>()

    init {
        core.mirror.register(handle) { signalId, op, reader -> entries.add(RawStore.Entry(signalId, op, reader.readRemaining())) }
    }

    /** Observes every signal; returns once the values were delivered. */
    fun observe() {
        core.observe(handle, RawStore.ALL_SIGNALS, true)
    }

    /** The oldest entry of signal [signalId], or `null`. */
    fun first(signalId: UInt): RawStore.Entry? = entries.firstOrNull { it.signalId == signalId }

    /** The newest entry of signal [signalId], or `null`. */
    fun last(signalId: UInt): RawStore.Entry? = entries.lastOrNull { it.signalId == signalId }
}

/** The raw call of the synchronous method [methodId] (no arguments) of the object [handle]. */
private fun callSync(core: UndraCore, handle: Long, methodId: UInt): ByteArray =
    core.callSync(CallTarget.ObjectMethod(Handle(handle), methodId), methodId, ByteArray(0))

private fun configureRemote(core: UndraCore) {
    core.callSync(
        CallTarget.FreeFunction(BuildB.CONFIGURE_REMOTE),
        BuildB.CONFIGURE_REMOTE,
        RemoteConfig.encodeToByteArray(RemoteConfig(World.BASE_URL)),
    )
}

private fun liveHandles(core: UndraCore): Long = core.readStats().liveHandles

/**
 * S35 step 10 (ADR-059): loads a new core of build B (the first was closed: S14's and S15's steps left their stores in it, and the
 * step counts what a restore adds to a fresh runtime), over an empty `Kv`, a new `FakeServer` and a new `CapturingLog`, restores the
 * snapshot of build A's step 2 into it and uses the handles of step 1. The handle of `roster` is refused, as build B changed the
 * type of its parameter; `remote_todos`, `ticker` and the library's store did not change.
 */
private fun queryHandleSteps(schemaHash: ULong, handed: Handover.QueryHandles) {
    val server = FakeServer()
    val log = CapturingLog()
    val core = UndraCore.load(
        LoadOptions(
            expectedSchemaHash = schemaHash,
            adapters = mapOf(
                StandardPorts.Clock.PORT_ID to ManualClock().portImpl(),
                StandardPorts.Http.PORT_ID to server.portImpl(),
                StandardPorts.Kv.PORT_ID to MemoryKv().portImpl(),
                StandardPorts.Log.PORT_ID to log.portImpl(),
            ),
        ),
        UndraCoreNative,
    )
    try {
        restoreQueryHandles(core, server, log, handed)
    } finally {
        core.close()
    }
}

/** The steps of [queryHandleSteps] on the new core. */
private fun restoreQueryHandles(core: UndraCore, server: FakeServer, log: CapturingLog, handed: Handover.QueryHandles) {
    val url = "${World.BASE_URL}/lists/s35/todos"
    val milk = RemoteTodo(1u, "Buy milk", false)
    val dog = RemoteTodo(2u, "Walk the dog", false)
    val gets = { server.count(HttpMethod.GET, url) }
    val allGets = { server.requests.count { it.method == HttpMethod.GET } }
    val todos = Codecs.option(Codecs.vec(RemoteTodo))
    check(BuildB.QUERY_REFETCH == UndraIds.Objects.RemoteTodosQueryHandle.REFETCH) { "fnv1a32(\"query.refetch\") is not the refetch id of the query handles" }

    // 10. The runner reads live_handles, then restore(snapshot) succeeds.
    val handlesBefore = liveHandles(core)
    val getsBefore = allGets()
    core.restore(handed.snapshot)
    expectEq("live_handles grown by the restore (2 stores, 2 page servers, 3 query handles; the roster handle is not counted)", handlesBefore + S35_RE_ISSUED, liveHandles(core))
    // No GET was made yet: a restore reads no clock and starts no fetch (R12).
    quietFor()
    expectEq("GET requests since the restore", getsBefore, allGets())

    // configure_remote is core state outside stores (S22): configuring is not using the handle.
    configureRemote(core)
    server.respond(HttpMethod.GET, url, 200, """[{"id":1,"title":"Buy milk","done":false},{"id":2,"title":"Walk the dog","done":false}]""", delayMs = 300)
    quietFor()
    expectEq("GET requests for the list after configure_remote", 0, gets())

    // observe(remote handle) is answered with status = fetching (wire value 1, a u16) and data absent, then the data.
    val remote = Watch(core, handed.remote)
    remote.observe()
    flushMainThread()
    val status = remote.first(1u) ?: fail("observing the remote handle delivered no status: ${remote.entries}")
    expectEq("the status entry of the observe (a u16)", listOf<Byte>(1, 0), status.value.toList())
    expectEq("the status the observe answered", QueryStatus.FETCHING, QueryStatus.decodeAll(status.value))
    val absent = remote.first(0u) ?: fail("observing the remote handle delivered no data: ${remote.entries}")
    expectEq("the data the observe answered (nothing cached)", null, todos.decodeAll(absent.value))
    awaitUntil("the data fetched by the fresh core") { remote.last(0u)?.let { todos.decodeAll(it.value) } == listOf(milk, dog) }
    expectEq("GET requests for the list after the first fetch", 1, gets())

    // refetch on it is status 0 (callSync throws for anything else) and makes one more GET.
    callSync(core, handed.remote, BuildB.QUERY_REFETCH)
    awaitUntil("the GET of refetch") { gets() == 2 }
    holdsFor("exactly one more GET", 200) { gets() == 2 }

    // The ticker's handle delivers ticks, from the fresh core's own counter: it is observed again like any handle after a reload.
    val ticker = Watch(core, handed.ticker)
    ticker.observe()
    awaitUntil("two ticks of the fresh core") { (ticker.last(0u)?.let { Codecs.option(Codecs.u32).decodeAll(it.value) } ?: 0u) >= 2u }

    // Library's books (observed again) pages through the server its op 0 names (a new server in this core).
    val library = Watch(core, handed.library)
    library.observe()
    flushMainThread()
    val books = Payloads.LazyValue.decode((library.first(0u) ?: fail("observing the library delivered no books entry: ${library.entries}")).value)
    expectEq("the length in the books entry", 10_000u, books.len)
    val page = pageHeader(core, books.handle, 0u, 3u)
    expectEq("the total of the page call through the server the books entry names", 10_000u, page.total)
    expectEq("the rows of that page", 3u, page.count)

    // The handle of roster is refused (status 5), and the Log port received a WARN that names it.
    val refused = expectFails<UndraReplyException>("refetch on the roster handle, whose query's parameter type changed") {
        callSync(core, handed.roster, BuildB.QUERY_REFETCH)
    }
    expectBadRequest("refetch on the roster handle", refused)
    val named = "Handle(index=${handed.roster and 0xFF_FFFFL}, gen=${handed.roster ushr 24})"
    val warns = log.records.filter { it.level == 3 && named in it.message }
    check(warns.isNotEmpty()) { "no WARN names $named: ${log.records.filter { it.level >= 3 }.map { it.message }}" }
    check("is not re-issued" in warns.first().message) { "the WARN does not say the handle is not re-issued: ${warns.first().message}" }
    check("the types it was made from changed" in warns.first().message) { "the WARN does not say why: ${warns.first().message}" }
    println("MIGRATION S35 build B ok")
}
