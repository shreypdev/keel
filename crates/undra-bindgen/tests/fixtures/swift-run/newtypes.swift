// Execution test of the `newtypes` golden case (ADR-042): the generated Swift is built with the real
// runtime and this file is its `main.swift`. A newtype crosses as the bytes of its inner value, is
// `Hashable`, `Codable` as that value, and `Comparable` only where the order of the inner type means
// something.

import Foundation
import GoldenNewtypes
import UndraRuntime

nonisolated(unsafe) var failures: [String] = []

func check(_ ok: Bool, _ what: String) {
    if !ok { failures.append(what) }
}

func json<T: Encodable>(_ value: T) throws -> String {
    String(decoding: try JSONEncoder().encode(value), as: UTF8.self)
}

func run() throws {
    let id = UUID(uuidString: "00000000-0000-0000-0000-0000000000AB")!

    // The bytes are the inner value's: no tag, no length.
    check(UserId(id).undraEncoded() == id.undraEncoded(), "UserId bytes")
    check(TodoId("t").undraEncoded() == "t".undraEncoded(), "TodoId bytes")
    check(OrderNo(7).undraEncoded() == UInt64(7).undraEncoded(), "OrderNo bytes")
    check(Meters(1.5).undraEncoded() == Double(1.5).undraEncoded(), "Meters bytes")
    check(Flag(true).undraEncoded() == true.undraEncoded(), "Flag bytes")
    check(Tags(["a", "b"]).undraEncoded() == ["a", "b"].undraEncoded(), "Tags bytes")
    check(Nickname("n").undraEncoded() == Optional("n").undraEncoded(), "Nickname bytes")
    check(Nickname(nil).undraEncoded() == [0], "an empty Nickname is the option's tag")
    check(Boss(Owner(UserId(id))).undraEncoded() == id.undraEncoded(), "nested newtypes are the same bytes")
    check(Maybe(UserId(id)).undraEncoded() == Optional(id).undraEncoded(), "an option of a newtype")
    let blob = Blob([1, 2, 3])
    check(try Blob.undraDecoded(from: blob.undraEncoded()) == blob, "Blob round trip")
    check(blob.undraEncoded() == UndraBytes([1, 2, 3]).undraEncoded(), "Blob bytes")
    let timeout = Timeout(.milliseconds(1500))
    check(try Timeout.undraDecoded(from: timeout.undraEncoded()) == timeout, "Timeout round trip")

    // They decode what they encode, and refuse what is cut short.
    check(try UserId.undraDecoded(from: UserId(id).undraEncoded()) == UserId(id), "UserId round trip")
    check(try Boss.undraDecoded(from: Boss(Owner(UserId(id))).undraEncoded()).rawValue.rawValue.rawValue == id, "unwrapping")
    do {
        _ = try UserId.undraDecoded(from: [1, 2, 3])
        failures.append("a truncated UserId must fail")
    } catch is WireError {
    }

    // `RawRepresentable`, `Hashable`, and a dictionary keyed by a newtype.
    check(UserId(rawValue: id).rawValue == id, "rawValue")
    check(UserId(id) == UserId(rawValue: id), "the two initializers agree")
    check(Set([UserId(id), UserId(id)]).count == 1, "equal ids collapse in a Set")
    let scores: [UserId: Meters] = [UserId(id): Meters(2)]
    check(scores[UserId(id)] == Meters(2), "a newtype is a dictionary key")
    check(try [UserId: Meters].undraDecoded(from: scores.undraEncoded()) == scores, "a map keyed by a newtype")

    // `Comparable` where the order means something.
    check(Meters(1) < Meters(2) && !(Meters(2) < Meters(1)), "Meters")
    check(Span(Meters(1)) < Span(Meters(2)), "a newtype of an ordered newtype")
    check(TodoId("a") < TodoId("b"), "TodoId")
    check(OrderNo(1) < OrderNo(2), "OrderNo")
    check(Timeout(.milliseconds(1)) < Timeout(.milliseconds(2)), "Timeout")
    check([Meters(3), Meters(1)].sorted() == [Meters(1), Meters(3)], "sorted")

    // `Codable` as the inner value, in every format.
    check(try json(UserId(id)) == "\"00000000-0000-0000-0000-0000000000AB\"", "UserId JSON")
    check(try JSONDecoder().decode(UserId.self, from: Data("\"00000000-0000-0000-0000-0000000000AB\"".utf8)) == UserId(id), "UserId JSON decode")
    check(try json(Meters(1.5)) == "1.5", "Meters JSON")
    check(try json(Tags(["a"])) == "[\"a\"]", "Tags JSON")
    check(try json(Nickname(nil)) == "null", "an empty Nickname is null")
    check(try JSONDecoder().decode(Nickname.self, from: Data("null".utf8)) == Nickname(nil), "null decodes to an empty Nickname")
    check(try json(Boss(Owner(UserId(id)))) == "\"00000000-0000-0000-0000-0000000000AB\"", "nested newtypes in JSON")
    check(try JSONDecoder().decode(Todo.self, from: JSONEncoder().encode(Todo(id: TodoId("t"), title: "x", owner: UserId(id), due: nil, level: Level(.low)))) == Todo(id: TodoId("t"), title: "x", owner: UserId(id), due: nil, level: Level(.low)), "a record of newtypes in JSON")
}

do {
    try run()
} catch {
    failures.append("threw \(error)")
}
if failures.isEmpty {
    print("newtypes: all checks passed")
} else {
    for failure in failures { print("FAILED: \(failure)") }
    exit(1)
}
