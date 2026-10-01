package dev.undra.reactnative;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/**
 * The bytes the {@code Db} port's JNI calls carry between the C++ binding ({@code cpp/UndraDb.cpp}) and
 * {@link UndraDatabase}: the wire format of docs/SPEC.md section 3 (little-endian), so one byte array crosses per
 * statement each way instead of a JNI call per value. Pure Java: {@code android/test/run.sh} checks it on the JVM.
 *
 * <ul>
 *   <li>parameters in: {@code Vec<DbValue>} ({@code u32} count, then each value: {@code u16} variant, {@code Null} 0,
 *       {@code Integer(i64)} 1, {@code Real(f64)} 2, {@code Text(String)} 3, {@code Blob(Bytes)} 4);</li>
 *   <li>a result out: {@code u8} 0 then the answer ({@code DbExecuted}: {@code u64 changes, i64 last_insert_id};
 *       {@code DbRows}: {@code Vec<String>} columns, {@code Vec<Vec<DbValue>>} rows), or {@code u8} 1 then the
 *       exception's class name and message as two {@code String}s, which C++ maps to a {@code DbError} by the
 *       {@code (code NNNN ...)} suffix Android's SQLite writes into its messages.</li>
 * </ul>
 */
final class DbWire {
    /** The first byte of a successful result. */
    static final int OK = 0;
    /** The first byte of a failure. */
    static final int FAILED = 1;

    private DbWire() {}

    /** The parameters of a statement: {@code null}, {@link Long}, {@link Double}, {@link String} or {@code byte[]}. */
    static Object[] readParams(byte[] bytes) {
        Reader in = new Reader(bytes);
        int count = in.u32();
        if (count < 0 || count > bytes.length) throw new IllegalArgumentException("malformed parameters");
        Object[] out = new Object[count];
        for (int i = 0; i < count; i++) {
            int variant = in.u16();
            switch (variant) {
                case 0 -> out[i] = null;
                case 1 -> out[i] = in.u64();
                case 2 -> out[i] = Double.longBitsToDouble(in.u64());
                case 3 -> out[i] = new String(in.bytes(), StandardCharsets.UTF_8);
                case 4 -> out[i] = in.bytes();
                default -> throw new IllegalArgumentException("unknown DbValue variant " + variant);
            }
        }
        if (!in.done()) throw new IllegalArgumentException("malformed parameters");
        return out;
    }

    /** {@code OK, DbExecuted}. */
    static byte[] executed(long changes, long lastInsertId) {
        Writer out = new Writer(17);
        out.u8(OK).u64(changes).u64(lastInsertId);
        return out.finish();
    }

    /** {@code FAILED, class name, message}. */
    static byte[] failure(String className, String message) {
        Writer out = new Writer(64);
        out.u8(FAILED).str(className).str(message == null ? "" : message);
        return out.finish();
    }

    /** A {@code DbRows} result, written as the cursor is read: columns first, then each row's cells in order. */
    static final class Rows {
        private final Writer out = new Writer(256);
        private final int columns;
        private final int countAt;
        private int rows;
        private int cells;

        Rows(String[] columns) {
            this.columns = columns.length;
            out.u8(OK).u32(columns.length);
            for (String column : columns) out.str(column);
            countAt = out.size;
            out.u32(0); // the row count, patched by finish()
        }

        /** Starts the next row. */
        void beginRow() {
            if (rows > 0 && cells != columns) throw new IllegalStateException("a row has " + cells + " cells, not " + columns);
            out.u32(columns);
            rows++;
            cells = 0;
        }

        void nullCell() {
            out.u16(0);
            cells++;
        }

        void integer(long value) {
            out.u16(1).u64(value);
            cells++;
        }

        void real(double value) {
            out.u16(2).u64(Double.doubleToRawLongBits(value));
            cells++;
        }

        void text(String value) {
            out.u16(3).str(value);
            cells++;
        }

        void blob(byte[] value) {
            out.u16(4).bytes(value);
            cells++;
        }

        byte[] finish() {
            if (rows > 0 && cells != columns) throw new IllegalStateException("a row has " + cells + " cells, not " + columns);
            out.patchU32(countAt, rows);
            return out.finish();
        }
    }

    /** Reads the wire format from a byte array. */
    private static final class Reader {
        private final byte[] bytes;
        private int at;

        Reader(byte[] bytes) {
            this.bytes = bytes;
        }

        private void need(int n) {
            if (n < 0 || bytes.length - at < n) throw new IllegalArgumentException("malformed parameters");
        }

        int u16() {
            need(2);
            int v = (bytes[at] & 0xff) | ((bytes[at + 1] & 0xff) << 8);
            at += 2;
            return v;
        }

        int u32() {
            need(4);
            int v = (bytes[at] & 0xff) | ((bytes[at + 1] & 0xff) << 8) | ((bytes[at + 2] & 0xff) << 16) | ((bytes[at + 3] & 0xff) << 24);
            at += 4;
            return v;
        }

        long u64() {
            need(8);
            long v = 0;
            for (int i = 0; i < 8; i++) v |= (bytes[at + i] & 0xffL) << (8 * i);
            at += 8;
            return v;
        }

        byte[] bytes() {
            int n = u32();
            need(n);
            byte[] out = Arrays.copyOfRange(bytes, at, at + n);
            at += n;
            return out;
        }

        boolean done() {
            return at == bytes.length;
        }
    }

    /** Writes the wire format into a growing array. */
    private static final class Writer {
        private byte[] buf;
        int size;

        Writer(int capacity) {
            buf = new byte[capacity];
        }

        private void room(int n) {
            if (buf.length - size < n) buf = Arrays.copyOf(buf, Math.max(buf.length * 2, size + n));
        }

        Writer u8(int v) {
            room(1);
            buf[size++] = (byte) v;
            return this;
        }

        Writer u16(int v) {
            room(2);
            buf[size++] = (byte) v;
            buf[size++] = (byte) (v >>> 8);
            return this;
        }

        Writer u32(int v) {
            room(4);
            for (int i = 0; i < 4; i++) buf[size++] = (byte) (v >>> (8 * i));
            return this;
        }

        Writer u64(long v) {
            room(8);
            for (int i = 0; i < 8; i++) buf[size++] = (byte) (v >>> (8 * i));
            return this;
        }

        Writer bytes(byte[] value) {
            u32(value.length);
            room(value.length);
            System.arraycopy(value, 0, buf, size, value.length);
            size += value.length;
            return this;
        }

        Writer str(String value) {
            return bytes(value.getBytes(StandardCharsets.UTF_8));
        }

        void patchU32(int at, int v) {
            for (int i = 0; i < 4; i++) buf[at + i] = (byte) (v >>> (8 * i));
        }

        byte[] finish() {
            return Arrays.copyOf(buf, size);
        }
    }
}
