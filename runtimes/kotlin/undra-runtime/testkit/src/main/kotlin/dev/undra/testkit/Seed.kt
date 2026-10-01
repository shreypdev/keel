package dev.undra.testkit

import dev.undra.runtime.UndraException
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.NetKind

/** The `version` of the seed document this kit reads. */
public const val SEED_VERSION: Int = 1

/** Why a seed could not be read or applied: [path] says where (`http[1].status`). */
public class SeedException(public val path: String, problem: String) : UndraException("seed $path: $problem")

/** One scripted HTTP rule of a [Seed]. */
public class SeedHttpRule(
    public val url: String? = null,
    public val urlPrefix: String? = null,
    public val method: HttpMethod? = null,
    public val reply: HttpReply,
)

/**
 * The starting state of the fakes: the same document seeds `undra::ports::fakes` in Rust and the fakes of the Swift and TypeScript kits
 * (`testkit/fixtures/seed.json` is an example). Every field is optional. Build one with its constructor or read one with [fromJson], then
 * [apply] it to [Fakes].
 */
public class Seed(
    public val nowMs: Long? = null,
    public val rngSeed: ULong? = null,
    public val kv: List<Pair<String, ByteArray>> = emptyList(),
    public val secureStore: List<Pair<String, ByteArray>> = emptyList(),
    public val fs: List<Pair<String, ByteArray>> = emptyList(),
    public val http: List<SeedHttpRule> = emptyList(),
    public val connectivity: Pair<Boolean, NetKind>? = null,
    public val lifecycle: AppState? = null,
) {
    /**
     * Puts the seed into [fakes].
     *
     * @throws SeedException for an `fs` path the fake refuses (an empty path, `..`, a file in the way).
     */
    public fun apply(fakes: Fakes) {
        nowMs?.let { fakes.clock.setNowMs(it) }
        rngSeed?.let { fakes.rng.reseed(it) }
        for ((key, value) in kv) fakes.kv.insert(key, value)
        for ((key, value) in secureStore) fakes.secureStore.insert(key, value)
        for ((path, contents) in fs) {
            try {
                fakes.fs.seed(path, contents)
            } catch (e: FsError) {
                throw SeedException("fs.$path", e.message ?: "refused")
            }
        }
        for (rule in http) {
            var matcher = HttpMatcher.any()
            rule.url?.let { matcher = matcher and HttpMatcher.url(it) }
            rule.urlPrefix?.let { matcher = matcher and HttpMatcher.urlPrefix(it) }
            rule.method?.let { matcher = matcher and HttpMatcher.method(it) }
            fakes.http.respond(matcher, rule.reply)
        }
        connectivity?.let { fakes.connectivity.set(it.first, it.second) }
        lifecycle?.let { fakes.lifecycle.set(it) }
    }

    /** Reading seeds. */
    public companion object {
        /**
         * Reads a seed document.
         *
         * @throws SeedException naming the path of the first malformed value.
         */
        public fun fromJson(text: String): Seed = readSeed(text)
    }
}

private val METHODS = HttpMethod.entries.associateBy { it.name.lowercase() }
private val KINDS = NetKind.entries.associateBy { it.name.lowercase() }
private val STATES = AppState.entries.associateBy { it.name.lowercase() }

private fun bytesOf(path: String, v: Json?): ByteArray = when {
    v is Json.Str -> v.value.toByteArray(Charsets.UTF_8)
    v is Json.Obj && v.fields["hex"] is Json.Str -> (v.fields["hex"] as Json.Str).value.fromHex() ?: throw SeedException(path, "an object value must be {\"hex\": \"..\"}")
    else -> throw SeedException(path, "must be a string or {\"hex\": \"..\"}")
}

private fun entries(doc: Map<String, Json>, key: String): List<Pair<String, ByteArray>> {
    val v = doc[key] ?: return emptyList()
    val map = (v as? Json.Obj)?.fields ?: throw SeedException(key, "must be an object")
    return map.map { (k, value) -> k to bytesOf("$key.$k", value) }
}

private fun <T> named(table: Map<String, T>, path: String, v: Json?): T {
    val name = (v as? Json.Str)?.value ?: throw SeedException(path, "must be a string")
    return table[name] ?: throw SeedException(path, "is not one of ${table.keys.joinToString(", ")}")
}

private fun httpError(path: String, v: Json?): HttpError {
    if (v is Json.Str && v.value == "timeout") return HttpError.Timeout
    if (v is Json.Str && v.value == "cancelled") return HttpError.Cancelled
    if (v is Json.Obj) {
        (v.fields["network"] as? Json.Str)?.let { return HttpError.Network(it.value) }
        (v.fields["invalid_url"] as? Json.Str)?.let { return HttpError.InvalidUrl(it.value) }
    }
    throw SeedException(path, "must be \"timeout\", \"cancelled\", {\"network\": ..} or {\"invalid_url\": ..}")
}

private fun httpRule(i: Int, v: Json): SeedHttpRule {
    fun at(field: String) = "http[$i].$field"
    val obj = (v as? Json.Obj)?.fields ?: throw SeedException("http[$i]", "must be an object")
    fun text(field: String): String? = when (val x = obj[field]) {
        null -> null
        is Json.Str -> x.value
        else -> throw SeedException(at(field), "must be a string")
    }
    val method = obj["method"]?.let { named(METHODS, at("method"), it) }
    val reply: HttpReply = if (obj["error"] != null) {
        HttpReply.Failure(httpError(at("error"), obj["error"]))
    } else {
        val status = when (val s = obj["status"]) {
            null -> 200
            is Json.Num -> s.raw.toIntOrNull()?.takeIf { it in 0..0xffff } ?: throw SeedException(at("status"), "must be a status code")
            else -> throw SeedException(at("status"), "must be a status code")
        }
        val body = obj["body"]?.let { bytesOf(at("body"), it) } ?: ByteArray(0)
        val headers = ArrayList<Header>()
        obj["headers"]?.let { h ->
            val list = (h as? Json.Arr)?.items ?: throw SeedException(at("headers"), "must be a list of [name, value]")
            for (pair in list) {
                val items = (pair as? Json.Arr)?.items
                val name = (items?.getOrNull(0) as? Json.Str)?.value
                val value = (items?.getOrNull(1) as? Json.Str)?.value
                if (items == null || items.size != 2 || name == null || value == null) throw SeedException(at("headers"), "must be a list of [name, value]")
                headers += Header(name, value)
            }
        }
        HttpReply.Response(HttpResponse(status.toUShort(), headers, body))
    }
    return SeedHttpRule(text("url"), text("url_prefix"), method, reply)
}

private fun readSeed(text: String): Seed {
    val doc = try {
        parseJson(text)
    } catch (e: JsonException) {
        throw SeedException("$", "is not valid JSON: ${e.message}")
    }
    val obj = (doc as? Json.Obj)?.fields ?: throw SeedException("$", "must be an object")
    obj["version"]?.let { v ->
        if ((v as? Json.Num)?.raw != SEED_VERSION.toString()) throw SeedException("version", "is not supported (this reader knows $SEED_VERSION)")
    }
    val nowMs = obj["now_ms"]?.let { (it as? Json.Num)?.raw?.toLongOrNull() ?: throw SeedException("now_ms", "must be an integer") }
    val rngSeed: ULong? = when (val r = obj["rng_seed"]) {
        null -> null
        is Json.Num -> r.raw.toULongOrNull() ?: throw SeedException("rng_seed", "must be a non-negative integer")
        is Json.Str -> parseHex64(r.value) ?: throw SeedException("rng_seed", "a string seed must be \"0x..\"")
        else -> throw SeedException("rng_seed", "must be an integer or a \"0x..\" string")
    }
    val http = obj["http"]?.let { h ->
        (h as? Json.Arr)?.items?.mapIndexed { i, r -> httpRule(i, r) } ?: throw SeedException("http", "must be a list")
    } ?: emptyList()
    val connectivity = obj["connectivity"]?.let { c ->
        val fields = (c as? Json.Obj)?.fields
        val online = (fields?.get("online") as? Json.Bool)?.value ?: throw SeedException("connectivity.online", "must be true or false")
        online to named(KINDS, "connectivity.kind", fields["kind"])
    }
    val lifecycle = obj["lifecycle"]?.let { named(STATES, "lifecycle", it) }
    return Seed(nowMs, rngSeed, entries(obj, "kv"), entries(obj, "secure_store"), entries(obj, "fs"), http, connectivity, lifecycle)
}
