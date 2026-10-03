package com.acme.todo

import dev.undra.bazel.hello.Todo
import dev.undra.bazel.hello.Todos

/** One line for the app's header: how many items are left. */
fun summary(todos: Todos): String {
    val all: List<Todo> = todos.todos.value
    return "${todos.remaining.value} of ${all.size} left"
}
