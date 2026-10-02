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

    /**
     * {@code namespace} if it is a core namespace (a lowercase letter, then lowercase letters, digits and {@code _}, at most
     * 32 characters: the rule of {@code undra.toml}), else an {@link IllegalArgumentException}: it becomes a directory, a
     * Keystore alias and a file name, so {@code ..}, {@code /}, a NUL or a {@code -} never gets that far.
     */
    static String checked(String namespace) {
        if (namespace == null || namespace.isEmpty() || namespace.length() > 32) {
            throw new IllegalArgumentException("not an Undra core namespace (1 to 32 characters): " + shown(namespace));
        }
        for (int i = 0; i < namespace.length(); i++) {
            char c = namespace.charAt(i);
            boolean lower = c >= 'a' && c <= 'z';
            boolean other = (c >= '0' && c <= '9') || c == '_';
            if (!(lower || (i > 0 && other))) {
                throw new IllegalArgumentException("not an Undra core namespace (lowercase letters, digits and '_', starting with a letter): " + shown(namespace));
            }
        }
        return namespace;
    }

    private static String shown(String text) {
        if (text == null) return "null";
        String clipped = text.length() > 40 ? text.substring(0, 40) + "..." : text;
        StringBuilder out = new StringBuilder("`");
        for (int i = 0; i < clipped.length(); i++) {
            char c = clipped.charAt(i);
            if (c >= 0x20 && c < 0x7f) out.append(c);
            else out.append(String.format("\\u%04x", (int) c));
        }
        return out.append('`').toString();
    }

    /** The Keystore alias of the core {@code namespace}'s AES key: {@code <namespace>.dev.undra.securestore}. */
    static String keyAlias(String namespace) {
        return checked(namespace) + "." + KEY_ALIAS;
    }

    /** The directory of the core {@code namespace}'s sealed secrets below {@code noBackupFilesDir}: {@code undra/<namespace>/secure}. */
    static String securePath(String namespace) {
        return "undra/" + checked(namespace) + "/secure";
    }

    /** The file name of database {@code name} of the core {@code namespace}: {@code undra-<namespace>-<name>.sqlite}. */
    static String databaseFileName(String namespace, String name) {
        return "undra-" + checked(namespace) + "-" + name + ".sqlite";
    }
}
