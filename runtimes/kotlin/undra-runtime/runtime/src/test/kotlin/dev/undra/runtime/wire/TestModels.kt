package dev.undra.runtime.wire

import java.util.UUID

// Hand-written stand-ins for what undra-bindgen emits (SPEC §10.2), so the codecs are exercised the
// way generated code uses them: a record is its fields in order, an enum is a u16 variant index
// followed by the variant's fields, decoding an unknown index is an InvalidTag.

data class Todo(val id: UUID, val title: String, val done: Boolean) {
    companion object : UndraCodec<Todo> {
        override fun encode(w: UndraWriter, v: Todo) {
            Codecs.uuid.encode(w, v.id)
            Codecs.string.encode(w, v.title)
            Codecs.bool.encode(w, v.done)
        }

        override fun decode(r: UndraReader): Todo {
            val id = Codecs.uuid.decode(r)
            val title = Codecs.string.decode(r)
            val done = Codecs.bool.decode(r)
            return Todo(id, title, done)
        }
    }
}

enum class Filter(val index: UShort) {
    ALL(0u),
    ACTIVE(1u),
    DONE(2u),
    ;

    companion object : UndraCodec<Filter> {
        override fun encode(w: UndraWriter, v: Filter) = w.writeU16(v.index)

        override fun decode(r: UndraReader): Filter {
            val at = r.position
            val index = r.readU16()
            return entries.firstOrNull { it.index == index } ?: throw WireException.InvalidTag(index.toUInt(), at, "Filter")
        }
    }
}

sealed interface Shape {
    data class Circle(val radius: Double) : Shape

    data class Rect(val w: Double, val h: Double) : Shape

    companion object : UndraCodec<Shape> {
        override fun encode(w: UndraWriter, v: Shape) {
            when (v) {
                is Circle -> {
                    w.writeU16(0u)
                    w.writeF64(v.radius)
                }
                is Rect -> {
                    w.writeU16(1u)
                    w.writeF64(v.w)
                    w.writeF64(v.h)
                }
            }
        }

        override fun decode(r: UndraReader): Shape {
            val at = r.position
            return when (val index = r.readU16().toInt()) {
                0 -> Circle(r.readF64())
                1 -> {
                    val width = r.readF64()
                    Rect(width, r.readF64())
                }
                else -> throw WireException.InvalidTag(index.toUInt(), at, "Shape")
            }
        }
    }
}
