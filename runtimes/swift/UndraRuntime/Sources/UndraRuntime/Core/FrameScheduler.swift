// When the mirror drains what the core produced on its own (ADR-031 decision 2).
//
// The mirror asks for a frame when the first entry arrives after a drain, from whatever thread
// delivered it, and drains when the frame comes. On iOS, tvOS and visionOS a frame is a tick of a
// `CADisplayLink` on the main run loop, paused while nothing waits; elsewhere (macOS) it is the
// next turn of the main actor. Replies, synchronous calls and `observe` do not wait for a frame
// (see `UndraCore`). Tests replace the scheduler with one they drive by hand.

import Dispatch
import Foundation

#if os(iOS) || os(tvOS) || os(visionOS)
import QuartzCore
#endif

/// Decides when a mirror's frame drain runs: the seam between the mirror and the display.
///
/// The mirror calls `requestFrame` at most once until the tick it passed has run, so an
/// implementation needs no deduplication of its own.
protocol FrameScheduler: AnyObject, Sendable {
    /// Arranges for `tick` to run once, on the main actor, at the next frame. Called from any
    /// thread, never while the mirror holds its lock.
    func requestFrame(_ tick: @escaping @MainActor @Sendable () -> Void)

    /// Releases what the scheduler holds (the display link). A request made afterwards still runs
    /// its tick, on the next turn of the main actor.
    func invalidate()
}

/// The scheduler of the current platform: a display link where there is one, the main actor's
/// next turn elsewhere.
func makePlatformFrameScheduler() -> any FrameScheduler {
    #if os(iOS) || os(tvOS) || os(visionOS)
    return DisplayLinkFrameScheduler()
    #else
    return MainActorHopScheduler()
    #endif
}

/// Runs each tick on the next turn of the main actor: the scheduler of platforms without a display
/// link (macOS), where the mirror then drains everything queued while the main actor was busy.
final class MainActorHopScheduler: FrameScheduler {
    init() {}

    func requestFrame(_ tick: @escaping @MainActor @Sendable () -> Void) {
        Task { @MainActor in
            tick()
        }
    }

    func invalidate() {}
}

#if os(iOS) || os(tvOS) || os(visionOS)

/// Runs each tick from a `CADisplayLink` on the main run loop (common modes, so a scroll does not
/// hold it back), paused while no tick is wanted.
///
/// A request unpauses the link: directly when it is made on the main thread, otherwise from one
/// hop to the main actor (the mirror requests at most once per frame, never once per
/// change-set). The tick pauses the link again before it runs. The link is created on the first
/// request and invalidated by `invalidate()` or once the scheduler is gone.
final class DisplayLinkFrameScheduler: FrameScheduler, @unchecked Sendable {
    // Touched on the main thread only (the `@MainActor` methods below).
    private var link: CADisplayLink?
    private var tick: (@MainActor @Sendable () -> Void)?
    private var invalidated = false

    init() {}

    func requestFrame(_ tick: @escaping @MainActor @Sendable () -> Void) {
        if Thread.isMainThread {
            MainActor.assumeIsolated {
                self.resume(tick)
            }
            return
        }
        Task { @MainActor in
            self.resume(tick)
        }
    }

    func invalidate() {
        if Thread.isMainThread {
            MainActor.assumeIsolated {
                self.tearDown()
            }
            return
        }
        Task { @MainActor in
            self.tearDown()
        }
    }

    @MainActor
    private func resume(_ next: @escaping @MainActor @Sendable () -> Void) {
        if invalidated {
            // Never run a drain from here: a request made on the main thread may come from inside
            // a core callback (the in-process core delivers there), which must not reach the core.
            Task { @MainActor in
                next()
            }
            return
        }
        tick = next
        if link == nil {
            let target = DisplayLinkTarget(owner: self)
            let created = CADisplayLink(target: target, selector: #selector(DisplayLinkTarget.step(_:)))
            created.add(to: RunLoop.main, forMode: .common)
            link = created
        }
        link?.isPaused = false
    }

    /// The display link fired: pause it, then run the tick that was requested.
    @MainActor
    fileprivate func step() {
        link?.isPaused = true
        let due = tick
        tick = nil
        due?()
    }

    @MainActor
    private func tearDown() {
        invalidated = true
        link?.invalidate()
        link = nil
        // A tick requested before the link went away still runs, on the next turn.
        if let due = tick {
            tick = nil
            Task { @MainActor in
                due()
            }
        }
    }
}

/// The display link's target. `CADisplayLink` retains its target, so this small object stands in
/// for the scheduler and holds it weakly; a link whose scheduler is gone invalidates itself.
@MainActor
private final class DisplayLinkTarget: NSObject {
    private weak var owner: DisplayLinkFrameScheduler?

    init(owner: DisplayLinkFrameScheduler) {
        self.owner = owner
    }

    @objc func step(_ link: CADisplayLink) {
        guard let owner = owner else {
            link.invalidate()
            return
        }
        owner.step()
    }
}

#endif
