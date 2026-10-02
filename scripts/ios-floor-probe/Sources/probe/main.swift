import Foundation
import Observation
import UndraRuntime

/// A flag a tracker's `onChange` (which is `@Sendable`) can set.
final class Flag: @unchecked Sendable {
    var value = false
}

@MainActor
func probe() async -> Bool {
    // The closed placeholder core: public, and it needs no core library.
    let core = UndraCore.shared
    var line = "os=\(ProcessInfo.processInfo.operatingSystemVersionString)"
    guard #available(iOS 17, macOS 14, *) else {
        print(line + " observation=unavailable (the floor path is right here)")
        return true
    }
    let first = core.connection
    let second = core.connection
    let fired = Flag()
    withObservationTracking({ _ = first.state }, onChange: { fired.value = true })
    // The state the placeholder reports reaches both observables on the main queue.
    try? await Task.sleep(nanoseconds: 300_000_000)
    let agree = "\(first.state)" == "\(core.connectionObject.state)"
    line += " observation=available same-object=\(first === second) tracker-fired=\(fired.value) agree-with-connectionObject=\(agree) state=\(first.state)"
    print(line)
    return first === second && fired.value && agree
}

exit(await probe() ? 0 : 1)
