// Lifecycle: the app's phase, reported to the core by default (docs/SPEC.md section 8, ADR-046).
//
//   iOS, tvOS, visionOS   UIApplication notifications
//   macOS                 NSApplication notifications
//
// The state goes to the core through `UndraCore.event` like any app-reported one, so it also reaches the
// runtime's own listeners (background scheduling, `UndraBackground`).

import Foundation
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// `Lifecycle`: reports the app's phase to the core (`Active`, `Inactive`, `Background`) from the
/// platform's application notifications, so the core refetches on `Active` and flushes persistence on
/// `Background` without the app writing a line (ADR-046).
///
/// It is part of ``Adapters/platformDefault``. An app that reports its own lifecycle keeps working: a
/// state equal to the one last reported is not reported again, so reporting by hand (``UndraLifecycle``)
/// next to this adapter makes the core see each change once. To turn it off, load the core with
/// `Adapters.platformDefault.removing(portId: fnv1a32("port.Lifecycle"))`; to report differently, replace it
/// with an adapter of your own for the same port.
///
/// | Notification (iOS) | Reported |
/// |---|---|
/// | `didBecomeActive` | `.active` |
/// | `willResignActive`, `willEnterForeground` | `.inactive` |
/// | `didEnterBackground` | `.background` |
///
/// On macOS: `didBecomeActive` is `.active`, `didResignActive` and `didUnhide` are `.inactive`, `didHide`
/// is `.background`. The adapter reads nothing about the app's phase at attach time: the core starts
/// believing the app is active, and the first notification corrects it.
public final class LifecycleAdapter: UndraAdapter, @unchecked Sendable {
    /// The notifications and the state each one means.
    static var platformNotifications: [(name: Notification.Name, state: UndraAppState)] {
        #if canImport(UIKit)
        return [
            (UIApplication.didBecomeActiveNotification, .active),
            (UIApplication.willResignActiveNotification, .inactive),
            (UIApplication.willEnterForegroundNotification, .inactive),
            (UIApplication.didEnterBackgroundNotification, .background),
        ]
        #elseif canImport(AppKit)
        return [
            (NSApplication.didBecomeActiveNotification, .active),
            (NSApplication.didResignActiveNotification, .inactive),
            (NSApplication.didUnhideNotification, .inactive),
            (NSApplication.didHideNotification, .background),
        ]
        #else
        return []
        #endif
    }

    private let center: NotificationCenter
    private let notifications: [(name: Notification.Name, state: UndraAppState)]
    private let observers = Guarded<[any NSObjectProtocol]>([])

    /// Creates the adapter: it listens to the application notifications of this platform.
    public convenience init() {
        self.init(center: .default, notifications: LifecycleAdapter.platformNotifications)
    }

    /// Creates an adapter that listens to `notifications` on `center`. Tests post their own.
    init(center: NotificationCenter, notifications: [(name: Notification.Name, state: UndraAppState)]) {
        self.center = center
        self.notifications = notifications
    }

    public var portId: UInt32 {
        return StandardPorts.Lifecycle.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return nil
    }

    public func attach(to core: UndraCore) {
        var added: [any NSObjectProtocol] = []
        for entry in notifications {
            let reported = entry.state
            let token = center.addObserver(forName: entry.name, object: nil, queue: nil) { [weak core] _ in
                core?.reportLifecycleIfChanged(reported)
            }
            added.append(token)
        }
        let previous = observers.withLock { (current: inout [any NSObjectProtocol]) -> [any NSObjectProtocol] in
            let old = current
            current = added
            return old
        }
        for token in previous {
            center.removeObserver(token)
        }
    }

    public func detach() {
        let existing = observers.withLock { (current: inout [any NSObjectProtocol]) -> [any NSObjectProtocol] in
            let old = current
            current = []
            return old
        }
        for token in existing {
            center.removeObserver(token)
        }
    }
}
