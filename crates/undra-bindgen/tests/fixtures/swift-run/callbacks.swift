// Execution test of the `callbacks` golden case: the generated Swift is built with the real runtime and this
// file is its `main.swift`. It exits non-zero and prints the failed checks when a weak wrapper stops forwarding
// (objects-followups O8): a weak wrapper whose target is gone fails with the error type's own `Unavailable`
// instead of never returning.

import Foundation
import GoldenCallbacks
import UndraRuntime

nonisolated(unsafe) var failures: [String] = []

func check(_ ok: Bool, _ what: String) {
    if !ok { failures.append(what) }
}

@MainActor
final class Recorder: UploadListener {
    var heard: [String] = []

    func progress(sent: UInt64, total: UInt64) {
        heard.append("progress \(sent)/\(total)")
    }

    func finished(name: String) {
        heard.append("finished \(name)")
    }

    func confirmReplace(name: String) async throws(PromptError) -> Bool {
        heard.append("confirm \(name)")
        return true
    }
}

@MainActor
func run() async {
    var target: Recorder? = Recorder()
    let weak = WeakUploadListener(target!)
    weak.finished(name: "a")
    do {
        let answer = try await weak.confirmReplace(name: "b")
        check(answer, "the weak wrapper forwards the question while its target lives")
    } catch {
        failures.append("a live target's question threw \(error)")
    }
    check(target?.heard == ["finished a", "confirm b"], "the target heard \(String(describing: target?.heard))")

    target = nil
    weak.finished(name: "c")
    check(weak.target == nil, "the weak wrapper does not keep its target alive")
    do {
        _ = try await weak.confirmReplace(name: "d")
        failures.append("a question to a gone target returned")
    } catch PromptError.unavailable(let reason) {
        check(reason.contains("WeakUploadListener") && reason.contains("gone"), "the reason says what happened: \(reason)")
    } catch {
        failures.append("a question to a gone target threw \(error), not PromptError.unavailable")
    }
}

await run()
if failures.isEmpty {
    print("callbacks: all checks passed")
} else {
    for failure in failures { print("FAILED: \(failure)") }
    exit(1)
}
