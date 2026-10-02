package dev.undra.reactnative;

import android.content.Context;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.io.File;
import java.io.RandomAccessFile;
import java.nio.channels.FileLock;
import java.security.KeyStore;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;

/**
 * The Android half of {@code @undra/react-native}'s default ports (ADR-038 amendment B, B1 and B8): what the C++ module
 * cannot reach without Java. The C++ module ({@code cpp/UndraPlatformAndroid.cpp}) calls the static methods below over
 * JNI; an app calls nothing here, except {@link #install} when it removes {@link UndraContextProvider}.
 *
 * <ul>
 *   <li>{@link #directories}: the app's {@code filesDir} and {@code noBackupFilesDir}, under which {@code Kv}
 *       ({@code filesDir/undra/<namespace>/kv}), {@code Fs} ({@code filesDir/undra/<namespace>/fs}) and
 *       {@code SecureStore} ({@code noBackupFilesDir/undra/<namespace>/secure}) keep their files, the directories of
 *       {@code android-adapters} for the core's namespace (ADR-044 amendment A: every default store is per core
 *       namespace, so two cores of one app never share one);</li>
 *   <li>{@link #seal} and {@link #open}: a {@code SecureStore} value sealed with AES-256-GCM under the Android Keystore
 *       key {@code <namespace>.dev.undra.securestore} (hardware-backed where the device has it, never exported, not
 *       bound to the lock screen, so the core can read it in the background after the first unlock), in
 *       {@link SecureSeal}'s layout: the alias, the layout and the directory of {@code android-adapters}'
 *       {@code AndroidSecureStoreAdapter}, so either shell of an app opens the other's secrets. The C++ side stores the
 *       sealed bytes;</li>
 *   <li>{@link #startConnectivity}: the {@link NetworkMonitor};</li>
 *   <li>the {@code Db} port's SQLite is {@link UndraDatabase} (ADR-048), reached from C++ the same way.</li>
 * </ul>
 */
public final class UndraPlatform {
    private static final String KEYSTORE = "AndroidKeyStore";
    private static final String KEY_LOCK_FILE = ".keystore.lock";
    private static final Object KEY_LOCK = new Object();

    private static volatile Context context;
    /** The Keystore keys made or found, by alias: one per core namespace. */
    private static final java.util.concurrent.ConcurrentHashMap<String, SecretKey> KEYS = new java.util.concurrent.ConcurrentHashMap<>();

    private UndraPlatform() {}

    /**
     * Gives the module the application context. {@link UndraContextProvider} calls it before {@code Application.onCreate};
     * an app that removes the provider calls it from {@code onCreate}, before React Native starts.
     */
    public static void install(Context context) {
        Context app = context.getApplicationContext();
        UndraPlatform.context = app != null ? app : context;
    }

    /** The application context, or {@code null} before {@link #install}. */
    static Context context() {
        return context;
    }

    /** {@code {filesDir, noBackupFilesDir}} of the app, or {@code null} before {@link #install}. Called over JNI. */
    static String[] directories() {
        Context app = context;
        if (app == null) return null;
        return new String[] {app.getFilesDir().getAbsolutePath(), app.getNoBackupFilesDir().getAbsolutePath()};
    }

    /** The directory of the core {@code namespace}'s sealed secrets: {@code <noBackupFilesDir>/undra/<namespace>/secure}. */
    static File secureDirectory(Context app, String namespace) {
        return new File(app.getNoBackupFilesDir(), StoreNames.securePath(namespace));
    }

    /**
     * Seals a {@code SecureStore} value of the core {@code namespace}. Called over JNI on the module's SecureStore
     * thread; throws on failure.
     */
    static byte[] seal(String namespace, String key, byte[] plain) throws Exception {
        return SecureSeal.seal(secret(namespace), key, plain);
    }

    /** Opens a sealed {@code SecureStore} value of the core {@code namespace}; throws when it fails authentication (never "missing"). */
    static byte[] open(String namespace, String key, byte[] sealed) throws Exception {
        return SecureSeal.open(secret(namespace), key, sealed);
    }

    /** Starts the {@code Connectivity} source reporting to {@code handle}; {@code null} when it cannot start. */
    static NetworkMonitor startConnectivity(long handle) {
        Context app = context;
        if (app == null) return null;
        NetworkMonitor monitor = new NetworkMonitor(app, handle);
        return monitor.start() ? monitor : null;
    }

    /** A report of the {@code Connectivity} source, into the C++ module (bound with {@code RegisterNatives}). */
    static native void nativeConnectivityChanged(long handle, boolean online, int kind);

    /**
     * The Keystore key, made on first use. Guarded by a lock in the process and a file lock across processes (the
     * same lock file as {@code android-adapters}), so two processes of one app never make two keys.
     */
    private static SecretKey secret(String namespace) throws Exception {
        final String alias = StoreNames.keyAlias(namespace);
        SecretKey key = KEYS.get(alias);
        if (key != null) return key;
        synchronized (KEY_LOCK) {
            SecretKey cached = KEYS.get(alias);
            if (cached != null) return cached;
            Context app = context;
            if (app == null) throw new IllegalStateException("UndraPlatform.install(context) was not called");
            File directory = secureDirectory(app, namespace);
            if (!directory.isDirectory() && !directory.mkdirs() && !directory.isDirectory()) {
                throw new java.io.IOException("cannot create " + directory);
            }
            try (RandomAccessFile file = new RandomAccessFile(new File(directory, KEY_LOCK_FILE), "rw");
                    FileLock ignored = file.getChannel().lock()) {
                KeyStore keyStore = KeyStore.getInstance(KEYSTORE);
                keyStore.load(null);
                if (keyStore.getKey(alias, null) instanceof SecretKey existing) {
                    KEYS.put(alias, existing);
                    return existing;
                }
                KeyGenParameterSpec spec = new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .build();
                KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE);
                generator.init(spec);
                SecretKey made = generator.generateKey();
                KEYS.put(alias, made);
                return made;
            }
        }
    }
}
