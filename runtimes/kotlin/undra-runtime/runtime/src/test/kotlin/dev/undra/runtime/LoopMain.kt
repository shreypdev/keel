package dev.undra.runtime

fun main() {
    var failed = 0
    repeat(400) { failed += LazyListTests().runAll() }
    println("LOOP failed=$failed")
    kotlin.system.exitProcess(0)
}
