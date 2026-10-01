package dev.undra.reactnative;

import android.content.Context;
import android.database.Cursor;
import android.database.DatabaseErrorHandler;
import android.database.sqlite.SQLiteCursor;
import android.database.sqlite.SQLiteDatabase;
import java.io.File;

/**
 * One connection of the {@code Db} port on Android (ADR-048): the SQLite that Android ships, through
 * {@code android.database.sqlite}, for the C++ binding ({@code cpp/UndraDb.cpp}), which owns everything else (ids,
 * migrations, transactions, the busy timeout, the error mapping). The NDK has no public sqlite3, so the binding reaches
 * this class over JNI ({@code cpp/UndraPlatformAndroid.cpp}), always from the one thread of its database: Android ties a
 * transaction to the thread that began it.
 *
 * <ul>
 *   <li>The file is {@code getDatabasePath("undra-<name>.sqlite")}, the one {@code android-adapters}'
 *       {@code AndroidDbAdapter} opens, so either shell of an app reads the other's database; {@code ":memory:"} is a
 *       private in-memory database.</li>
 *   <li>Opened with {@code NO_LOCALIZED_COLLATORS} (no {@code android_metadata} table in the app's schema), an error
 *       handler that keeps a corrupt file (Android's default deletes it; the core gets {@code DbError.Corrupt}), and
 *       Android's own write-ahead logging off: it would give the database a pool of connections, and the binding needs
 *       one ({@code changes()} and {@code last_insert_rowid()} are per connection). The binding then runs
 *       {@code PRAGMA journal_mode = WAL} itself, on that one connection.</li>
 *   <li>Every statement runs through a cursor whose factory binds the parameters by type ({@code bindLong},
 *       {@code bindDouble}, {@code bindString}, {@code bindBlob}, {@code bindNull}; {@code rawQuery} would bind them all
 *       as text), after Android compiled it, and refuses trailing SQL and a wrong parameter count there, so a syntax
 *       error is reported first, as the sqlite3 C API does on iOS. The C++ lexer tells it both: Android prepares only the
 *       first statement of a string and exposes no parameter count.</li>
 *   <li>Nothing throws to C++: a failure is returned as {@link DbWire#failure} with the exception's class and message,
 *       whose {@code (code NNNN SQLITE_...)} suffix C++ maps to the typed error.</li>
 * </ul>
 */
final class UndraDatabase {
    /** Corruption is the core's to see ({@code DbError.Corrupt}): Android's default handler would delete the file. */
    private static final DatabaseErrorHandler KEEP_THE_FILE = database -> {};

    private final SQLiteDatabase db;
    private final byte[] failure;

    private UndraDatabase(SQLiteDatabase db, byte[] failure) {
        this.db = db;
        this.failure = failure;
    }

    /** A statement the binding refuses after Android compiled it (C++ reports it as {@code DbError.Sql}). */
    static final class Refused extends RuntimeException {
        private static final long serialVersionUID = 1L;

        Refused(String message) {
            super(message);
        }
    }

    /** The directory of the databases ({@code getDatabasePath}'s), or {@code null} before {@link UndraPlatform#install}. */
    static String directory() {
        Context app = UndraPlatform.context();
        if (app == null) return null;
        File parent = app.getDatabasePath("undra-.sqlite").getParentFile();
        return parent == null ? null : parent.getPath();
    }

    /** Opens database {@code name}; never throws: {@link #failure} says why it did not open. */
    static UndraDatabase open(String name) {
        try {
            String path;
            if (":memory:".equals(name)) {
                path = ":memory:";
            } else {
                Context app = UndraPlatform.context();
                if (app == null) {
                    return failed("android.database.sqlite.SQLiteCantOpenDatabaseException",
                            "UndraPlatform has no context: keep its provider in the manifest, or call UndraPlatform.install(context)");
                }
                File file = app.getDatabasePath("undra-" + name + ".sqlite");
                File directory = file.getParentFile();
                if (directory != null && !directory.isDirectory() && !directory.mkdirs() && !directory.isDirectory()) {
                    return failed("android.database.sqlite.SQLiteCantOpenDatabaseException", "cannot create " + directory);
                }
                path = file.getPath();
            }
            SQLiteDatabase db = SQLiteDatabase.openDatabase(
                    path, null, SQLiteDatabase.CREATE_IF_NECESSARY | SQLiteDatabase.NO_LOCALIZED_COLLATORS, KEEP_THE_FILE);
            try {
                // One connection (see the class comment); a no-op where Android did not turn its WAL on.
                db.disableWriteAheadLogging();
            } catch (RuntimeException e) {
                db.close();
                throw e;
            }
            return new UndraDatabase(db, null);
        } catch (RuntimeException e) {
            return failed(e.getClass().getName(), e.getMessage());
        }
    }

    private static UndraDatabase failed(String className, String message) {
        return new UndraDatabase(null, DbWire.failure(className, message));
    }

    /** Why {@link #open} failed ({@link DbWire#failure}), or {@code null} when the database is open. */
    byte[] failure() {
        return failure;
    }

    /**
     * Runs one statement to its end: {@link DbWire#executed} with SQLite's {@code changes()} and
     * {@code last_insert_rowid()} of this connection, or a failure. {@code expected} is the statement's parameter count
     * ({@code -1}: not checked, a migration's statement), {@code trailing} whether SQL followed it.
     */
    byte[] execute(String sql, byte[] params, int expected, boolean trailing) {
        try {
            Object[] args = DbWire.readParams(params);
            try (Cursor cursor = run(sql, args, expected, trailing)) {
                cursor.getCount(); // steps the statement to its end
            }
            try (Cursor counts = db.rawQuery("SELECT changes(), last_insert_rowid()", null)) {
                if (!counts.moveToFirst()) return DbWire.executed(0, 0);
                return DbWire.executed(counts.getLong(0), counts.getLong(1));
            }
        } catch (RuntimeException e) {
            return DbWire.failure(e.getClass().getName(), e.getMessage());
        }
    }

    /** Runs one query: {@code DbRows} ({@link DbWire.Rows}), each cell by its storage class, or a failure. */
    byte[] query(String sql, byte[] params, int expected, boolean trailing) {
        try {
            Object[] args = DbWire.readParams(params);
            try (Cursor cursor = run(sql, args, expected, trailing)) {
                String[] names = cursor.getColumnNames();
                DbWire.Rows rows = new DbWire.Rows(names);
                while (cursor.moveToNext()) {
                    rows.beginRow();
                    for (int c = 0; c < names.length; c++) {
                        switch (cursor.getType(c)) {
                            case Cursor.FIELD_TYPE_NULL -> rows.nullCell();
                            case Cursor.FIELD_TYPE_INTEGER -> rows.integer(cursor.getLong(c));
                            case Cursor.FIELD_TYPE_FLOAT -> rows.real(cursor.getDouble(c));
                            case Cursor.FIELD_TYPE_STRING -> rows.text(cursor.getString(c));
                            default -> rows.blob(cursor.getBlob(c));
                        }
                    }
                }
                return rows.finish();
            }
        } catch (RuntimeException e) {
            return DbWire.failure(e.getClass().getName(), e.getMessage());
        }
    }

    /** Closes the connection. */
    void close() {
        if (db != null) db.close();
    }

    /** A cursor over {@code sql} with {@code args} bound by type once Android has compiled it. */
    private Cursor run(String sql, Object[] args, int expected, boolean trailing) {
        SQLiteDatabase.CursorFactory factory = (database, driver, editTable, query) -> {
            if (trailing) throw new Refused("only one statement per call: use a migration for several");
            if (expected >= 0 && expected != args.length) {
                throw new Refused("the statement has " + expected + " parameters, " + args.length + " were given");
            }
            for (int i = 0; i < args.length; i++) {
                int index = i + 1;
                Object value = args[i];
                if (value == null) {
                    query.bindNull(index);
                } else if (value instanceof Long integer) {
                    query.bindLong(index, integer);
                } else if (value instanceof Double real) {
                    query.bindDouble(index, real);
                } else if (value instanceof String text) {
                    query.bindString(index, text);
                } else {
                    query.bindBlob(index, (byte[]) value);
                }
            }
            return new SQLiteCursor(driver, editTable, query);
        };
        return db.rawQueryWithFactory(factory, sql, null, null);
    }
}
