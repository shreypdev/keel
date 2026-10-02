package dev.undra.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import dev.undra.runtime.CoreNamespace
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.FileKv
import dev.undra.runtime.adapters.KeyValueBackend
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.adapters.StoragePort
import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Path
import java.security.KeyStore
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The `SecureStore` port: each value is encrypted with AES-256-GCM under a key that lives in the Android Keystore and
 * never leaves it, and the ciphertext is kept in a file under `<noBackupFilesDir>/undra/<namespace>/secure`. The default
 * alias and directory carry the core's namespace (`<namespace>.dev.undra.securestore`), so two cores of one app never read
 * each other's secrets (ADR-044 amendment A).
 *
 *  - **The key** is generated on first use in the `AndroidKeyStore` provider (hardware-backed where the device has a TEE
 *    or StrongBox; not extractable; usable only by this app) under the alias [keyAlias]: AES-256, GCM, no padding, for
 *    encryption and decryption, with the Keystore drawing a fresh 12-byte IV for every encryption (the provider's
 *    default, `setRandomizedEncryptionRequired`, is kept, so a caller can never supply or repeat one). It is **not**
 *    bound to the user's authentication or to the device being unlocked, on purpose: the core replays the offline queue
 *    and refreshes queries in the background, when no one is there to confirm, as the Keychain item of the Swift adapter
 *    (`AfterFirstUnlockThisDeviceOnly`) does. Like that item it is usable only once the device has been unlocked after
 *    boot, because its files live in credential-encrypted storage. Creating it is guarded by a lock inside the process
 *    and a file lock across processes.
 *  - **The values** are sealed as `format, iv, ciphertext + tag`, with the key name as authenticated data, so a file
 *    cannot be copied to another key (the layout of the web adapter). They are written like [AndroidKvAdapter]'s,
 *    atomically, and survive the process being killed.
 *  - **Key names are not secret**: `list` works, so the names are stored in clear next to the sealed value (as in the
 *    Keychain and in IndexedDB). Put the secret in the value.
 *  - **Not backed up**: the directory is `noBackupFilesDir`, excluded from Auto Backup, because a restored ciphertext
 *    could not be opened on another device (Keystore keys do not travel). Uninstalling the app deletes both.
 *  - **Failures** are [StorageError]s, which the port answers the core with (ADR-049), and a read never pretends that a
 *    value which exists is missing:
 *
 *    | Failure | StorageError |
 *    |---|---|
 *    | no Android Keystore (or no `AndroidKeyStore` provider) | [StorageError.Unavailable] |
 *    | a key that needs the user to authenticate first (`UserNotAuthenticatedException`) | [StorageError.Locked] |
 *    | a key invalidated for good (`KeyPermanentlyInvalidatedException`), a value that fails authentication, a stored file in another format | [StorageError.Corrupt] |
 *    | a full disk (`ENOSPC`) | [StorageError.Full] |
 *    | anything else the Keystore, the cipher or the file system reports | [StorageError.Io] |
 *
 *    If the Keystore no longer has the key (it is not bound to the lock screen, so an OS upgrade or a changed lock
 *    screen does not invalidate it; a wiped Keystore does), a new one is made and every value sealed under the old one
 *    is [StorageError.Corrupt] until the app replaces or deletes it, which is the moment to ask the user to sign in
 *    again.
 *
 * Use it for tokens and keys, not for bulk data: every value goes through the Keystore service. It is the library's own
 * `javax.crypto` and `AndroidKeyStore` code rather than `androidx.security:security-crypto`, which is deprecated and
 * brings in a large cryptography library for what is about eighty lines here.
 *
 * All operations run on `Dispatchers.IO`.
 *
 * @param directory where the sealed entries live; created on the first write.
 * @param keyAlias the Keystore alias of the AES key.
 */
public class AndroidSecureStoreAdapter internal constructor(
    directory: Path,
    private val keys: SecretKeySource,
) : KeyValueBackend {
    /** An adapter over [directory] whose AES key comes from [keys] (the tests' software key). */
    internal constructor(directory: File, keys: SecretKeySource) : this(directory.toPath(), keys)

    /** An adapter keeping its files in [directory] and its key in the Keystore under [keyAlias]. */
    public constructor(directory: File, keyAlias: String = DEFAULT_KEY_ALIAS) :
        this(directory.toPath(), KeystoreKeySource(keyAlias, File(directory, KEY_LOCK_FILE)))

    /**
     * The adapter over `<noBackupFilesDir>/undra/<namespace>/secure` of [context]'s application, with the key alias
     * `<namespace>.dev.undra.securestore`: [namespace] is the core's (`UndraCore.namespace`).
     */
    public constructor(context: Context, namespace: String) :
        this(directoryOf(context, namespace), keyAliasOf(namespace))

    private val store = FileKv(directory)

    /**
     * The value stored under [key], or `null` if there is none.
     *
     * @throws StorageError.Corrupt if the value cannot be authenticated or is not in the sealed format.
     * @throws StorageError.Locked if the key needs the user to authenticate first.
     * @throws StorageError.Unavailable if there is no Android Keystore.
     * @throws StorageError if it cannot be read otherwise.
     */
    override suspend fun get(key: String): ByteArray? {
        val stored = store.get(key) ?: return null
        // The Keystore is a call into another process: never on the thread of the caller.
        return withContext(Dispatchers.IO) { SecureSeal.open(secret(), key, stored) }
    }

    /**
     * Seals [value] and stores it under [key], replacing what was there.
     *
     * @throws StorageError.Full if the disk is full (the old value stays).
     * @throws StorageError if the value cannot be encrypted or written (see the class documentation).
     */
    override suspend fun set(key: String, value: ByteArray) {
        val sealed = withContext(Dispatchers.IO) { SecureSeal.seal(secret(), key, value) }
        store.set(key, sealed)
    }

    /**
     * Removes [key]; removing a missing key is not an error.
     *
     * @throws StorageError if it cannot be removed.
     */
    override suspend fun delete(key: String): Unit = store.delete(key)

    /**
     * Every stored key that starts with [prefix], sorted.
     *
     * @throws StorageError if the directory cannot be read.
     */
    override suspend fun list(prefix: String): List<String> = store.list(prefix)

    /** This adapter as an async [PortImpl] for [StandardPorts.SecureStore]; a failure answers the core with its [StorageError]. */
    public fun portImpl(): PortImpl = StoragePort.SECURE_STORE.portImpl(this)

    /** The AES key, with what obtaining it can throw as a [StorageError]. */
    private fun secret(): SecretKey =
        try {
            keys.secret()
        } catch (e: Exception) {
            throw SecureSeal.failure(e, "obtaining the Keystore key")
        }

    /** Where the AES key comes from. */
    internal fun interface SecretKeySource {
        /** The key every value is sealed under; the same key on every call. */
        fun secret(): SecretKey
    }

    /** The key of the Android Keystore under [alias], created on first use. */
    private class KeystoreKeySource(private val alias: String, private val lockFile: File) : SecretKeySource {
        @Volatile
        private var cached: SecretKey? = null

        override fun secret(): SecretKey = cached ?: synchronized(PROCESS_LOCK) { cached ?: load().also { cached = it } }

        /**
         * The key under [alias], made if there is none.
         *
         * @throws StorageError.Unavailable if this device has no Android Keystore (or it cannot be opened).
         * @throws StorageError otherwise, as [SecureSeal.failure] maps it.
         */
        private fun load(): SecretKey {
            try {
                lockFile.parentFile?.mkdirs()
                RandomAccessFile(lockFile, "rw").use { file ->
                    file.channel.lock().use {
                        val keyStore = openKeystore()
                        (keyStore.getKey(alias, null) as? SecretKey)?.let { return it }
                        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                            .setKeySize(KEY_BITS)
                            .build()
                        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE).apply { init(spec) }.generateKey()
                    }
                }
            } catch (e: Exception) {
                throw SecureSeal.failure(e, "obtaining the Android Keystore key '$alias'")
            }
        }

        /** The `AndroidKeyStore`, loaded; a device (or a desktop JVM) without one is [StorageError.Unavailable]. */
        private fun openKeystore(): KeyStore =
            try {
                KeyStore.getInstance(KEYSTORE).apply { load(null) }
            } catch (e: Exception) {
                throw StorageError.Unavailable("the Android Keystore is not available: ${e.javaClass.simpleName}: ${e.message}")
            }
    }

    /** Defaults. */
    public companion object {
        /** What the default Keystore alias of a core is made of, after the core's namespace: `<namespace>.dev.undra.securestore`. */
        public const val DEFAULT_KEY_ALIAS: String = "dev.undra.securestore"

        /** The default Keystore alias of the core [namespace]: `<namespace>.dev.undra.securestore`. */
        public fun keyAliasOf(namespace: String): String = "${CoreNamespace.requireForStores(namespace)}.$DEFAULT_KEY_ALIAS"

        /** `<noBackupFilesDir>/undra/<namespace>/secure`: the directory of [context]'s application for the core [namespace]. */
        public fun directoryOf(context: Context, namespace: String): File =
            File(context.applicationContext.noBackupFilesDir, "undra/${CoreNamespace.requireForStores(namespace)}/secure")

        private const val KEY_LOCK_FILE = ".keystore.lock"
        private const val KEYSTORE = "AndroidKeyStore"
        private const val KEY_BITS = 256
        private val PROCESS_LOCK = Any()
    }
}
