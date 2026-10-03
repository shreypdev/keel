package com.acme.todo

import dev.undra.bazel.hello.Todos
import dev.undra.bazel.hello.UndraHelloCore
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.UndraDispatchers
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.coroutines.EmptyCoroutineContext
import kotlin.system.exitProcess
import kotlinx.coroutines.runBlocking

private fun <T> onMain(block: () -> T): T {
    val result = CompletableFuture<T>()
    UndraDispatchers.main.dispatch(EmptyCoroutineContext, Runnable { result.complete(block()) })
    return result.get(10, TimeUnit.SECONDS)
}

fun main() {
    val core = UndraHelloCore.load(LoadOptions())
    val todos = onMain { Todos() }
    runBlocking { todos.add("Ship it") }
    val line = onMain { summary(todos) }
    core.close()
    println("summary: $line")
    exitProcess(if (line == "1 of 1 left") 0 else 1)
}
