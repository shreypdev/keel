import os

/// The runtime's own diagnostics: problems it can survive (a malformed change-set, a reply for a
/// call nobody waits for) but that indicate a bug or version skew someone should hear about.
/// Written to the unified log under the subsystem `dev.keel.runtime`.
enum KeelLog {
    private static let logger = Logger(subsystem: "dev.keel.runtime", category: "runtime")

    static func warning(_ message: String) {
        logger.warning("\(message, privacy: .public)")
    }

    static func error(_ message: String) {
        logger.error("\(message, privacy: .public)")
    }

    /// Forwards a log record the core produced (remote transport `Log` messages when no `Log`
    /// port implementation is registered). `level` uses the Log port numbering: 0 trace,
    /// 1 debug, 2 info, 3 warn, 4 error, 5 fatal.
    static func forward(level: UInt8, target: String, message: String) {
        let category = Logger(subsystem: "dev.keel.core", category: target)
        let type = osLogType(forKeelLevel: level)
        category.log(level: type, "\(message, privacy: .public)")
    }

    /// Maps the Log port's level numbering onto `OSLogType`.
    static func osLogType(forKeelLevel level: UInt8) -> OSLogType {
        switch level {
        case 0, 1:
            return .debug
        case 2:
            return .info
        case 3:
            return .default
        case 4:
            return .error
        default:
            return .fault
        }
    }
}
