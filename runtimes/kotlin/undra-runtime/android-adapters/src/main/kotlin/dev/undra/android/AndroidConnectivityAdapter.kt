package dev.undra.android

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import dev.undra.runtime.UndraCore
import dev.undra.runtime.adapters.ConnectivityEvents
import dev.undra.runtime.adapters.NetKind

/**
 * What the device's default network looks like to the core: whether it can carry traffic and what kind it is.
 *
 * @property online `true` when there is a default network that provides internet access.
 * @property kind the most specific link in use: Wi-Fi, then cellular, then wired; [NetKind.UNKNOWN] for another link
 *   (Bluetooth, VPN without a known carrier, ...); [NetKind.NONE] when [online] is `false`.
 */
public data class NetworkStatus(val online: Boolean, val kind: NetKind) {
    /** The statuses worth naming. */
    public companion object {
        /** No network. */
        public val OFFLINE: NetworkStatus = NetworkStatus(online = false, kind = NetKind.NONE)
    }
}

/**
 * The classification of a network, kept apart from [AndroidConnectivityAdapter] so that JVM unit tests can check it.
 */
internal object NetworkClassifier {
    /**
     * The status of a network that has the given properties. Online means the network says it provides internet
     * access (`NET_CAPABILITY_INTERNET`), like `NWPath.satisfied` on Apple platforms: a route exists, not that a
     * server answers. With [requireValidated] it must also have passed Android's own connectivity check
     * (`NET_CAPABILITY_VALIDATED`), which treats a captive portal, and a network where that check cannot reach its
     * server, as offline.
     */
    fun classify(
        hasInternet: Boolean,
        validated: Boolean,
        wifi: Boolean,
        cellular: Boolean,
        ethernet: Boolean,
        requireValidated: Boolean,
    ): NetworkStatus {
        if (!hasInternet || (requireValidated && !validated)) return NetworkStatus.OFFLINE
        val kind = when {
            wifi -> NetKind.WIFI
            cellular -> NetKind.CELLULAR
            ethernet -> NetKind.WIRED
            else -> NetKind.UNKNOWN
        }
        return NetworkStatus(online = true, kind = kind)
    }

    /** [classify] for the capabilities Android reports; `null` (no network) is offline. */
    fun classify(capabilities: NetworkCapabilities?, requireValidated: Boolean): NetworkStatus =
        if (capabilities == null) {
            NetworkStatus.OFFLINE
        } else {
            classify(
                hasInternet = capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET),
                validated = capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
                wifi = capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
                cellular = capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR),
                ethernet = capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET),
                requireValidated = requireValidated,
            )
        }
}

/**
 * The `Connectivity` event port over `ConnectivityManager`: tells the core whether the device has a network and which
 * kind, from the moment it is started and on every change.
 *
 * [attach] reports the current state at once (the core assumes the network is up until told otherwise, and the offline
 * queue of `undra-query` replays when the state turns from offline to online), then registers a default-network
 * callback and reports each change: a network that appears, disappears or changes capabilities, such as Wi-Fi giving
 * way to cellular. Identical consecutive states are reported once. Reports are made on a handler thread of their own,
 * never on the main thread. Needs `ACCESS_NETWORK_STATE`, which this library's manifest declares; without it the
 * adapter logs a warning and reports nothing.
 *
 * ```kotlin
 * val connectivity = AndroidConnectivityAdapter(context)
 * connectivity.attach(UndraCore.shared)
 * ```
 *
 * [AndroidPlatformDefaults.install] does this for you. The state is `NWPath`-like: online when a default network
 * provides internet access, which includes a network behind a captive portal. Pass `requireValidated = true` to
 * require Android's own reachability check as well.
 *
 * @param context any context of the app.
 * @param requireValidated whether a network must also be validated by Android to count as online.
 */
public class AndroidConnectivityAdapter(context: Context, private val requireValidated: Boolean = false) : AutoCloseable {
    private val manager: ConnectivityManager? = context.applicationContext.getSystemService(ConnectivityManager::class.java)
    private var thread: HandlerThread? = null
    private var callback: ConnectivityManager.NetworkCallback? = null

    // Only touched on the handler thread once started.
    private var tracked: Network? = null
    private var last: NetworkStatus? = null

    /** The current state of the default network, read now. */
    public fun current(): NetworkStatus {
        val cm = manager ?: return NetworkStatus.OFFLINE
        return try {
            val network = cm.activeNetwork
            NetworkClassifier.classify(network?.let(cm::getNetworkCapabilities), requireValidated)
        } catch (e: SecurityException) {
            NetworkStatus.OFFLINE
        }
    }

    /**
     * Starts reporting to [listener]: the current state first, then every change. The listener is called on a handler
     * thread named `undra-connectivity`. Calling it again replaces the previous listener.
     */
    public fun start(listener: (NetworkStatus) -> Unit) {
        close()
        val cm = manager
        if (cm == null) {
            Log.w(TAG, "no ConnectivityManager on this device; Connectivity events are not reported")
            return
        }
        val handlerThread = HandlerThread("undra-connectivity").also { it.start() }
        val handler = Handler(handlerThread.looper)
        val deliver = { status: NetworkStatus ->
            if (status != last) {
                last = status
                try {
                    listener(status)
                } catch (e: Exception) {
                    Log.w(TAG, "a Connectivity listener failed", e)
                }
            }
        }
        val networkCallback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                tracked = network // its capabilities follow in onCapabilitiesChanged
            }

            override fun onCapabilitiesChanged(network: Network, networkCapabilities: NetworkCapabilities) {
                tracked = network
                deliver(NetworkClassifier.classify(networkCapabilities, requireValidated))
            }

            override fun onLost(network: Network) {
                // A switch from one default network to another reports onAvailable for the new one and may or may not
                // report onLost for the old: only the loss of the tracked network means there is none.
                if (network == tracked) {
                    tracked = null
                    deliver(NetworkStatus.OFFLINE)
                }
            }
        }
        try {
            cm.registerDefaultNetworkCallback(networkCallback, handler)
        } catch (e: SecurityException) {
            Log.w(TAG, "ACCESS_NETWORK_STATE is not granted; Connectivity events are not reported", e)
            handlerThread.quitSafely()
            return
        }
        thread = handlerThread
        callback = networkCallback
        // After registering, on the same thread: whatever the callback has already reported is older than this read.
        handler.post { deliver(current()) }
    }

    /** Starts reporting to [core] as `Connectivity.changed(online, kind)` events; see [start]. */
    public fun attach(core: UndraCore) {
        val events = ConnectivityEvents(core)
        start { status ->
            try {
                events.changed(status.online, status.kind)
            } catch (e: Exception) {
                Log.w(TAG, "could not report the connectivity state to the core", e)
            }
        }
    }

    /** Stops reporting. Idempotent. */
    override fun close() {
        val registered = callback
        val handlerThread = thread
        callback = null
        thread = null
        if (registered != null) {
            try {
                manager?.unregisterNetworkCallback(registered)
            } catch (e: IllegalArgumentException) {
                // not registered any more
            }
        }
        handlerThread?.quitSafely()
        tracked = null
        last = null
    }

    private companion object {
        const val TAG = "Undra"
    }
}
