package dev.undra.contract

import dev.undra.playground.core.Note
import dev.undra.playground.core.Notes
import dev.undra.playground.core.dbCells
import dev.undra.playground.core.dbMigrate
import dev.undra.playground.core.dbRun
import dev.undra.runtime.adapters.DbConstraint
import dev.undra.runtime.adapters.DbError
import dev.undra.runtime.adapters.JdbcDbAdapter
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.dbPort
import java.nio.file.Files
import kotlinx.coroutines.runBlocking

/**
 * S25: the opt-in `Db` port through the JVM's real SQLite adapter (`JdbcDbAdapter`, `java.sql` with the SQLite JDBC driver),
 * rooted in a fresh temporary directory the scenario deletes; the core's side is `Notes`, `db_cells`, `db_run` and
 * `db_migrate`. Without a driver on the class path (`org.xerial:sqlite-jdbc`; `run.sh` adds `$UNDRA_SQLITE_JDBC`) it is
 * skipped, saying so.
 */
fun s25Db(w: World) {
    if (!JdbcDbAdapter.isDriverAvailable()) {
        if (System.getenv("UNDRA_REQUIRE_TOOLCHAINS") == "1") fail("UNDRA_REQUIRE_TOOLCHAINS=1 and no SQLite JDBC driver on the class path (set UNDRA_SQLITE_JDBC)")
        throw Skipped("no SQLite JDBC driver on the class path")
    }
    val dir = Files.createTempDirectory("undra-contract-s25")
    w.core.registerPort(StandardPorts.Db.PORT_ID, dbPort(JdbcDbAdapter(dir)))
    try {
        runBlocking { scenario() }
    } finally {
        dir.toFile().deleteRecursively()
    }
}

private suspend fun scenario() {
    // 1. Open: two migrations ran; no notes.
    val notes = Notes.create()
    expectEq("the version after opening contract-s25", 2u, notes.open("contract-s25"))
    awaitEq("the notes of a new database", emptyList<Note>()) { notes.notes.value }

    // 2. Add, toggle, count.
    expectEq("the id of milk", 1L, notes.add("milk").id)
    expectEq("the id of eggs", 2L, notes.add("eggs").id)
    awaitEq("the mirror after two adds", listOf(Note(1, "milk", false), Note(2, "eggs", false))) { notes.notes.value }
    notes.toggle(1)
    awaitEq("the mirror after toggling note 1", listOf(Note(1, "milk", true), Note(2, "eggs", false))) { notes.notes.value }
    expectEq("count()", 2u, notes.count())

    // 3. Every storage class there and back.
    val cells = dbCells(-9007199254740993L, 1.5, "é😀", byteArrayOf(0, 255.toByte(), 7), null)
    expectEq("the integer", -9007199254740993L, cells.int)
    expectEq("the real", 1.5, cells.real)
    expectEq("the text", "é😀", cells.text)
    expectEq("the blob", byteArrayOf(0, 255.toByte(), 7), cells.blob)
    expectEq("the null", null, cells.none)
    expectEq("the storage classes", listOf("integer", "real", "text", "blob", "null"), cells.types)

    // 4. Constraints, typed; a failed batch changes nothing.
    val duplicate = expectFailsAsync<DbError.Constraint>("addWithId(1, \"dup\")") { notes.addWithId(1, "dup") }
    expectEq("the kind of the duplicate id", DbConstraint.UNIQUE, duplicate.kind)
    val missing = expectFailsAsync<DbError.Constraint>("addAll([\"a\", null])") { notes.addAll(listOf("a", null)) }
    expectEq("the kind of the null title", DbConstraint.NOT_NULL, missing.kind)
    expectEq("count() after the failed batch", 2u, notes.count())
    holdsFor("the mirror after the failed batch") { notes.notes.value.size == 2 }
    expectEq("addAll([\"a\", \"b\"])", 2u, notes.addAll(listOf("a", "b")))
    expectEq("count() after the batch", 4u, notes.count())

    // 5. SQL errors, typed.
    expectFailsAsync<DbError.Sql>("INSERT INTO missing") { dbRun(":memory:", "INSERT INTO missing VALUES (1)") }
    expectFailsAsync<DbError.Sql>("SELEC 1") { dbRun(":memory:", "SELEC 1") }

    // 6. Migrations run in one transaction.
    val broken = expectFailsAsync<DbError.Migration>("dbMigrate(broken)") { dbMigrate("contract-s25-m", true) }
    expectEq("the version of the failed migration", 2u, broken.version)
    expectEq("dbMigrate(good) after the failed one", 2u, dbMigrate("contract-s25-m", false))

    // 7. Persistence.
    notes.closeDatabase()
    val reopened = Notes.create()
    expectEq("the version of the reopened database", 2u, reopened.open("contract-s25"))
    awaitEq("the notes of the reopened database", listOf("milk" to true, "eggs" to false, "a" to false, "b" to false)) {
        reopened.notes.value.map { it.title to it.done }
    }

    // 8. A name that leaves the directory.
    val third = Notes.create()
    expectFailsAsync<DbError.Unavailable>("open(\"../escape\")") { third.open("../escape") }
    for (store in listOf(notes, reopened, third)) store.close()
}
