package dev.undra.contract

import dev.undra.playground.core.RemoteConfig
import dev.undra.playground.core.StorageStatus
import dev.undra.playground.core.UndraCoreNative
import dev.undra.playground.core.UndraIds
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraRestoreException
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray

/**
 * The build-B process of the two-build steps (scenarios.md, "Two builds"; S14 steps 8 and 9, S15 steps 12 to 14).
 *
 * `run.sh` runs it after the main run, in a second JVM (`UNDRA_CONTRACT_PHASE=B`) whose `java.library.path` holds build
 * B's `libplayground_core`. Build B has no generated bindings: the core is loaded with the hash it reports
 * (`undra_schema_hash`) and driven through `UndraCore`'s raw API with ids made by `fnv1a32` (the generated
 * `StorageStatus` and `RemoteConfig` are used only as codecs: their layouts are the same in both builds). It reads what
 * build A handed over ([Handover]) and prints only `SCENARIO S14 FAIL ...` / `SCENARIO S15 FAIL ...` lines (and an
 * informational `MIGRATION ... ok`), so `check.sh`, which reads the last line of an id, keeps build A's `PASS` unless
 * build B fails. A library that is build A's is a failure of both.
 *
 * @return the number of scenarios whose build-B steps failed.
 */
fun migrationBuildB(): Int {
    val failures = sortedSetOf<String>()
    fun failed(id: String, reason: String) {
        failures += id
        val title = if (id == "S14") "offline queue replay" else "snapshot and restore"
        println("SCENARIO $id FAIL $title: build B: ${reason.replace('\n', ' ')}")
    }

    if (!UndraCoreNative.isAvailable) {
        val why = "the native core library could not be loaded: ${UndraCoreNative.unavailableReason}"
        failed("S14", why)
        failed("S15", why)
        return failures.size
    }
    val reported = UndraCoreNative.schemaHash().toULong()
    if (reported == UndraIds.SCHEMA_HASH) {
        failed("S14", "the core library is build A (schema hash 0x${reported.toString(16)}), not build B")
        failed("S15", "the core library is build A, not build B")
        return failures.size
    }
    val queue: Handover.Queue?
    val snapshots: Handover.Snapshots?
    try {
        queue = Handover.readQueue()
        snapshots = Handover.readSnapshots()
    } catch (e: Exception) {
        failed("S14", "the handover from build A does not read: $e")
        failed("S15", "the handover from build A does not read: $e")
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
        return failures.size
    }
    try {
        val connectivity = ConnectivityEvents(core)
        connectivity.changed(online = false, kind = NetKind.NONE)
        try {
            core.callSync(
                CallTarget.FreeFunction(BuildB.CONFIGURE_REMOTE),
                BuildB.CONFIGURE_REMOTE,
                RemoteConfig.encodeToByteArray(RemoteConfig(World.BASE_URL)),
            )
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
    return failures.size
}

/** The ids and routes build B is driven by. */
private object BuildB {
    val CONFIGURE_REMOTE: UInt = Fnv.fnv1a32("fn.configure_remote")
    val STORAGE_STATUS: UInt = Fnv.fnv1a32("fn.storage_status")
    val ADD: UInt = Fnv.fnv1a32("fn.add")
    val PROFILE_DESCRIBE: UInt = Fnv.fnv1a32("Profile.describe")
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
