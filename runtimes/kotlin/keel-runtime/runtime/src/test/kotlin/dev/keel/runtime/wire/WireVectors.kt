// GENERATED FILE, DO NOT EDIT. Regenerate with scripts/gen-vectors.py.
// Source: contract-tests/wire-vectors.json (sha256 d8b8370e85e024a33459f5906bf964a1f834c5ab9abf269cf0bcda3a340288ab)
package dev.keel.runtime.wire

import dev.keel.runtime.testing.JV
import dev.keel.runtime.testing.WireVector

internal object WireVectors {
    const val SPEC: String = "docs/SPEC.md \u00a73"

    val all: List<WireVector> = listOf(
        WireVector("bool_true", "bool", JV.Bool(true), "01", ""),
        WireVector("bool_false", "bool", JV.Bool(false), "00", ""),
        WireVector("u8_255", "u8", JV.Num("255"), "ff", ""),
        WireVector("i32_neg", "i32", JV.Num("-2"), "feffffff", ""),
        WireVector("u32_max", "u32", JV.Num("4294967295"), "ffffffff", ""),
        WireVector("i64_big", "i64", JV.Str("-9007199254740993"), "ffffffffffffdfff", "beyond JS safe integer; TS uses bigint"),
        WireVector("u64_max", "u64", JV.Str("18446744073709551615"), "ffffffffffffffff", ""),
        WireVector("f32_pi", "f32", JV.Num("3.140000104904175"), "c3f54840", "f32 bits of 3.14"),
        WireVector("f64_e", "f64", JV.Num("2.718281828459045"), "6957148b0abf0540", ""),
        WireVector("string_empty", "string", JV.Str(""), "00000000", ""),
        WireVector("string_utf8", "string", JV.Str("h\u00e9llo \ud83c\udf0a"), "0b00000068c3a96c6c6f20f09f8c8a", ""),
        WireVector("bytes", "bytes", JV.Arr(listOf(JV.Num("1"), JV.Num("2"), JV.Num("3"), JV.Num("255"))), "04000000010203ff", ""),
        WireVector("option_none", "option<string>", JV.Null, "00", ""),
        WireVector("option_some", "option<string>", JV.Str("x"), "010100000078", ""),
        WireVector("vec_i32", "vec<i32>", JV.Arr(listOf(JV.Num("1"), JV.Num("-1"), JV.Num("7"))), "0300000001000000ffffffff07000000", ""),
        WireVector("vec_string_empty", "vec<string>", JV.Arr(listOf()), "00000000", ""),
        WireVector("map_string_i32", "map<string,i32>", JV.Obj(listOf("b" to JV.Num("2"), "a" to JV.Num("1"))), "02000000010000006101000000010000006202000000", "encoder sorts by encoded key bytes: 'a' before 'b'"),
        WireVector("duration_1_5s", "duration", JV.Str("1500000000"), "002f685900000000", "nanoseconds"),
        WireVector("timestamp", "timestamp", JV.Str("1727654400000"), "00103a4092010000", "unix ms"),
        WireVector("uuid", "uuid", JV.Str("123e4567-e89b-12d3-a456-426614174000"), "123e4567e89b12d3a456426614174000", "16 raw big-endian bytes"),
        WireVector("record_todo", "record Todo{id:uuid,title:string,done:bool}", JV.Obj(listOf("id" to JV.Str("123e4567-e89b-12d3-a456-426614174000"), "title" to JV.Str("Milk"), "done" to JV.Bool(false))), "123e4567e89b12d3a456426614174000040000004d696c6b00", ""),
        WireVector("enum_unit", "enum Filter{All,Active,Done}", JV.Str("done"), "0200", ""),
        WireVector("enum_data", "enum Shape{Circle{radius:f64},Rect{w:f64,h:f64}}", JV.Obj(listOf("kind" to JV.Str("rect"), "w" to JV.Num("2.0"), "h" to JV.Num("3.0"))), "010000000000000000400000000000000840", ""),
        WireVector("result_ok", "result<i32,string>", JV.Obj(listOf("ok" to JV.Num("5"))), "0005000000", ""),
        WireVector("result_err", "result<i32,string>", JV.Obj(listOf("err" to JV.Str("bad"))), "0103000000626164", ""),
        WireVector("handle", "handle", JV.Str("4294967297"), "0100000001000000", "index 1, generation 1"),
        WireVector("envelope_call", "envelope", JV.Obj(listOf("kind" to JV.Num("1"), "seq" to JV.Num("7"), "schema" to JV.Str("72623859790382856"), "payload_hex" to JV.Str("aabbcc"))), "4b45454c01000807060504030201010700000003000000aabbcc", "header is 23 bytes"),
        WireVector("fnv1a32_calculator_add", "fnv1a32(\"Calculator.add\")", JV.Str("2353348832"), "e040458c", ""),
        WireVector("fnv1a64_keel", "fnv1a64(\"keel\")", JV.Str("6367360722358687308"), "4cd6f65cd7685d58", ""),
        WireVector("call_method", "call payload", JV.Obj(listOf("target" to JV.Num("1"), "handle" to JV.Str("4294967297"), "method_id" to JV.Str("2353348832"), "call_id" to JV.Num("9"), "args" to JV.Arr(listOf(JV.Num("2"), JV.Num("3"))))), "010100000001000000e040458c090000000200000003000000", ""),
        WireVector("reply_ok", "reply payload", JV.Obj(listOf("call_id" to JV.Num("9"), "status" to JV.Num("0"), "body" to JV.Num("5"))), "090000000005000000", ""),
        WireVector("changeset_one", "changeset payload", JV.Obj(listOf("txn_id" to JV.Str("42"), "entries" to JV.Arr(listOf(JV.Obj(listOf("handle" to JV.Str("4294967297"), "signal_id" to JV.Num("0"), "op" to JV.Num("0"), "value" to JV.Arr(listOf(JV.Num("1"), JV.Num("2"))))))))), "2a0000000000000001000000010000000100000000000000000c000000020000000100000002000000", ""),
        WireVector("keyed_patch", "keyed patch (item i32)", JV.Obj(listOf("ops" to JV.Arr(listOf(JV.Obj(listOf("op" to JV.Str("insert"), "index" to JV.Num("0"), "item" to JV.Num("5"))), JV.Obj(listOf("op" to JV.Str("remove"), "index" to JV.Num("1"))), JV.Obj(listOf("op" to JV.Str("move"), "from" to JV.Num("0"), "to" to JV.Num("1"))), JV.Obj(listOf("op" to JV.Str("clear"))))))), "04000000000000000005000000010100000003000000000100000004", ""),
    )
}
