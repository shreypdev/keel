// Diagnostics: the standard sync port the core hands every contained panic to (docs/SPEC.md section 8,
// ADR-046). The default adapter decodes the report and forwards it to `LoadOptions.onPanic`.

import Dispatch
import Foundation

/// `Diagnostics`: receives one structured ``UndraPanicReport`` per panic the core contained and hands it
/// to ``LoadOptions/onPanic``, once and in order, on the main thread.
///
/// Every core loaded through ``UndraCore/load(_:)`` has it, whatever adapters the app passes (an app
/// that registers its own `Diagnostics` implementation replaces it). The core calls the port from the
/// thread that panicked, possibly while it holds its lock, fire and forget; the adapter answers at once
/// and runs the handler later on the main queue, so the handler is free to call into Undra and the
/// app's crash reporter. Without an `onPanic` the adapter writes one error line to the runtime's log, so
/// a panic is never silent. An argument that does not decode is logged and answered "ok": a malformed
/// report never reaches the core as a failure.
public struct DiagnosticsAdapter: UndraAdapter {
    /// Creates the adapter.
    public init() {}

    public var portId: UInt32 {
        return StandardPorts.Diagnostics.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        // The handler only, not the core: the core owns this table.
        let handler = core.onPanic
        return .sync([
            StandardPorts.Diagnostics.panicked: { args in
                DiagnosticsAdapter.deliver(args, to: handler, on: DispatchQueue.main)
                return []
            },
        ])
    }

    /// Decodes the arguments of `Diagnostics.panicked(report)` and hands the report on: to `handler` on
    /// `queue` (in the order of the calls, since a serial queue runs its blocks in order), or to the log
    /// when there is no handler. Never throws and never calls `handler` on the calling thread.
    static func deliver(
        _ args: [UInt8],
        to handler: (@Sendable (UndraPanicReport) -> Void)?,
        on queue: DispatchQueue
    ) {
        let report: UndraPanicReport
        do {
            report = try UndraPanicReport.undraDecoded(from: args)
        } catch {
            UndraLog.error(
                "Diagnostics.panicked received an argument that does not decode as a PanicReport (\(args.count) bytes: \(error)); the report was dropped"
            )
            return
        }
        guard let handler = handler else {
            UndraLog.error(DiagnosticsAdapter.line(for: report))
            return
        }
        queue.async {
            handler(report)
        }
    }

    /// The one line the runtime logs for a panic nobody asked to hear about: the operation, the message
    /// and the location, on a single line.
    static func line(for report: UndraPanicReport) -> String {
        let message = report.message.replacingOccurrences(of: "\n", with: " ")
        return "the Undra core panicked in \(report.operation): \(message) (at \(report.location)); set LoadOptions.onPanic to hand panics to a crash reporter"
    }
}
