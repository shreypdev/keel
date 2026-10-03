// The Kotlin lines of the cookbook recipe "Your network stack" (site/docs/cookbook/network-stack.html quotes the region between
// `docs:begin` and `docs:end`: site/scripts/build-cookbook.mjs). They are compiled here with the module, and NetworkStackRecipeTest
// runs the client they build against a local server, so what the page shows is what ran. Everything outside the markers is
// scaffolding that makes the lines compile.
package dev.undra.okhttp

import android.content.Context
import dev.undra.android.AndroidPlatformDefaults
import dev.undra.runtime.UndraCore
import okhttp3.CertificatePinner
import okhttp3.OkHttpClient

/** Where the app keeps its session tokens. */
class TokenStore(var access: String, private val refreshed: () -> String?) {
    /** Asks the server for a new access token and keeps it; `null` when the session is over. */
    fun refresh(): String? = refreshed()?.also { access = it }
}

// docs:begin network-kotlin
/** The app's one client: a token on every request, a refresh when the server says 401, and pinning. */
fun appClient(tokens: TokenStore): OkHttpClient =
    OkHttpClient.Builder()
        .addInterceptor { chain ->
            val signed = chain.request().newBuilder().header("Authorization", "Bearer ${tokens.access}").build()
            chain.proceed(signed)
        }
        .authenticator { _, response ->
            // OkHttp asks on a 401. A request that is already a retry gives up: the refresh did not help.
            if (response.priorResponse != null) return@authenticator null
            val fresh = tokens.refresh() ?: return@authenticator null
            response.request.newBuilder().header("Authorization", "Bearer $fresh").build()
        }
        .certificatePinner(CertificatePinner.Builder().add("api.example.com", "sha256/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").build())
        .build()

/** In `Application.onCreate`, in place of `AndroidPlatformDefaults.install(core, this)`: Http, WebSocket and Sse go through [client]. */
fun installNetwork(core: UndraCore, context: Context, client: OkHttpClient) {
    AndroidPlatformDefaults.installWithOkHttp(core, context, client)
}
// docs:end
