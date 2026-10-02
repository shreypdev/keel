import Foundation
import PlaygroundCore
@testable import UndraRuntime
import XCTest

/// What the runner's reporter answers when the core asks it to confirm.
enum ConfirmAnswer {
    case yes
    case no
    case fail(ReportError)
    /// Waits until its task is cancelled, then answers `true` (an answer that comes too late).
    case waitForCancel
}

/// The runner's `Reporter` (S28): records every call, in order, with the workshop as it was then.
@MainActor
final class ScenarioReporter: Reporter {
    var lines: [String] = []
    var progress: [String] = []
    /// `Workshop.notes` as the reporter saw it at each note.
    var notesSeen: [UInt32] = []
    /// What a call back into the core from inside `note` returned (`watching()`).
    var reentered: [UInt32] = []
    weak var workshop: Workshop?
    var answer: ConfirmAnswer = .yes
    var confirmsStarted = 0
    var sawCancellation = false

    func progress(done: UInt32, total: UInt32) {
        progress.append("\(done)/\(total)")
    }

    func note(line: String) {
        lines.append(line)
        notesSeen.append(workshop?.notes ?? 0)
        if line == "two", let workshop = workshop {
            // Into the core from inside the callback: it runs at the drain, not inside the core's call.
            reentered.append((try? workshop.watching()) ?? 999)
        }
    }

    func confirm(question: String) async throws(ReportError) -> Bool {
        confirmsStarted += 1
        switch answer {
        case .yes:
            return true
        case .no:
            return false
        case .fail(let error):
            throw error
        case .waitForCancel:
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(5))
            }
            sawCancellation = true
            return true
        }
    }
}

/// A reporter nothing but the core's registry holds (S28 step 8).
@MainActor
final class InlineReporter: Reporter {
    let heard: Locked<[String]>

    init(heard: Locked<[String]>) {
        self.heard = heard
    }

    func progress(done: UInt32, total: UInt32) {}

    func note(line: String) {
        heard.withLock { $0.append(line) }
    }

    func confirm(question: String) async throws(ReportError) -> Bool {
        return true
    }
}

extension ContractScenarios {
    // MARK: S27

    /// ADR-040: a store hands out child stores (`Workshop.shelf`), takes them back as arguments, and
    /// one object has one handle and one wrapper. `host_refs` is the core's count of the references
    /// the host owns.
    func testS27_objectsCross() async {
        await scenario("S27", "objects cross") {
            let core = try self.core
            let refs = { core.stats().hostRefs }
            let workshop = try Workshop(ctx: core)
            defer { workshop.close() }

            // 1. A parent returns a child, a store observed when its wrapper is made.
            let handles1 = core.stat("live_handles")
            let refs1 = refs()
            let a = try workshop.shelf(name: "a")
            try checkEqual(a.label, "a", "the shelf's label")
            try checkEqual(a.items, 0, "the shelf's items")
            try checkEqual(core.stat("live_handles") - handles1, 1, "live handles for the shelf")
            try checkEqual(refs() - refs1, 1, "host_refs for the shelf")

            // 2. One object, one handle, one wrapper.
            let again = try workshop.shelf(name: "a")
            try check(again === a, "shelf(\"a\") twice is one wrapper")
            try checkEqual(again.handle, a.handle, "and one handle")
            try checkEqual(refs() - refs1, 1, "host_refs after the duplicate was given back")
            let found = try workshop.find(name: "a")
            try check(found === a, "find(\"a\") is the same wrapper")
            let missing = try workshop.find(name: "zz")
            try check(missing == nil, "find(\"zz\") is nil")
            let listed = try workshop.shelves()
            try check(listed.count == 1 && listed[0] === a, "shelves() lists it once: \(listed.count)")
            let changeSets2 = core.mirror.stats().changeSetsReceived
            let applied2 = core.mirror.stats().entriesApplied
            a.stock(count: 2)
            try checkEqual(a.items, 2, "items after stock(2)")
            try checkEqual(core.mirror.stats().changeSetsReceived - changeSets2, 1, "change-sets for stock(2)")
            try checkEqual(core.mirror.stats().entriesApplied - applied2, 1, "entries applied: the store is mirrored once")

            // 3. A child is passed back as a parameter; parameters are borrowed.
            let b = try workshop.shelf(name: "b")
            b.stock(count: 3)
            let refs3 = refs()
            let changeSets3 = core.mirror.stats().changeSetsReceived
            let transactions3 = core.stat("transactions")
            workshop.merge(from: a, onto: b)
            try checkEqual(a.items, 0, "a after the merge")
            try checkEqual(b.items, 5, "b after the merge")
            try checkEqual(core.mirror.stats().changeSetsReceived - changeSets3, 2, "one change-set for each shelf")
            try checkEqual(core.stat("transactions") - transactions3, 1, "delivered in one transaction")
            try checkEqual(try workshop.total(shelves: [a, b]), 5, "total([a, b])")
            try checkEqual(try workshop.describe(shelf: a), "a", "describe(a)")
            try checkEqual(try workshop.describe(shelf: nil), "none", "describe(nil)")
            try checkEqual(refs(), refs3, "host_refs after the calls that borrowed shelves")

            // 4. Closing releases exactly one reference; a new wrapper after that.
            let refs4 = refs()
            let c = try workshop.shelf(name: "c")
            let cAgain = try workshop.shelf(name: "c")
            try check(cAgain === c, "shelf(\"c\") twice is one wrapper")
            try checkEqual(refs() - refs4, 1, "one reference for c")
            let closedHandle = c.handle
            c.close()
            try checkEqual(refs(), refs4, "host_refs after c closed")
            let reports4 = Fixture.shared.unhandled.snapshot.count
            c.stock(count: 1)
            let report4 = try require(Fixture.shared.unhandled.snapshot.dropFirst(reports4).first, "the report of stock on the closed shelf")
            try checkEqual(report4.operation, "Shelf.stock", "the operation reported")
            guard case .refused = report4.error else {
                throw ScenarioFailure(description: "stock on a closed shelf reported \(report4.error), not .refused")
            }
            let c2 = try workshop.shelf(name: "c")
            try check(c2 !== c && c2.handle != closedHandle, "shelf(\"c\") after close() is a new wrapper with a new handle")
            try checkEqual(c2.label, "c", "the new wrapper's label")
            c2.close()

            // 5. A call cancelled before it finishes owes nothing.
            let refs5 = refs()
            let handles5 = core.stat("live_handles")
            let active = core.stat("active_calls")
            let slow = Task { try await workshop.open(name: "slow", delayMs: 60_000) }
            try await waitUntil("open(\"slow\") to be running") { core.stat("active_calls") > active }
            slow.cancel()
            switch await slow.result {
            case .success:
                throw ScenarioFailure(description: "open(\"slow\") returned after it was cancelled")
            case .failure(let error):
                try check(error is CancellationError, "the cancelled open failed with \(error)")
            }
            try checkEqual(refs(), refs5, "host_refs after the cancelled open")
            try checkEqual(core.stat("live_handles"), handles5, "live handles after the cancelled open")
            let fast = try await workshop.open(name: "fast", delayMs: 10)
            try checkEqual(fast.label, "fast", "open(\"fast\")'s label")
            try await checkThrows({ try await workshop.open(name: "", delayMs: 0) }, WorkshopError.noName, "open(\"\")")
            try checkEqual(refs() - refs5, 1, "host_refs: only the fast shelf")
            fast.close()
            try checkEqual(refs(), refs5, "host_refs after closing it")

            // 6. A restore makes derived handles stale.
            let snapshot = try core.snapshot()
            let stored = try Wire.Snapshot.decode(snapshot).stores.map(\.handle)
            try check(stored.contains(workshop.handle), "the snapshot holds the workshop")
            try check(!stored.contains(a.handle) && !stored.contains(b.handle), "the snapshot holds no shelf (derived handles are transient)")
            try core.restore(snapshot)
            _ = try workshop.watching()
            let reports6 = Fixture.shared.unhandled.snapshot.count
            a.stock(count: 1)
            let report6 = try require(Fixture.shared.unhandled.snapshot.dropFirst(reports6).first, "the report of stock on a stale shelf")
            guard case .refused = report6.error else {
                throw ScenarioFailure(description: "stock on a shelf after the restore reported \(report6.error), not .refused")
            }
            let freshA = try workshop.shelf(name: "a")
            try check(freshA !== a, "shelf(\"a\") after the restore is a fresh wrapper")
            try checkEqual(freshA.items, 0, "the fresh shelf's items")
            a.close()
            b.close()

            // 7. Letting the last reference go releases it (deinit).
            let refs7 = refs()
            do {
                let dropped = try workshop.shelf(name: "d")
                try checkEqual(refs() - refs7, 1, "host_refs while d is held")
                try checkEqual(dropped.label, "d", "d's label")
            }
            try await waitUntil("host_refs to go back down after d was dropped") { refs() == refs7 }

            // 8. A foreign object is refused before anything is sent (the two cores of S26).
            try s27ForeignObjects()

            // 9. Statistics: host_refs, and a handle given back twice over-releases nothing.
            try check(refs() > 0, "host_refs is reported")
            let refs9 = refs()
            let raw = try core.callSync(
                .objectMethod(handle: workshop.handle, methodId: UndraIds.Objects.Workshop.shelf),
                method: UndraIds.Objects.Workshop.shelf,
                args: encoded { $0.writeString("raw") }
            )
            let rawHandle = try UndraHandle.undraDecoded(from: raw)
            try checkEqual(refs() - refs9, 1, "host_refs for the raw shelf")
            core.release(rawHandle)
            core.release(rawHandle)
            try checkEqual(refs(), refs9, "host_refs after giving the raw handle back twice")
            freshA.stock(count: 1)
            try checkEqual(freshA.items, 1, "a wrapper's own reference is untouched")
            freshA.close()
            freshA.close()
            try checkEqual(refs(), refs9 - 1, "close() twice gives back one reference")
        }
    }

    // MARK: S28

    /// ADR-041: the workshop calls the app's `Reporter` back. Main-thread delivery: through the
    /// mirror's drain, after the change-sets committed before each call.
    func testS28_hostCallbacks() async {
        await scenario("S28", "host callbacks") {
            let core = try self.core
            let registry = core.callbacks
            let baseline = registry.liveCount
            let workshop = try Workshop(ctx: core)
            defer { workshop.close() }
            let rep = ScenarioReporter()
            rep.workshop = workshop

            // 1. Ordered, after the change-sets committed before them; never inside the core's call.
            let delivered = core.mirror.stats().callbacksDelivered
            let watch = try workshop.watch(reporter: rep)
            _ = try workshop.announce(line: "one")
            _ = try workshop.announce(line: "two")
            try await waitUntil("the two notes") { rep.lines.count == 2 }
            try checkEqual(rep.lines, ["one", "two"], "the notes, in order")
            try checkEqual(rep.notesSeen, [1, 2], "Workshop.notes as each note saw it")
            try checkEqual(rep.reentered, [1], "a call into the core from inside note")
            try check(core.mirror.stats().callbacksDelivered - delivered >= 2, "the drain delivered the notes")

            // 2. An async callback returns a value, or throws its typed error.
            rep.lines = []
            rep.progress = []
            rep.answer = .yes
            let ran = try await workshop.run(steps: 3, reporter: rep)
            try checkEqual(ran, 3, "run(3) when confirm says yes")
            try checkEqual(rep.lines, ["step 1 of 3", "step 2 of 3", "step 3 of 3"], "the notes of run(3)")
            try checkEqual(rep.progress.last, "3/3", "the newest progress report")
            try check(rep.progress == rep.progress.sorted() && rep.progress.allSatisfy({ ["1/3", "2/3", "3/3"].contains($0) }), "progress in order: \(rep.progress)")
            rep.answer = .no
            try await checkThrows({ try await workshop.run(steps: 1, reporter: rep) }, ReportError.declined, "run when confirm says no")
            rep.answer = .fail(.unavailable("x"))
            try await checkThrows({ try await workshop.run(steps: 1, reporter: rep) }, ReportError.unavailable("x"), "run when confirm throws its own error")

            // 3. Another throw is reported: with typed throws (the default) a Swift `confirm` can only
            //    throw a ReportError and `note` cannot throw, so the type system rules the step out;
            //    the runtime's own tests cover the mapping (CallbackTests).
            ScenarioCase.report("NOTE S28 step 3: not representable in Swift with typed throws; covered by UndraRuntimeTests.CallbackTests")

            // 4. Cancellation reaches the host task; the late answer is discarded.
            rep.answer = .waitForCancel
            let started = rep.confirmsStarted
            let running = Task { try await workshop.run(steps: 1, reporter: rep) }
            try await waitUntil("confirm to start") { rep.confirmsStarted > started }
            running.cancel()
            switch await running.result {
            case .success(let value):
                throw ScenarioFailure(description: "the cancelled run returned \(value)")
            case .failure(let error):
                try check(error is CancellationError, "the cancelled run failed with \(error)")
            }
            try await waitUntil("confirm to see its cancellation", timeout: .seconds(1)) { rep.sawCancellation }
            try await waitUntil("the run's reference to come back") { registry.count(of: rep) == 1 }

            // 5. Interning, and an empty registry once the core lets go.
            watch.close()
            try await waitUntil("the registry to let go of the first subscription's reporter") { registry.count(of: rep) == 0 }
            weak var gone: ScenarioReporter?
            do {
                let twice = ScenarioReporter()
                gone = twice
                let first = registry.lend(twice)
                let second = registry.lend(twice)
                try checkEqual(first, second, "lend returns one instance handle for one reporter")
                registry.giveBack(first)
                registry.giveBack(second)
                let watch1 = try workshop.watch(reporter: twice)
                let watch2 = try workshop.watch(reporter: twice)
                try checkEqual(registry.count(of: twice), 1, "one reference after the core gave the duplicate back")
                try checkEqual(try workshop.watching(), 2, "two subscriptions, one proxy")
                watch1.close()
                watch2.close()
                try await waitUntil("the registry to let the reporter go", timeout: .seconds(1)) { registry.count(of: twice) == 0 }
                try checkEqual(registry.liveCount, baseline, "the registry is back where it started")
            }
            try check(gone == nil, "the reporter is gone once nothing holds it")

            // 6. `coalesce` delivers only the newest per drain; every note arrives.
            rep.lines = []
            rep.progress = []
            workshop.burst(steps: 50, reporter: rep)
            try await waitUntil("the burst's notes") { rep.lines.count == 50 }
            try checkEqual(rep.progress, ["50/50"], "progress delivered once, the newest")
            try checkEqual(rep.lines, (1 ... 50).map { "burst \($0)" }, "every note, in order")
            try await waitUntil("the burst's reference to come back") { registry.count(of: rep) == 0 }

            // 7. A refused call leaves the registry unchanged.
            let stale = try Workshop(ctx: core)
            stale.close()
            let rep2 = ScenarioReporter()
            let refused = try callError("watch on a closed workshop") { try stale.watch(reporter: rep2) }
            guard case .refused = refused else {
                throw ScenarioFailure(description: "watch on a closed workshop failed with \(refused), not .refused")
            }
            try checkEqual(registry.count(of: rep2), 0, "the refused reporter is not in the registry")
            try checkEqual(registry.liveCount, baseline, "nothing is held for the refused call")

            // 8. Strong by default; the weak wrapper forwards while its target lives.
            let heard = Locked<[String]>([])
            weak var inlineGone: InlineReporter?
            let inlineWatch: Watch
            do {
                let inline = InlineReporter(heard: heard)
                inlineGone = inline
                inlineWatch = try workshop.watch(reporter: inline)
            }
            try check(inlineGone != nil, "the registry holds an inline reporter")
            _ = try workshop.announce(line: "inline")
            try await waitUntil("the inline reporter's note") { heard.snapshot == ["inline"] }
            inlineWatch.close()
            try await waitUntil("the inline reporter to go") { inlineGone == nil }
            var target: ScenarioReporter? = ScenarioReporter()
            let weakReporter = WeakReporter(try require(target, "the target"))
            let ranWeakly = try await workshop.run(steps: 1, reporter: weakReporter)
            try checkEqual(ranWeakly, 1, "run through the weak wrapper while its target lives")
            try checkEqual(target?.lines, ["step 1 of 1"], "the target heard the note")
            target = nil
            do {
                _ = try await workshop.run(steps: 1, reporter: weakReporter)
                throw ScenarioFailure(description: "run through a weak wrapper whose target is gone returned")
            } catch ReportError.unavailable {
                // Answered unavailable for the gone target.
            }
            try await waitUntil("the weak wrapper's references to come back") { registry.liveCount == baseline }

            // 9. A background interface is not delivered through the drain: the playground has none
            //    (the golden case `callbacks` and CallbackTests cover it).
        }
    }
}
