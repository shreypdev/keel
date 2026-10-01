package dev.undra.reactnative;

import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.util.Arrays;
import javax.crypto.SecretKey;
import javax.crypto.spec.SecretKeySpec;

/**
 * The pure parts of the package's Android library on the JVM (ADR-038 amendment B, B10): the secure-store seal with a
 * software key, against a vector computed independently with Node's AES-256-GCM (the layout of android-adapters and
 * the web adapter), the network classification, and the bytes the Db port's JNI calls carry ({@link DbWire}). The Keystore and ConnectivityManager themselves are exercised on
 * the device (scripts/rn-device-checks.sh). Run by android/test/run.sh; prints {@code ok - <name>} per check.
 */
public final class PureTest {
    private static int checks = 0;

    private static void check(boolean condition, String what) {
        if (!condition) {
            System.err.println("not ok - " + what);
            System.exit(1);
        }
    }

    private static void ok(String name) {
        checks++;
        System.out.println("ok - " + name);
    }

    private static byte[] hex(String text) {
        byte[] out = new byte[text.length() / 2];
        for (int i = 0; i < out.length; i++) out[i] = (byte) Integer.parseInt(text.substring(2 * i, 2 * i + 2), 16);
        return out;
    }

    private static byte[] range(int start, int count) {
        byte[] out = new byte[count];
        for (int i = 0; i < count; i++) out[i] = (byte) (start + i);
        return out;
    }

    public static void main(String[] args) throws Exception {
        SecretKey key = new SecretKeySpec(range(0, 32), "AES");
        byte[] plain = "s3cret".getBytes(StandardCharsets.UTF_8);
        // node: aes-256-gcm, key 00..1f, iv a0..ab, AAD "undra.secure:token", plaintext "s3cret".
        byte[] vector = hex("01a0a1a2a3a4a5a6a7a8a9aaab952b1f5f20bf2d475b03b52d2d61ff36a3bb8f71e57b");
        check(Arrays.equals(SecureSeal.sealWithIv(key, "token", plain, range(0xa0, 12)), vector), "the sealed bytes are the vector");
        check(Arrays.equals(SecureSeal.open(key, "token", vector), plain), "the vector opens");
        ok("the seal is format, iv, ciphertext and tag with the key name as AAD (a vector from Node's AES-GCM)");

        byte[] sealed = SecureSeal.seal(key, "token", plain);
        byte[] again = SecureSeal.seal(key, "token", plain);
        check(sealed.length == 1 + 12 + plain.length + 16 && sealed[0] == 1, "the layout's length and format byte");
        check(!Arrays.equals(Arrays.copyOfRange(sealed, 1, 13), Arrays.copyOfRange(again, 1, 13)), "a fresh IV for every seal");
        check(Arrays.equals(SecureSeal.open(key, "token", sealed), plain), "a seal opens");
        ok("every seal draws a fresh IV and opens");

        boolean moved = false;
        try {
            SecureSeal.open(key, "other", vector);
        } catch (GeneralSecurityException e) {
            moved = true;
        }
        check(moved, "a value moved to another key fails authentication");
        byte[] tampered = vector.clone();
        tampered[tampered.length - 1] ^= 1;
        boolean caught = false;
        try {
            SecureSeal.open(key, "token", tampered);
        } catch (GeneralSecurityException e) {
            caught = true;
        }
        check(caught, "a tampered value fails authentication");
        boolean format = false;
        try {
            SecureSeal.open(key, "token", new byte[] {2, 0, 0});
        } catch (GeneralSecurityException e) {
            format = true;
        }
        check(format, "a value that is not in the format is refused, not 'missing'");
        ok("moved, tampered and foreign values fail instead of reading as missing");

        check(NetworkClassifier.classify(false, true, false, false) == NetworkClassifier.NONE, "no internet: none");
        check(NetworkClassifier.classify(true, true, true, false) == NetworkClassifier.WIFI, "Wi-Fi first");
        check(NetworkClassifier.classify(true, false, true, true) == NetworkClassifier.CELLULAR, "then cellular");
        check(NetworkClassifier.classify(true, false, false, true) == NetworkClassifier.WIRED, "then wired");
        check(NetworkClassifier.classify(true, false, false, false) == NetworkClassifier.UNKNOWN, "else unknown");
        check(NetworkClassifier.WIFI == 0 && NetworkClassifier.NONE == 4, "NetKind wire indices");
        ok("the network classification is android-adapters' (and NWPath's)");

        // The Db port's bytes across JNI (DbWire): the wire format of docs/SPEC.md section 3, as UndraDb.cpp writes it.
        byte[] params = hex("05000000" + "0000" + "0100feffffffffffffff" + "0200000000000000f03f" + "030002000000c3a9" + "04000100000009");
        Object[] values = DbWire.readParams(params);
        check(values.length == 5 && values[0] == null, "Null is null");
        check(Long.valueOf(-2).equals(values[1]), "Integer(-2) is a Long");
        check(Double.valueOf(1.0).equals(values[2]), "Real(1.0) is a Double");
        check("\u00e9".equals(values[3]), "Text is a String, from UTF-8");
        check(values[4] instanceof byte[] blob && Arrays.equals(blob, new byte[] {9}), "Blob is a byte[]");
        check(DbWire.readParams(hex("00000000")).length == 0, "no parameters");
        for (String bad : new String[] {"01000000", "010000000900", "0100000001000000", "00000000ff"}) {
            boolean refused = false;
            try {
                DbWire.readParams(hex(bad));
            } catch (IllegalArgumentException e) {
                refused = true;
            }
            check(refused, "malformed parameters are refused: " + bad);
        }
        ok("DbWire reads the parameters of a statement (every DbValue variant; malformed input refused)");

        check(Arrays.equals(DbWire.executed(3, -1), hex("00" + "0300000000000000" + "ffffffffffffffff")), "OK, DbExecuted { 3, -1 }");
        check(Arrays.equals(DbWire.failure("C", "m"), hex("01" + "0100000043" + "010000006d")), "FAILED, class, message");
        DbWire.Rows rows = new DbWire.Rows(new String[] {"a", "b"});
        rows.beginRow();
        rows.integer(1);
        rows.text("x");
        rows.beginRow();
        rows.nullCell();
        rows.blob(new byte[0]);
        rows.beginRow();
        rows.real(-0.5);
        rows.blob(new byte[] {0, (byte) 0xff});
        byte[] expected = hex("00" + "02000000" + "0100000061" + "0100000062" + "03000000"
                + "02000000" + "01000100000000000000" + "03000100000078"
                + "02000000" + "0000" + "040000000000"
                + "02000000" + "0200000000000000e0bf" + "04000200000000ff");
        check(Arrays.equals(rows.finish(), expected), "OK, DbRows: the columns, then each row's cells");
        check(Arrays.equals(new DbWire.Rows(new String[0]).finish(), hex("00" + "00000000" + "00000000")), "no columns, no rows");
        ok("DbWire writes DbExecuted, DbRows and a failure as the C++ side reads them");

        System.out.println("# " + checks + " checks passed");
    }
}
