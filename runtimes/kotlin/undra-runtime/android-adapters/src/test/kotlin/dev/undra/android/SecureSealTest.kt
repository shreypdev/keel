package dev.undra.android

import dev.undra.runtime.adapters.StorageError
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The sealed layout of secrets (the same as the web adapter's), with a software AES key standing in for the Keystore's.
 * Whatever does not open is [StorageError.Corrupt] (ADR-049): altered, moved to another key name, another key, another format.
 */
class SecureSealTest {
    private fun newKey(): SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()

    private val secret = "hunter2 correct horse".toByteArray()

    @Test
    fun a_sealed_value_opens_to_what_was_sealed() {
        val key = newKey()
        for (plain in listOf(ByteArray(0), byteArrayOf(0), secret, ByteArray(100_000) { it.toByte() })) {
            assertArrayEquals(plain, SecureSeal.open(key, "token", SecureSeal.seal(key, "token", plain)))
        }
    }

    @Test
    fun the_layout_is_format_then_iv_then_ciphertext_and_tag() {
        val sealed = SecureSeal.seal(newKey(), "k", secret)
        assertEquals(1, sealed[0].toInt())
        assertEquals(1 + 12 + secret.size + 16, sealed.size)
    }

    @Test
    fun the_plaintext_is_not_in_the_sealed_bytes() {
        val sealed = SecureSeal.seal(newKey(), "k", secret)
        assertFalse(String(sealed, Charsets.ISO_8859_1).contains(String(secret, Charsets.ISO_8859_1)))
    }

    @Test
    fun every_seal_uses_a_fresh_iv() {
        val key = newKey()
        val a = SecureSeal.seal(key, "k", secret)
        val b = SecureSeal.seal(key, "k", secret)
        assertFalse(a.copyOfRange(1, 13).contentEquals(b.copyOfRange(1, 13)))
        assertFalse(a.contentEquals(b))
    }

    @Test
    fun a_value_cannot_be_moved_to_another_key_name() {
        val key = newKey()
        val sealed = SecureSeal.seal(key, "a", secret)
        val e = assertThrows(StorageError.Corrupt::class.java) { SecureSeal.open(key, "b", sealed) }
        assertTrue(e.message, e.message!!.contains("failed authentication"))
    }

    @Test
    fun a_changed_byte_is_detected_wherever_it_is() {
        val key = newKey()
        val sealed = SecureSeal.seal(key, "k", secret)
        for (at in listOf(1, 5, 13, sealed.size - 1)) {
            val tampered = sealed.copyOf().also { it[at] = (it[at].toInt() xor 1).toByte() }
            assertThrows("byte $at", StorageError.Corrupt::class.java) { SecureSeal.open(key, "k", tampered) }
        }
    }

    @Test
    fun another_key_cannot_open_it() {
        val sealed = SecureSeal.seal(newKey(), "k", secret)
        assertThrows(StorageError.Corrupt::class.java) { SecureSeal.open(newKey(), "k", sealed) }
    }

    @Test
    fun bytes_in_another_format_are_refused_with_a_clear_message() {
        val key = newKey()
        val sealed = SecureSeal.seal(key, "k", secret)
        val wrongFormat = sealed.copyOf().also { it[0] = 2 }
        assertTrue(assertThrows(StorageError.Corrupt::class.java) { SecureSeal.open(key, "k", wrongFormat) }.message!!.contains("not in the secure-store format"))
        assertTrue(assertThrows(StorageError.Corrupt::class.java) { SecureSeal.open(key, "k", ByteArray(20)) }.message!!.contains("not in the secure-store format"))
        assertThrows(StorageError.Corrupt::class.java) { SecureSeal.open(key, "k", ByteArray(0)) }
    }

    @Test
    fun the_authenticated_data_names_the_key() {
        assertArrayEquals("undra.secure:session/token".toByteArray(), SecureSeal.aad("session/token"))
    }

    /**
     * A value sealed by the web adapter (`webCryptoSecureStore` in `runtimes/ts/@undra/runtime/src/adapters/secure.ts`,
     * run under Node's WebCrypto with a fixed key and IV) opens here: the format byte, the IV, the tag length and the
     * authenticated data are byte-for-byte the same. The key never travels between platforms; the layout does.
     */
    @Test
    fun a_value_sealed_by_the_web_adapter_opens_with_the_same_key_name_and_not_another() {
        val key = SecretKeySpec(hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"), "AES")
        val sealedByWebCrypto = hex("01a0a1a2a3a4a5a6a7a8a9aaab8e6d125920b9301d0fd4ef9116c715270548bd33a919ee")
        assertEquals(1 + 12 + "hunter2".length + 16, sealedByWebCrypto.size)
        assertArrayEquals("hunter2".toByteArray(), SecureSeal.open(key, "session.token", sealedByWebCrypto))
        assertThrows(StorageError.Corrupt::class.java) { SecureSeal.open(key, "session.other", sealedByWebCrypto) }
    }

    private fun hex(text: String): ByteArray = ByteArray(text.length / 2) { text.substring(2 * it, 2 * it + 2).toInt(16).toByte() }
}
