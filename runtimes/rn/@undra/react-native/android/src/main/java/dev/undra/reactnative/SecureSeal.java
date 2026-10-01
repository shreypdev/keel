package dev.undra.reactnative;

import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import javax.crypto.Cipher;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/**
 * The sealed form of a secret: {@code format u8, iv (12 bytes), ciphertext and 16-byte tag}, AES-256-GCM with the key
 * name as authenticated data ({@code undra.secure:<key>}), so a ciphertext cannot be moved to another key.
 *
 * <p>It is the layout of {@code android-adapters}' {@code SecureSeal} and of the web's {@code webCryptoSecureStore},
 * so a secret the Compose shell of an app sealed opens here and the reverse (ADR-038 amendment B, B1). Pure
 * {@code javax.crypto}: the JVM test seals with a software key against a vector computed with Node's AES-GCM; the app
 * seals with the Android Keystore's key, which never leaves it.
 */
final class SecureSeal {
    /** The first byte of a sealed value. */
    static final int FORMAT = 1;
    /** Bytes of the IV, which the cipher draws for every encryption. */
    static final int IV_BYTES = 12;
    private static final int TAG_BITS = 128;
    private static final int TAG_BYTES = TAG_BITS / 8;
    private static final String TRANSFORMATION = "AES/GCM/NoPadding";

    private SecureSeal() {}

    /** The authenticated data of the entry {@code key}. */
    static byte[] aad(String key) {
        return ("undra.secure:" + key).getBytes(StandardCharsets.UTF_8);
    }

    /** Seals {@code plain} under {@code secret} for the entry {@code key}, with a fresh IV from the cipher. */
    static byte[] seal(SecretKey secret, String key, byte[] plain) throws GeneralSecurityException {
        Cipher cipher = Cipher.getInstance(TRANSFORMATION);
        cipher.init(Cipher.ENCRYPT_MODE, secret);
        return finishSeal(cipher, key, plain);
    }

    /** Seals with a given IV: for the test vector only (the Keystore refuses a caller's IV, as it should). */
    static byte[] sealWithIv(SecretKey secret, String key, byte[] plain, byte[] iv) throws GeneralSecurityException {
        Cipher cipher = Cipher.getInstance(TRANSFORMATION);
        cipher.init(Cipher.ENCRYPT_MODE, secret, new GCMParameterSpec(TAG_BITS, iv));
        return finishSeal(cipher, key, plain);
    }

    private static byte[] finishSeal(Cipher cipher, String key, byte[] plain) throws GeneralSecurityException {
        cipher.updateAAD(aad(key));
        byte[] sealed = cipher.doFinal(plain);
        byte[] iv = cipher.getIV();
        if (iv == null || iv.length != IV_BYTES) {
            throw new GeneralSecurityException("the cipher produced an IV of " + (iv == null ? 0 : iv.length) + " bytes, expected " + IV_BYTES);
        }
        byte[] out = new byte[1 + IV_BYTES + sealed.length];
        out[0] = (byte) FORMAT;
        System.arraycopy(iv, 0, out, 1, IV_BYTES);
        System.arraycopy(sealed, 0, out, 1 + IV_BYTES, sealed.length);
        return out;
    }

    /** Opens what {@link #seal} produced for the entry {@code key}; fails on any tampering or a wrong key. */
    static byte[] open(SecretKey secret, String key, byte[] stored) throws GeneralSecurityException {
        if (stored.length < 1 + IV_BYTES + TAG_BYTES || stored[0] != FORMAT) {
            throw new GeneralSecurityException("the value stored under '" + key + "' is not in the secure-store format");
        }
        Cipher cipher = Cipher.getInstance(TRANSFORMATION);
        cipher.init(Cipher.DECRYPT_MODE, secret, new GCMParameterSpec(TAG_BITS, stored, 1, IV_BYTES));
        cipher.updateAAD(aad(key));
        return cipher.doFinal(stored, 1 + IV_BYTES, stored.length - 1 - IV_BYTES);
    }
}
