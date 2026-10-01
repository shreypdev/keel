// Execution test of the `recursive` golden case: the generated Swift is built with the real
// runtime and this file is its `main.swift`. It exits non-zero and prints the failed checks when
// a record that holds itself stops behaving like the plain value it stands for.

import Foundation
import GoldenRecursive
import UndraRuntime

nonisolated(unsafe) var failures: [String] = []

func check(_ ok: Bool, _ what: String) {
    if !ok { failures.append(what) }
}

func json<T: Encodable>(_ value: T) throws -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    return String(decoding: try encoder.encode(value), as: UTF8.self)
}

func run() throws {
    // A three-node list, copied and mutated through the public property.
    let list = ListNode(value: 1, next: ListNode(value: 2, next: ListNode(value: 3, next: nil)))
    var copy = list
    copy.next?.next?.value = 99
    check(list.next?.next?.value == 3, "a copy's mutation must not reach the original")
    check(copy.next?.next?.value == 99, "the copy must see its own mutation")
    check(list != copy, "== must tell the mutated copy apart")
    check(list.hashValue != copy.hashValue, "the hash must tell the mutated copy apart")
    copy.next?.next?.value = 3
    check(list == copy && list.hashValue == copy.hashValue, "equal lists are equal and hash alike")
    check(Set([list, copy]).count == 1, "equal lists collapse in a Set")

    var shortened = list
    shortened.next = nil
    check(shortened.next == nil && list.next != nil, "assigning nil must leave the original alone")
    shortened.next = ListNode(value: 7, next: nil)
    check(shortened.next?.value == 7 && shortened.next?.next == nil, "assigning a node")
    var longer = ListNode(value: 0, next: list)
    check(longer.next?.next?.next?.value == 3, "the initializer takes a plain optional")
    longer.next = longer
    check(longer.next?.value == 0 && longer.next?.next?.value == 1, "assigning a copy of itself")

    // JSON: a nil child is omitted, a missing key and `null` decode as nil.
    let text = try json(list)
    check(text == #"{"next":{"next":{"value":3},"value":2},"value":1}"#, "JSON shape: \(text)")
    check(try JSONDecoder().decode(ListNode.self, from: Data(text.utf8)) == list, "JSON round trip")
    let missing = try JSONDecoder().decode(ListNode.self, from: Data(#"{"value":7}"#.utf8))
    check(missing.value == 7 && missing.next == nil, "a missing key decodes as nil")
    let null = try JSONDecoder().decode(ListNode.self, from: Data(#"{"value":7,"next":null}"#.utf8))
    check(null.next == nil, "null decodes as nil")

    let leaf = Tree(label: "leaf", children: [])
    let tree = Tree(label: "root", children: [leaf, Tree(label: "mid", children: [leaf])])
    check(try JSONDecoder().decode(Tree.self, from: Data(try json(tree).utf8)) == tree, "Tree JSON")

    let family = Parent(name: "p", child: Child(name: "c", parent: Parent(name: "pp", child: nil)))
    let familyText = try json(family)
    check(familyText == #"{"child":{"name":"c","parent":{"name":"pp"}},"name":"p"}"#, familyText)
    check(try JSONDecoder().decode(Parent.self, from: Data(familyText.utf8)) == family, "mutual JSON")

    // The wire: the same bytes as the fields written by hand, and every shape round-trips.
    let bytes = list.undraEncoded()
    check(bytes == [1, 0, 0, 0, 1, 2, 0, 0, 0, 1, 3, 0, 0, 0, 0], "wire bytes of the list: \(bytes)")
    check(try ListNode.undraDecoded(from: bytes) == list, "list wire round trip")
    check(try Tree.undraDecoded(from: tree.undraEncoded()) == tree, "tree wire round trip")
    check(try Parent.undraDecoded(from: family.undraEncoded()) == family, "mutual wire round trip")
    let expr = Expr.grouped(Group(label: "g", inner: .grouped(Group(label: "h", inner: .num(2.5)))))
    check(try Expr.undraDecoded(from: expr.undraEncoded()) == expr, "record and enum cycle")
    let path = Path.step(label: "a", rest: .step(label: "b", rest: .end))
    check(try Path.undraDecoded(from: path.undraEncoded()) == path, "indirect enum")
    let error = ParseError.nested(depth: 2, inner: .nested(depth: 1, inner: .eof))
    check(try ParseError.undraDecoded(from: error.undraEncoded()) == error, "indirect error enum")
    check(error.description == "failed at depth 2", "error description")

    var deep = ListNode(value: 0, next: nil)
    for i in 1..<100 { deep = ListNode(value: Int32(i), next: deep) }
    check(try ListNode.undraDecoded(from: deep.undraEncoded()) == deep, "a 100-node list")
}

do {
    try run()
} catch {
    failures.append("threw \(error)")
}
if failures.isEmpty {
    print("recursive: all checks passed")
} else {
    for failure in failures { print("FAILED: \(failure)") }
    exit(1)
}
