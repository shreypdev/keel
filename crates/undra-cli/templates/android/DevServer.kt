package @@APP_ID@@

import android.content.Intent

/**
 * Where the core comes from: in this process (the default), or the `undra dev` server on the developer's machine
 * (debug builds only; release builds ignore every setting here).
 *
 * The URL comes from the launch extra `undra_dev_url`, which switches a build at run time:
 *
 * ```sh
 * adb shell am start -n @@APP_ID@@/.MainActivity --es undra_dev_url ws://10.0.2.2:7443
 * ```
 *
 * or, when the intent has none, from the build (`./gradlew -PundraDevUrl=ws://10.0.2.2:7443 :app:installDebug`).
 * `undra dev` prints both addresses: `ws://10.0.2.2:<port>` for an emulator, and `ws://127.0.0.1:<port>` for a USB
 * device once `adb reverse tcp:<port> tcp:<port>` is in place (`undra dev --android` does it).
 */
object DevServer {
    /** The launch extra that names the dev server. */
    const val EXTRA = "undra_dev_url"

    /** The dev server this process should use, or `null` for the in-process core. Always `null` in a release build. */
    fun requested(intent: Intent?): String? {
        if (!BuildConfig.DEBUG) return null
        return intent?.getStringExtra(EXTRA)?.takeIf { it.isNotBlank() }
            ?: BuildConfig.UNDRA_DEV_URL.takeIf { it.isNotBlank() }
    }
}
