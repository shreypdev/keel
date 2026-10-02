package dev.undra.contract

import java.io.File

/**
 * What build A hands over to the build-B process (scenarios.md, "Two builds"): the `Kv` contents and the failed POST's
 * `Idempotency-Key` from S14 step 7, the snapshots `P` and `L` and the `Profile` handle from S15 step 11. One small
 * JSON file per scenario in the directory `UNDRA_CONTRACT_HANDOVER` names (`run.sh` sets it to `build/migration` and
 * empties it before the run), else `build/migration` under the working directory.
 */
object Handover {
    /** S14 step 7: every key of the `Kv` and its value, and the `Idempotency-Key` of build A's failed `save_note` POST. */
    class Queue(val kv: Map<String, ByteArray>, val idempotencyKey: String)

    /** S15 step 11: snapshot `P` (a `Profile` "ada" visited twice), snapshot `L` (`P` plus a `Legacy`), the `Profile` handle. */
    class Snapshots(val profile: ByteArray, val legacy: ByteArray, val profileHandle: Long)

    private val directory: File
        get() = File(System.getenv("UNDRA_CONTRACT_HANDOVER")?.takeIf { it.isNotEmpty() } ?: "build/migration")

    private fun file(name: String) = File(directory, "$name.json")

    /** Removes the file [name]: a scenario does this first, so that a failure never leaves an older run's handover in place. */
    fun discard(name: String) {
        file(name).delete()
    }

    /** Writes S14's handover. */
    fun write(queue: Queue) {
        val kv = queue.kv.entries.sortedBy { it.key }.joinToString(",") { "${quote(it.key)}:${quote(hex(it.value))}" }
        writeText("s14", """{"kv":{$kv},"idempotencyKey":${quote(queue.idempotencyKey)}}""")
    }

    /** Writes S15's handover. */
    fun write(snapshots: Snapshots) {
        writeText(
            "s15",
            """{"profile":${quote(hex(snapshots.profile))},"legacy":${quote(hex(snapshots.legacy))},"profileHandle":${quote(snapshots.profileHandle.toULong().toString())}}""",
        )
    }

    /** S14's handover, or `null` if build A did not write it. */
    fun readQueue(): Queue? {
        val doc = readObject("s14") ?: return null
        @Suppress("UNCHECKED_CAST")
        val kv = doc["kv"] as? Map<String, Any?> ?: fail("the S14 handover has no kv object")
        return Queue(kv.mapValues { unhex(it.value as String) }, doc["idempotencyKey"] as String)
    }

    /** S15's handover, or `null` if build A did not write it. */
    fun readSnapshots(): Snapshots? {
        val doc = readObject("s15") ?: return null
        return Snapshots(unhex(doc["profile"] as String), unhex(doc["legacy"] as String), (doc["profileHandle"] as String).toULong().toLong())
    }

    private fun writeText(name: String, text: String) {
        directory.mkdirs()
        val temp = File(directory, "$name.json.tmp")
        temp.writeText(text)
        check(temp.renameTo(file(name))) { "cannot write the handover ${file(name)}" }
    }

    private fun readObject(name: String): Map<String, Any?>? {
        val f = file(name)
        return if (f.isFile) Json.parseObject(f.readText()) else null
    }

    /** A JSON string literal of [text] (keys and hex: no control characters, but quotes and backslashes are escaped anyway). */
    private fun quote(text: String): String = "\"" + text.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

    /** Lowercase hex of [bytes]. */
    fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it.toInt() and 0xff) }

    /** The bytes of the hex [text]. */
    fun unhex(text: String): ByteArray {
        require(text.length % 2 == 0) { "odd hex length" }
        return ByteArray(text.length / 2) { text.substring(2 * it, 2 * it + 2).toInt(16).toByte() }
    }
}
