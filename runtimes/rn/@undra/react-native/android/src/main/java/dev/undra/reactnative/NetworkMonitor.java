package dev.undra.reactnative;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;

/**
 * The {@code Connectivity} source on Android: {@code ConnectivityManager.registerDefaultNetworkCallback} on a handler
 * thread of its own, reporting to the C++ module ({@link UndraPlatform#nativeConnectivityChanged}) the current state
 * first, then every change: a network that appears, is lost, or changes capabilities (Wi-Fi giving way to cellular).
 * The tracking of the default network is {@code android-adapters}' {@code AndroidConnectivityAdapter}'s; the C++ side
 * sends identical consecutive reports once.
 *
 * <p>Created and stopped by C++ over JNI ({@link UndraPlatform#startConnectivity}, {@link #stop}); {@link #stop} returns
 * only when no report is running or will run, because C++ frees what {@code handle} points at right after.
 */
final class NetworkMonitor {
    private static final String TAG = "Undra";

    private final ConnectivityManager manager;
    private final long handle;
    private final HandlerThread thread = new HandlerThread("undra-connectivity");
    private ConnectivityManager.NetworkCallback callback;
    private volatile boolean stopped;
    // Only touched on the handler thread.
    private Network tracked;

    NetworkMonitor(Context context, long handle) {
        this.manager = context.getSystemService(ConnectivityManager.class);
        this.handle = handle;
    }

    /** Starts reporting; {@code false} when the platform refused (no ConnectivityManager, no permission). */
    boolean start() {
        if (manager == null) {
            Log.w(TAG, "no ConnectivityManager on this device; Connectivity events are not reported");
            return false;
        }
        thread.start();
        Handler handler = new Handler(thread.getLooper());
        callback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                tracked = network; // its capabilities follow in onCapabilitiesChanged
            }

            @Override
            public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
                tracked = network;
                report(classify(capabilities));
            }

            @Override
            public void onLost(Network network) {
                // A switch from one default network to another reports onAvailable for the new one, and maybe onLost
                // for the old: only the loss of the tracked network means there is none.
                if (network.equals(tracked)) {
                    tracked = null;
                    report(NetworkClassifier.NONE);
                }
            }
        };
        try {
            manager.registerDefaultNetworkCallback(callback, handler);
        } catch (SecurityException e) {
            Log.w(TAG, "ACCESS_NETWORK_STATE is not granted; Connectivity events are not reported", e);
            callback = null;
            thread.quitSafely();
            return false;
        }
        // After registering, on the same thread: whatever the callback reported already is older than this read.
        handler.post(() -> report(current()));
        return true;
    }

    /** Stops reporting: unregisters, ends the handler thread and waits for it, so no report runs after this returns. */
    void stop() {
        stopped = true;
        ConnectivityManager.NetworkCallback registered = callback;
        callback = null;
        if (registered != null) {
            try {
                manager.unregisterNetworkCallback(registered);
            } catch (IllegalArgumentException e) {
                // not registered any more
            }
        }
        if (thread.isAlive()) {
            thread.quitSafely();
            boolean interrupted = false;
            while (thread.isAlive()) {
                try {
                    thread.join();
                } catch (InterruptedException e) {
                    interrupted = true;
                }
            }
            if (interrupted) Thread.currentThread().interrupt();
        }
    }

    private int current() {
        try {
            Network network = manager.getActiveNetwork();
            return classify(network == null ? null : manager.getNetworkCapabilities(network));
        } catch (SecurityException e) {
            return NetworkClassifier.NONE;
        }
    }

    private static int classify(NetworkCapabilities capabilities) {
        if (capabilities == null) return NetworkClassifier.NONE;
        return NetworkClassifier.classify(
                capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET),
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR),
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET));
    }

    private void report(int kind) {
        if (stopped) return;
        try {
            UndraPlatform.nativeConnectivityChanged(handle, kind != NetworkClassifier.NONE, kind);
        } catch (Throwable t) {
            Log.w(TAG, "could not report the connectivity state to the core", t);
        }
    }
}
