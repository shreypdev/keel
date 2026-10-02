import Foundation
import PlaygroundCore
@testable import UndraRuntime
import XCTest

/// The wire bytes of a decimal: a 128-bit two's-complement mantissa (16 little-endian bytes), then the scale. Built from numbers,
/// never from text: Foundation's parser compacts trailing zeros, so `1.10` parsed from a string is `1.1`.
private func wireDecimal(_ mantissa: Int64, scale: UInt8) -> [UInt8] {
    var bytes = withUnsafeBytes(of: UInt64(bitPattern: mantissa).littleEndian) { Array($0) }
    bytes.append(contentsOf: [UInt8](repeating: mantissa < 0 ? 0xFF : 0x00, count: 8))
    bytes.append(scale)
    return bytes
}

/// The same for a mantissa given as its sixteen little-endian bytes (the 128-bit extremes).
private func wireDecimal(raw mantissa: [UInt8], scale: UInt8) -> [UInt8] {
    return mantissa + [scale]
}

/// `i128::MAX` and `i128::MIN`, little-endian.
private let largestMantissa: [UInt8] = [UInt8](repeating: 0xFF, count: 15) + [0x7F]
private let smallestMantissa: [UInt8] = [UInt8](repeating: 0x00, count: 15) + [0x80]

/// An amount from wire bytes, so its scale is exactly the one written.
private func amount(_ bytes: [UInt8]) throws -> Price {
    return Price(try Decimal.undraDecoded(from: bytes))
}

extension ContractScenarios {
    // MARK: S31

    /// ADR-042: newtypes cross as their inner value, are map keys and keyed-list keys; named generic instantiations are plain
    /// records and enums; `Decimal` is exact; leaf types of other crates cross as wire types. The `ledger` module of the
    /// playground core, through the generated bindings.
    func testS31_newtypesGenericsAndLeafTypes() async {
        await scenario("S31", "newtypes, generic instantiations and leaf types") {
            let core = try self.core

            // 1. A newtype is its inner value on the wire.
            let uuid = UUID(uuidString: "12345678-9abc-def0-0102-030405060708")!
            let id = AccountId(uuid)
            try checkEqual(id.undraEncoded(), uuid.undraEncoded(), "the bytes of AccountId and of its UUID")
            try checkEqual(id.undraEncoded().count, 16, "the size of an AccountId")
            try checkEqual(Cents(-5).undraEncoded(), Int64(-5).undraEncoded(), "the bytes of Cents(-5) and of an i64")
            try checkEqual(Cents(-5).undraEncoded().count, 8, "the size of Cents")
            let rate = try Decimal.undraDecoded(from: wireDecimal(-150, scale: 2))
            try checkEqual(Price(rate).undraEncoded(), rate.undraEncoded(), "the bytes of a Price and of its Decimal")
            try checkEqual(Price(rate).undraEncoded().count, 17, "the size of a Price")
            try checkEqual(try echoAccount(id, ctx: core), id, "echo_account")
            try checkEqual(try echoCents(Cents(-5), ctx: core), Cents(-5), "echo_cents")
            try checkEqual(try echoPrice(Price(rate), ctx: core).undraEncoded(), Price(rate).undraEncoded(), "echo_price (scale included)")
            // A distinct type that wraps its inner value, ordered where the inner type is.
            try check(type(of: id) == AccountId.self && id.rawValue == uuid, "AccountId is a type of its own holding the UUID")
            try check(Cents(1) < Cents(2) && !(Cents(2) < Cents(1)) && Cents(-1) < Cents(0), "Cents is Comparable")
            try check(AccountId(rawValue: uuid) == id, "AccountId is a RawRepresentable")

            // 2. A newtype is a map key and the key of a keyed list.
            let ada = try openAccount(owner: "Ada", ctx: core)
            let bob = try openAccount(owner: "Bob", ctx: core)
            try check(ada != bob, "two accounts have two identities")
            let all = try balances(ctx: core)
            try checkEqual(Set(all.keys), Set([ada, bob]), "the keys of balances()")
            try check(all.values.allSatisfy { $0.undraEncoded() == wireDecimal(0, scale: 0) }, "both balances are 0")
            try checkEqual(try ownerOf(ada, ctx: core), "Ada", "owner_of(ada)")

            let ledgerType = UndraIds.Objects.Ledger.self
            let raw = try RawStore(core: core, type: ledgerType.typeId, method: ledgerType.new)
            defer { raw.close() }
            raw.observe()
            let initial = raw.entries(of: 0)
            try checkEqual(initial.count, 1, "initial entries for accounts")
            try checkEqual(initial[0].op, .fullValue, "form of the initial accounts entry")
            var mirrored = try initial[0].decode([Account].self)
            try checkEqual(mirrored, [], "no accounts yet")
            var model: [Account] = []

            /// The next change-set of `accounts`: one keyed patch of exactly one operation, applied to the mirror and compared, in
            /// full, with the model.
            @MainActor func expectPatch(_ what: String, _ expected: PatchOp<Account>, then mutate: (inout [Account]) -> Void) async throws -> RawStore.Entry {
                try await waitUntil("the change-set of \(what)") { !raw.entries(of: 0).isEmpty }
                let entries = raw.entries(of: 0)
                try checkEqual(entries.count, 1, "accounts entries after \(what)")
                try checkEqual(entries[0].op, .keyedPatch, "form of the accounts entry after \(what)")
                let ops: [PatchOp<Account>] = try entries[0].decodePatch(Account.self)
                try checkEqual(ops, [expected], "operations after \(what)")
                try applyPatch(ops, to: &mirrored)
                mutate(&model)
                try check(mirrored == model, "the mirror diverged from the model after \(what)")
                raw.clear()
                return entries[0]
            }

            raw.clear()
            let first = try decoded(AccountId.self, try raw.callSync(ledgerType.`open`, encoded { (w: inout UndraWriter) in w.writeString("Ada") }))
            let firstAccount = Account(id: first, owner: "Ada")
            let insert = try await expectPatch("open(\"Ada\")", .insert(index: 0, item: firstAccount)) { $0.append(firstAccount) }
            try check(insert.value.count < 64, "the insert patch is \(insert.value.count) bytes (expected < 64)")
            let second = try decoded(AccountId.self, try raw.callSync(ledgerType.`open`, encoded { (w: inout UndraWriter) in w.writeString("Bob") }))
            let secondAccount = Account(id: second, owner: "Bob")
            _ = try await expectPatch("open(\"Bob\")", .insert(index: 1, item: secondAccount)) { $0.append(secondAccount) }
            try raw.callSync(ledgerType.rename, encoded { (w: inout UndraWriter) in
                first.undraEncode(&w)
                w.writeString("Ada L.")
            })
            let renamed = Account(id: first, owner: "Ada L.")
            _ = try await expectPatch("rename", .update(index: 0, item: renamed)) { $0[0] = renamed }
            try raw.callSync(ledgerType.closeAccount, encoded { (w: inout UndraWriter) in second.undraEncode(&w) })
            _ = try await expectPatch("close_account", .remove(index: 1)) { $0.remove(at: 1) }

            // The same through the generated store.
            let ledger = try Ledger(ctx: core)
            defer { ledger.close() }
            try checkEqual(ledger.accounts, [], "the generated store starts empty")
            let grace = try ledger.open(owner: "Grace")
            let linus = try ledger.open(owner: "Linus")
            try checkEqual(ledger.accounts, [Account(id: grace, owner: "Grace"), Account(id: linus, owner: "Linus")], "the generated store's accounts")
            ledger.rename(id: grace, owner: "Grace H.")
            try checkEqual(ledger.accounts.first, Account(id: grace, owner: "Grace H."), "the generated store after rename")
            ledger.closeAccount(grace)
            try checkEqual(ledger.accounts, [Account(id: linus, owner: "Linus")], "the generated store after close_account")

            // 3. Generic instantiations are plain records and enums.
            let day = Date(timeIntervalSince1970: 1_790_856_000)
            let fifteen = wireDecimal(150, scale: 2)
            for index in 0 ..< 5 {
                _ = try deposit(account: ada, amount: try amount(fifteen), memo: "#\(index)", at: day, ctx: core)
            }
            let firstWindow = try statement(account: ada, offset: 0, limit: 2, ctx: core)
            try checkEqual(firstWindow.items.count, 2, "items of statement(a, 0, 2)")
            try checkEqual(firstWindow.next, 2, "next of statement(a, 0, 2)")
            try checkEqual(firstWindow.items.map(\.memo), ["#0", "#1"], "the memos of the first window")
            try checkEqual(firstWindow.items[0].account, ada, "the account of an entry")
            try checkEqual(firstWindow.items[0].amount.undraEncoded(), fifteen, "the amount of an entry keeps its scale")
            try checkEqual(firstWindow.items[0].at, day, "the instant of an entry")
            let lastWindow = try statement(account: ada, offset: 4, limit: 10, ctx: core)
            try checkEqual(lastWindow.items.count, 1, "items of statement(a, 4, 10)")
            try checkEqual(lastWindow.next, nil, "next of statement(a, 4, 10)")
            try checkEqual(try loadableStatement(account: ada, ready: false, limit: 3, ctx: core), .loading, "loadable_statement(a, false, 3)")
            guard case .loaded(let loaded) = try loadableStatement(account: ada, ready: true, limit: 3, ctx: core) else {
                throw ScenarioFailure(description: "loadable_statement(a, true, 3) is not .loaded")
            }
            try checkEqual(loaded.items.count, 3, "items of the loaded statement")
            try checkEqual(loaded.next, 3, "next of the loaded statement")
            let stranger = AccountId(UUID(uuid: (9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9)))
            try checkEqual(try loadableStatement(account: stranger, ready: true, limit: 3, ctx: core), .failed("no such account"), "an unknown account")

            // 4. Decimals are exact.
            let tenth = try amount(wireDecimal(1, scale: 1))
            let fifth = try amount(wireDecimal(2, scale: 1))
            _ = try deposit(account: bob, amount: tenth, memo: "a", at: day, ctx: core)
            let balance = try deposit(account: bob, amount: fifth, memo: "b", at: day, ctx: core)
            try checkEqual("\(balance.rawValue)", "0.3", "0.1 + 0.2")
            try checkEqual(balance.undraEncoded(), wireDecimal(3, scale: 1), "the balance is 3 with a scale of 1")
            let cases: [(String, [UInt8])] = [
                ("0", wireDecimal(0, scale: 0)),
                ("1.10", wireDecimal(110, scale: 2)),
                ("-1.50", wireDecimal(-150, scale: 2)),
                ("a scale of 38", wireDecimal(1, scale: 38)),
                ("the largest mantissa at a scale of 38", wireDecimal(raw: largestMantissa, scale: 38)),
                ("the smallest mantissa", wireDecimal(raw: smallestMantissa, scale: 0)),
                ("the largest mantissa", wireDecimal(raw: largestMantissa, scale: 0)),
            ]
            for (name, sent) in cases {
                let value = try Decimal.undraDecoded(from: sent)
                try checkEqual(try echoDecimal(value: value, ctx: core).undraEncoded(), sent, "echo_decimal of \(name): mantissa and scale")
            }
            // A scale of 39 is refused by the core as a bad request, never decoded.
            let tooFine = wireDecimal(0, scale: 39)
            do {
                _ = try core.callSync(.freeFunction(methodId: UndraIds.Functions.echoDecimal), method: UndraIds.Functions.echoDecimal, args: tooFine)
                throw ScenarioFailure(description: "echo_decimal accepted a decimal with a scale of 39")
            } catch let refused as UndraReplyError {
                try checkEqual(refused.status, .badRequest, "the reply to a scale of 39")
            }
            // An amount `rust_decimal` cannot hold.
            do {
                _ = try deposit(account: ada, amount: try amount(wireDecimal(raw: largestMantissa, scale: 0)), memo: "huge", at: day, ctx: core)
                throw ScenarioFailure(description: "deposit of the largest 128-bit mantissa succeeded")
            } catch let failure as LedgerError {
                try checkEqual(failure, .outOfRange, "deposit of an amount rust_decimal cannot hold")
            }
            try checkEqual(try statement(account: ada, offset: 0, limit: 10, ctx: core).items.count, 5, "the refused deposit left no entry")

            // 5. Leaf types of other crates cross as wire types.
            let receipt = try sampleReceipt(ctx: core)
            try checkEqual(receipt.id, UUID(uuidString: "1ed9e400-0000-0000-0000-000000000007")!, "the receipt's id")
            try checkEqual(receipt.issued.undraEncoded(), Int64(1_790_856_000_123).undraEncoded(), "the receipt's instant is 2026-10-01T12:00:00.123Z")
            #if UNDRA_FLOOR
            let ninety = UndraDuration(nanoseconds: 90_000_000_000)
            #else
            let ninety: Duration = .seconds(90)
            #endif
            try check(receipt.validFor == ninety, "the receipt is valid for 90 s: \(receipt.validFor)")
            try checkEqual(receipt.total.undraEncoded(), wireDecimal(19_990, scale: 3), "the receipt's total is 19.990 (scale 3)")
            try checkEqual(receipt.signature, [1, 2, 3, 255], "the receipt's signature")
            try checkEqual(try echoReceipt(receipt, ctx: core), receipt, "echo_receipt of the core's receipt")
            let mine = Receipt(
                id: UUID(uuidString: "9d8c7b6a-5f4e-3d2c-1b0a-ffeeddccbbaa")!,
                issued: Date(timeIntervalSince1970: 1_700_000_000.123),
                validFor: ninety,
                total: try Decimal.undraDecoded(from: wireDecimal(-1_234_500, scale: 4)),
                signature: [0, 255, 7]
            )
            let back = try echoReceipt(mine, ctx: core)
            try checkEqual(back, mine, "echo_receipt of a receipt the platform builds")
            try checkEqual(back.total.undraEncoded(), wireDecimal(-1_234_500, scale: 4), "its total keeps its scale")
        }
    }
}
