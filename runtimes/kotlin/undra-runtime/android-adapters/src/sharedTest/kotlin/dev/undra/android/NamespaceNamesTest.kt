package dev.undra.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

/** The names the default stores carry: per core namespace (ADR-044 amendment A), the same on every shell of an app. */
class NamespaceNamesTest {
    @Test
    fun the_keystore_alias_of_a_core_is_prefixed_with_its_namespace() {
        assertEquals("playground_a.dev.undra.securestore", AndroidSecureStoreAdapter.keyAliasOf("playground_a"))
        assertNotEquals(AndroidSecureStoreAdapter.keyAliasOf("playground_a"), AndroidSecureStoreAdapter.keyAliasOf("playground_b"))
        assertEquals("dev.undra.securestore", AndroidSecureStoreAdapter.DEFAULT_KEY_ALIAS)
    }

    @Test
    fun the_database_file_of_a_core_carries_its_namespace() {
        assertEquals("undra-playground_a-notes.sqlite", AndroidDbAdapter.fileNameOf("playground_a", "notes"))
        // A namespace has no `-`, so the first one after `undra-` ends it, whatever the database is called.
        val file = AndroidDbAdapter.fileNameOf("a", "b-c")
        assertEquals("undra-a-b-c.sqlite", file)
        assertEquals("a", file.removePrefix("undra-").substringBefore('-'))
    }
}
