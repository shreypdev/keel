package dev.undra.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.FileKv
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import java.io.File
import java.io.RandomAccessFile
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * The `SecureStore` port: each value is encrypted with AES-256-GCM under a key that lives in the Android Keystore and
 * never leaves it, and the ciphertext is kept in a file under `<noBackupFilesDir>/undra/secure`.
 *
 *  - **The key** is generated on first use in the `AndroidKeyStore` provider (hardware-backed where the device has a TEE
 *    or StrongBox; not extractable; usable only by this app) under the alias [keyAlias]. Creating it is guarded by a
 *    lock inside the process and a file lock across processes.
 *  - **The values** are sealed as `format, iv, ciphertext + tag`, with the key name as authenticated data, so a file
 *    cannot be copied to another key (the layout of the web adapter). They are written like [AndroidKvAdapter]'s,
 *    atomically, and survive the process being killed.
 *  - **Key names are not secret**: `list` works, so the names are stored in clear next to the sealed value (as in the
 *    Keychain and in IndexedDB). Put the secret in the value.
 *  - **Not backed up**: the directory is `noBackupFilesDir`, excluded from Auto Backup, because a restored ciphertext
 *    could not be opened on another device (Keystore keys do not travel). Uninstalling the app deletes both.
 *  - **Failures** (a Keystore that is unavailable, a key that was lost, a file that fails authentication) throw
 *    [SecureStoreException]; the port has no error channel, so the core sees `PortError::Unavailable`, and a read never
 *    pretends that a value which exists is missing.
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
    directory: File,
    private val keys: SecretKeySource,
) {
    /** An adapter keeping its files in [directory] and its key in the Keystore under [keyAlias]. */
    public constructor(directory: File, keyAlias: String = DEFAULT_KEY_ALIAS) : this(directory, KeystoreKeySource(keyAlias, File(directory, KEY_LOCK_FILE)))

    /** The adapter over `<noBackupFilesDir>/undra/secure` of [context]'s application, with the default key alias. */
    public constructor(context: Context) : this(File(context.applicationContext.noBackupFilesDir, DEFAULT_PATH))

    private val store = FileKv(directory.toPath())

    /** The value stored under [key], or `null` if there is none.
     *
     * @throws SecureStoreException if the value cannot be decrypted or authenticated.
     */
    public suspend fun get(key: String): ByteArray? {
        val stored = store.get(key) ?: return null
        return SecureSeal.open(keys.secret(), key, stored)
    }

    /** Seals [value] and stores it under [key], replacing what was there.
     *
     * @throws SecureStoreException if the value cannot be encrypted.
     */
    public suspend fun set(key: String, value: ByteArray) {
        store.set(key, SecureSeal.seal(keys.secret(), key, value))
    }

    /** Removes [key]; removing a missing key is not an error. */
    public suspend fun delete(key: String): Unit = store.delete(key)

    /** Every stored key that starts with [prefix], sorted. */
    public suspend fun list(prefix: String): List<String> = store.list(prefix)

    /** This adapter as an async [PortImpl] for [StandardPorts.SecureStore]; a failure answers `unavailable` (see the class documentation). */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.SecureStore.GET] = { args ->
                val key = readArgs(args) { it.readStr() }
                OPTION_BYTES.encodeToByteArray(this@AndroidSecureStoreAdapter.get(key))
            }
            this[StandardPorts.SecureStore.SET] = { args ->
                val reader = UndraReader(args)
                val key = reader.readStr()
                val value = reader.readBytes()
                reader.finish()
                this@AndroidSecureStoreAdapter.set(key, value)
                NO_REPLY
            }
            this[StandardPorts.SecureStore.DELETE] = { args ->
                this@AndroidSecureStoreAdapter.delete(readArgs(args) { it.readStr() })
                NO_REPLY
            }
            this[StandardPorts.SecureStore.LIST] = { args ->
                STRING_LIST.encodeToByteArray(this@AndroidSecureStoreAdapter.list(readArgs(args) { it.readStr() }))
            }
        },
    )

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

        private fun load(): SecretKey {
            try {
                lockFile.parentFile?.mkdirs()
                RandomAccessFile(lockFile, "rw").use { file ->
                    file.channel.lock().use {
                        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
                        (keyStore.getKey(alias, null) as? SecretKey)?.let { return it }
                        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                            .setKeySize(KEY_BITS)
                            .build()
                        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE).apply { init(spec) }.generateKey()
                    }
                }
            } catch (e: GeneralSecurityException) {
                throw SecureStoreException("the Android Keystore key '$alias' is unavailable: ${e.message}", e)
            } catch (e: java.io.IOException) {
                throw SecureStoreException("the Android Keystore key '$alias' is unavailable: ${e.message}", e)
            } catch (e: RuntimeException) {
                throw SecureStoreException("the Android Keystore key '$alias' is unavailable: ${e.message}", e)
            }
        }
    }

    /** Defaults. */
    public companion object {
        /** The Keystore alias of the AES key unless another is given. */
        public const val DEFAULT_KEY_ALIAS: String = "dev.undra.securestore"

        private const val DEFAULT_PATH = "undra/secure"
        private const val KEY_LOCK_FILE = ".keystore.lock"
        private const val KEYSTORE = "AndroidKeyStore"
        private const val KEY_BITS = 256
        private val PROCESS_LOCK = Any()
        private val OPTION_BYTES: UndraCodec<ByteArray?> = Codecs.option(Codecs.bytes)
        private val STRING_LIST: UndraCodec<List<String>> = Codecs.vec(Codecs.string)
    }
}
