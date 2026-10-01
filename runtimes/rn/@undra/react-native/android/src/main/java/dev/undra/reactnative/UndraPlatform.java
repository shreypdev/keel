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
 *   <li>{@link #directories}: where {@code Kv} ({@code filesDir/undra/kv}), {@code Fs} ({@code filesDir/undra/fs}) and
 *       {@code SecureStore} ({@code noBackupFilesDir/undra/secure}) keep their files, the directories of
 *       {@code android-adapters};</li>
 *   <li>{@link #seal} and {@link #open}: a {@code SecureStore} value sealed with AES-256-GCM under the Android Keystore
 *       key {@code dev.undra.securestore} (hardware-backed where the device has it, never exported, not bound to the
 *       lock screen, so the core can read it in the background after the first unlock), in {@link SecureSeal}'s
 *       layout: the alias, the layout and the directory of {@code android-adapters}' {@code AndroidSecureStoreAdapter},
 *       so either shell of an app opens the other's secrets. The C++ side stores the sealed bytes;</li>
 *   <li>{@link #startConnectivity}: the {@link NetworkMonitor}.</li>
 * </ul>
 */
public final class UndraPlatform {
    /** The Keystore alias of the AES key, {@code android-adapters}' default. */
    static final String KEY_ALIAS = "dev.undra.securestore";
    private static final String KEYSTORE = "AndroidKeyStore";
    private static final String KEY_LOCK_FILE = ".keystore.lock";
    private static final String SECURE_PATH = "undra/secure";
    private static final Object KEY_LOCK = new Object();

    private static volatile Context context;
    private static volatile SecretKey cachedKey;

    private UndraPlatform() {}

    /**
     * Gives the module the application context. {@link UndraContextProvider} calls it before {@code Application.onCreate};
     * an app that removes the provider calls it from {@code onCreate}, before React Native starts.
     */
    public static void install(Context context) {
        Context app = context.getApplicationContext();
        UndraPlatform.context = app != null ? app : context;
    }

    /** {@code {filesDir, noBackupFilesDir}} of the app, or {@code null} before {@link #install}. Called over JNI. */
    static String[] directories() {
        Context app = context;
        if (app == null) return null;
        return new String[] {app.getFilesDir().getAbsolutePath(), app.getNoBackupFilesDir().getAbsolutePath()};
    }

    /** Seals a {@code SecureStore} value. Called over JNI on the module's SecureStore thread; throws on failure. */
    static byte[] seal(String key, byte[] plain) throws Exception {
        return SecureSeal.seal(secret(), key, plain);
    }

    /** Opens a sealed {@code SecureStore} value; throws when it fails authentication (never "missing"). */
    static byte[] open(String key, byte[] sealed) throws Exception {
        return SecureSeal.open(secret(), key, sealed);
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
    private static SecretKey secret() throws Exception {
        SecretKey key = cachedKey;
        if (key != null) return key;
        synchronized (KEY_LOCK) {
            if (cachedKey != null) return cachedKey;
            Context app = context;
            if (app == null) throw new IllegalStateException("UndraPlatform.install(context) was not called");
            File directory = new File(app.getNoBackupFilesDir(), SECURE_PATH);
            if (!directory.isDirectory() && !directory.mkdirs() && !directory.isDirectory()) {
                throw new java.io.IOException("cannot create " + directory);
            }
            try (RandomAccessFile file = new RandomAccessFile(new File(directory, KEY_LOCK_FILE), "rw");
                    FileLock ignored = file.getChannel().lock()) {
                KeyStore keyStore = KeyStore.getInstance(KEYSTORE);
                keyStore.load(null);
                if (keyStore.getKey(KEY_ALIAS, null) instanceof SecretKey existing) {
                    cachedKey = existing;
                    return existing;
                }
                KeyGenParameterSpec spec = new KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .build();
                KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE);
                generator.init(spec);
                cachedKey = generator.generateKey();
                return cachedKey;
            }
        }
    }
}
