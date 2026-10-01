package dev.undra.reactnative;

/**
 * What a network looks like to the core's {@code Connectivity} port, kept apart from {@link NetworkMonitor} so the JVM
 * test can check it. The rule is {@code android-adapters}' {@code NetworkClassifier}: online when the network says it
 * provides internet access ({@code NET_CAPABILITY_INTERNET}, like {@code NWPath.satisfied} on Apple platforms); the
 * kind is Wi-Fi, then cellular, then wired, else unknown.
 */
final class NetworkClassifier {
    /** {@code NetKind} wire indices (docs/SPEC.md section 8). */
    static final int WIFI = 0;
    static final int CELLULAR = 1;
    static final int WIRED = 2;
    static final int UNKNOWN = 3;
    static final int NONE = 4;

    private NetworkClassifier() {}

    /** The {@code NetKind} of a network with these properties; {@link #NONE} means offline. */
    static int classify(boolean hasInternet, boolean wifi, boolean cellular, boolean ethernet) {
        if (!hasInternet) return NONE;
        if (wifi) return WIFI;
        if (cellular) return CELLULAR;
        if (ethernet) return WIRED;
        return UNKNOWN;
    }
}
