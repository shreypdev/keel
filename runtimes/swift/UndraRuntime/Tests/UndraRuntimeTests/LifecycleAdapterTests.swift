import Foundation
import XCTest
@testable import UndraRuntime
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// `LifecycleAdapter` (ADR-046, decision 3.3 and PO-9): the app's phase reaches the core by default, from
/// the platform's application notifications, once per change, and without getting in the way of an app
/// that reports its own.
@MainActor
final class LifecycleAdapterTests: XCTestCase {
    private let didBecomeActive = Notification.Name("test.didBecomeActive")
    private let willResignActive = Notification.Name("test.willResignActive")
    private let didEnterBackground = Notification.Name("test.didEnterBackground")
    private let willEnterForeground = Notification.Name("test.willEnterForeground")

    private func adapter(_ center: NotificationCenter) -> LifecycleAdapter {
        return LifecycleAdapter(center: center, notifications: [
            (didBecomeActive, .active),
            (willResignActive, .inactive),
            (willEnterForeground, .inactive),
            (didEnterBackground, .background),
        ])
    }

    /// The `Lifecycle.changed` events the transport saw, as states.
    private func reported(_ transport: FakeTransport) -> [UndraAppState] {
        var states: [UndraAppState] = []
        for item in transport.sent {
            if case .event(let port, let method, let payload) = item,
               port == StandardPorts.Lifecycle.portId, method == StandardPorts.Lifecycle.changed,
               let state = try? UndraAppState.undraDecoded(from: payload) {
                states.append(state)
            }
        }
        return states
    }

    func testTheAdapterIsTheLifecyclePortAndHasNoMethodsTheCoreCalls() {
        let adapter = LifecycleAdapter()
        XCTAssertEqual(adapter.portId, StandardPorts.Lifecycle.portId)
        XCTAssertEqual(adapter.portId, fnv1a32("port.Lifecycle"))
        XCTAssertNil(adapter.makePortImpl(core: UndraCore.unloaded))
    }

    func testThePlatformsNotificationsCoverTheThreePhases() {
        let states = LifecycleAdapter.platformNotifications.map { $0.state }
        XCTAssertTrue(states.contains(.active))
        XCTAssertTrue(states.contains(.inactive))
        XCTAssertTrue(states.contains(.background))
        XCTAssertEqual(Set(LifecycleAdapter.platformNotifications.map { $0.name.rawValue }).count, LifecycleAdapter.platformNotifications.count)
        #if canImport(UIKit)
        let byName = Dictionary(uniqueKeysWithValues: LifecycleAdapter.platformNotifications.map { ($0.name, $0.state) })
        XCTAssertEqual(byName[UIApplication.didBecomeActiveNotification], .active)
        XCTAssertEqual(byName[UIApplication.willResignActiveNotification], .inactive)
        XCTAssertEqual(byName[UIApplication.didEnterBackgroundNotification], .background)
        #elseif canImport(AppKit)
        let byName = Dictionary(uniqueKeysWithValues: LifecycleAdapter.platformNotifications.map { ($0.name, $0.state) })
        XCTAssertEqual(byName[NSApplication.didBecomeActiveNotification], .active)
        XCTAssertEqual(byName[NSApplication.didResignActiveNotification], .inactive)
        XCTAssertEqual(byName[NSApplication.didHideNotification], .background)
        #endif
    }

    func testTheNotificationsOfTheAppAreReportedToTheCore() throws {
        let center = NotificationCenter()
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([adapter(center)]))
        center.post(name: willResignActive, object: nil)
        center.post(name: didEnterBackground, object: nil)
        center.post(name: willEnterForeground, object: nil)
        center.post(name: didBecomeActive, object: nil)
        XCTAssertEqual(reported(transport), [.inactive, .background, .inactive, .active])
        core.shutdown()
    }

    func testAStateTheCoreWasAlreadyToldIsNotReportedAgain() throws {
        let center = NotificationCenter()
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([adapter(center)]))
        center.post(name: didBecomeActive, object: nil)
        center.post(name: didBecomeActive, object: nil)
        center.post(name: willResignActive, object: nil)
        center.post(name: willResignActive, object: nil)
        // `inactive` then `inactive` again on the way back to the foreground is one change.
        center.post(name: didEnterBackground, object: nil)
        center.post(name: didEnterBackground, object: nil)
        XCTAssertEqual(reported(transport), [.active, .inactive, .background])
        core.shutdown()
    }

    func testAnAppThatReportsByItselfAsWellSeesEachChangeOnce() throws {
        let center = NotificationCenter()
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([adapter(center)]))
        // The documented way before the runtime reported by default.
        UndraLifecycle(core: core).changed(.background)
        center.post(name: didEnterBackground, object: nil)
        XCTAssertEqual(reported(transport), [.background], "the adapter does not repeat what the app reported")
        center.post(name: didBecomeActive, object: nil)
        UndraLifecycle(core: core).changed(.active)
        XCTAssertEqual(reported(transport), [.background, .active, .active], "the app's own report always goes through")
        core.shutdown()
    }

    func testDetachingStopsTheReports() throws {
        let center = NotificationCenter()
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([adapter(center)]))
        center.post(name: didBecomeActive, object: nil)
        core.shutdown()
        center.post(name: didEnterBackground, object: nil)
        XCTAssertEqual(reported(transport), [.active])
    }

    func testAttachingAgainReplacesTheObserversNotAddsToThem() throws {
        let center = NotificationCenter()
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let lifecycle = adapter(center)
        lifecycle.attach(to: core)
        lifecycle.attach(to: core)
        center.post(name: didEnterBackground, object: nil)
        XCTAssertEqual(reported(transport), [.background])
        lifecycle.detach()
        core.shutdown()
    }

    func testANotificationAfterTheCoreIsGoneIsANoOp() throws {
        let center = NotificationCenter()
        let lifecycle = adapter(center)
        let transport = FakeTransport()
        do {
            let core = try makeCore(transport)
            lifecycle.attach(to: core)
            core.shutdown()
        }
        // The observer holds the core weakly and a shut-down core says nothing: no event, no crash.
        center.post(name: didEnterBackground, object: nil)
        XCTAssertEqual(reported(transport), [])
        lifecycle.detach()
    }

    func testEventsAnAdapterOfTheAppsOwnSendReachTheRuntimesListeners() throws {
        // `UndraBackground` listens to what the core is told, whoever tells it (the testkit's ScriptedLifecycle sends
        // `core.event` directly, an app may too).
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let heard = Guarded<[UndraAppState]>([])
        let token = UndraCore.addLifecycleObserver { (told: UndraCore, state: UndraAppState) -> Void in
            if told === core {
                heard.withLock { $0.append(state) }
            }
        }
        defer { UndraCore.removeLifecycleObserver(token) }
        core.event(port: StandardPorts.Lifecycle.portId, method: StandardPorts.Lifecycle.changed, payload: UndraAppState.background.undraEncoded())
        core.event(port: StandardPorts.Lifecycle.portId, method: StandardPorts.Lifecycle.changed, payload: [9, 9])
        core.event(port: StandardPorts.Connectivity.portId, method: StandardPorts.Connectivity.changed, payload: [1, 0, 0])
        XCTAssertEqual(heard.withLock { $0 }, [.background], "only a decodable Lifecycle.changed counts")
        core.shutdown()
        core.event(port: StandardPorts.Lifecycle.portId, method: StandardPorts.Lifecycle.changed, payload: UndraAppState.active.undraEncoded())
        XCTAssertEqual(heard.withLock { $0 }, [.background], "a shut-down core says nothing")
    }
}
