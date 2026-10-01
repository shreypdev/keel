#!/usr/bin/env python3
"""Generate the Kotlin vector table from the shared wire test vectors.

    scripts/gen-vectors.py           # rewrite WireVectors.kt
    scripts/gen-vectors.py --check   # exit 1 if the checked-in WireVectors.kt is stale

Input:  contract-tests/wire-vectors.json  (the cross-language source of truth, SPEC section 3)
Output: runtime/src/test/kotlin/dev/undra/runtime/wire/WireVectors.kt

The output mirrors the JSON structurally (see testing/Vectors.kt): numbers keep their source text,
object entries keep their order. The tests decide how to interpret each vector by its `type`, and
fail on a type they do not know, so a new vector can never be silently skipped.
"""
import hashlib
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
UNDRA_RUNTIME = HERE.parent
REPO = HERE.parents[3]
SOURCE = REPO / "contract-tests" / "wire-vectors.json"
TARGET = UNDRA_RUNTIME / "runtime/src/test/kotlin/dev/undra/runtime/wire/WireVectors.kt"


class Num(str):
    """A JSON number, kept as its source text."""


class Obj(list):
    """A JSON object, kept as an ordered list of (key, value) pairs."""


def kotlin_string(s: str) -> str:
    out = ['"']
    for unit in s:
        code = ord(unit)
        if unit in ('"', "\\", "$"):
            out.append("\\" + unit)
        elif 0x20 <= code < 0x7F:
            out.append(unit)
        elif code > 0xFFFF:
            code -= 0x10000
            out.append("\\u%04x\\u%04x" % (0xD800 + (code >> 10), 0xDC00 + (code & 0x3FF)))
        else:
            out.append("\\u%04x" % code)
    out.append('"')
    return "".join(out)


def emit(v) -> str:
    if v is None:
        return "JV.Null"
    if v is True:
        return "JV.Bool(true)"
    if v is False:
        return "JV.Bool(false)"
    if isinstance(v, Num):
        return "JV.Num(%s)" % kotlin_string(str(v))
    if isinstance(v, str):
        return "JV.Str(%s)" % kotlin_string(v)
    if isinstance(v, Obj):
        return "JV.Obj(listOf(%s))" % ", ".join("%s to %s" % (kotlin_string(k), emit(x)) for k, x in v)
    if isinstance(v, list):
        return "JV.Arr(listOf(%s))" % ", ".join(emit(x) for x in v)
    raise TypeError("unsupported JSON value: %r" % (v,))


def generate() -> str:
    text = SOURCE.read_text(encoding="utf-8")
    doc = json.loads(text, parse_int=Num, parse_float=Num, object_pairs_hook=Obj)
    digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
    lines = [
        "// GENERATED FILE, DO NOT EDIT. Regenerate with scripts/gen-vectors.py.",
        "// Source: contract-tests/wire-vectors.json (sha256 %s)" % digest,
        "package dev.undra.runtime.wire",
        "",
        "import dev.undra.runtime.testing.JV",
        "import dev.undra.runtime.testing.WireVector",
        "",
        "internal object WireVectors {",
        "    const val SPEC: String = %s" % kotlin_string(dict(doc)["spec"]),
        "",
        "    val all: List<WireVector> = listOf(",
    ]
    for vec in dict(doc)["vectors"]:
        f = dict(vec)
        lines.append(
            "        WireVector(%s, %s, %s, %s, %s),"
            % (
                kotlin_string(f["name"]),
                kotlin_string(f["type"]),
                emit(f["value"]),
                kotlin_string(f["hex"]),
                kotlin_string(f.get("note", "")),
            )
        )
    lines += ["    )", "}", ""]
    return "\n".join(lines)


def main() -> int:
    generated = generate()
    if "--check" in sys.argv[1:]:
        current = TARGET.read_text(encoding="utf-8") if TARGET.exists() else ""
        if current != generated:
            sys.stderr.write(
                "WireVectors.kt is stale; run runtimes/kotlin/undra-runtime/scripts/gen-vectors.py\n"
            )
            return 1
        return 0
    TARGET.write_text(generated, encoding="utf-8")
    print("wrote %s" % TARGET.relative_to(REPO))
    return 0


if __name__ == "__main__":
    sys.exit(main())
