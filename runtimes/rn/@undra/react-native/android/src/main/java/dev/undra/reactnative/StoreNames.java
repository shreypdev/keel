package dev.undra.reactnative;

/**
 * Where the default stores of a core are named, per core namespace (ADR-044 amendment A): the Keystore alias, the
 * directory of the sealed secrets and the file of a database, the same names as {@code android-adapters}
 * ({@code AndroidSecureStoreAdapter.keyAliasOf}, {@code directoryOf}, {@code AndroidDbAdapter.fileNameOf}) so either
 * shell of an app reads what the other wrote. Pure Java, so the JVM test checks them without Android.
 */
final class StoreNames {
    /** What the Keystore alias of a core's AES key is made of after its namespace. */
    static final String KEY_ALIAS = "dev.undra.securestore";

    private StoreNames() {}

    /** The Keystore alias of the core {@code namespace}'s AES key: {@code <namespace>.dev.undra.securestore}. */
    static String keyAlias(String namespace) {
        return namespace + "." + KEY_ALIAS;
    }

    /** The directory of the core {@code namespace}'s sealed secrets below {@code noBackupFilesDir}: {@code undra/<namespace>/secure}. */
    static String securePath(String namespace) {
        return "undra/" + namespace + "/secure";
    }

    /** The file name of database {@code name} of the core {@code namespace}: {@code undra-<namespace>-<name>.sqlite}. */
    static String databaseFileName(String namespace, String name) {
        return "undra-" + namespace + "-" + name + ".sqlite";
    }
}
