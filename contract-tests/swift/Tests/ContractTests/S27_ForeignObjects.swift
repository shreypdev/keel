import Foundation
import PlaygroundA
import PlaygroundB
import UndraRuntime
import XCTest

extension ContractScenarios {
    /// S27 step 8, in a file of its own because it uses the two packages of S26, whose names are the
    /// playground's: package A's generated classes on core A and on core B, so that only the runtime
    /// can tell the two shelves apart.
    func s27ForeignObjects() throws {
        let reports = Locked<[UndraUnhandledError]>([])
        let coreA = try UndraPlaygroundA.load(.inproc(
            adapters: Fixture.shared.makeAdapters(),
            onError: { report in reports.withLock { $0.append(report) } }
        ))
        let coreB = try UndraPlaygroundB.load(.inproc(adapters: Fixture.shared.makeAdapters()))
        defer {
            coreA.shutdown()
            coreB.shutdown()
        }
        let workshopA = try PlaygroundA.Workshop(ctx: coreA)
        let workshopB = try PlaygroundA.Workshop(ctx: coreB)
        let shelfOfA = try workshopA.shelf(name: "x")
        let shelfOfB = try workshopB.shelf(name: "x")
        defer {
            shelfOfA.close()
            shelfOfB.close()
            workshopA.close()
            workshopB.close()
        }
        try checkEqual(shelfOfA.handle, shelfOfB.handle, "two cores with the same history issue the same handle numbers")
        let calls = (coreA.stat("crossings.calls"), coreB.stat("crossings.calls"))
        let refs = (coreA.stats().hostRefs, coreB.stats().hostRefs)
        workshopA.merge(from: shelfOfB, onto: shelfOfA)
        let reported = try require(reports.snapshot.last, "the report of merge with a shelf of another core")
        try checkEqual(reported.operation, "Workshop.merge", "the operation reported")
        guard case .refused(let reason) = reported.error, reason.contains("Shelf"), reason.contains("another") else {
            throw ScenarioFailure(description: "merge with a foreign shelf reported \(reported.error)")
        }
        let thrown = try callError("total with a foreign shelf") { try workshopA.total(shelves: [shelfOfA, shelfOfB]) }
        guard case .refused = thrown else {
            throw ScenarioFailure(description: "total with a foreign shelf threw \(thrown), not .refused")
        }
        try checkEqual(coreA.stat("crossings.calls"), calls.0, "calls on core A: nothing was sent")
        try checkEqual(coreB.stat("crossings.calls"), calls.1, "calls on core B")
        try checkEqual(coreA.stats().hostRefs, refs.0, "host_refs of core A")
        try checkEqual(coreB.stats().hostRefs, refs.1, "host_refs of core B")
    }
}
