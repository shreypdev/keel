// Runtime statistics: the core's `undra_stats_json` document plus the host's own counters.

/// The core's background-run counters (`"background"` in its stats document, ADR-046): what
/// ``UndraCore/runInBackground(deadline:)`` has to work with and what it did. All zero for a core that
/// predates them.
public struct UndraBackgroundStats: Sendable, Equatable {
    /// Background tasks registered in the core (the query runtime registers three).
    public var tasks: Int
    /// Items of work a background window would drain: queued offline mutations, stale persisted queries,
    /// unflushed persistence. The platform reads it after reporting `Lifecycle.Background` to decide
    /// whether to ask the OS for a window.
    public var pending: Int
    /// Background runs started.
    public var runs: Int
    /// Runs that finished all their work.
    public var finished: Int
    /// Offline mutations replayed by background runs.
    public var replayed: Int
    /// Queries refetched by background runs.
    public var refetched: Int

    /// Counters that are all zero.
    public init() {
        self.init(tasks: 0, pending: 0, runs: 0, finished: 0, replayed: 0, refetched: 0)
    }

    /// Creates the counters.
    public init(tasks: Int, pending: Int, runs: Int, finished: Int, replayed: Int, refetched: Int) {
        self.tasks = tasks
        self.pending = pending
        self.runs = runs
        self.finished = finished
        self.replayed = replayed
        self.refetched = refetched
    }
}

/// A snapshot of the runtime's counters (`UndraCore.stats()`).
///
/// The `core` fields come from the core's `undra_stats_json` document and are zero when the
/// transport cannot ask for it (the remote transport); the `host` fields are counted by this
/// runtime and are always available. A store or object that was created and not closed shows
/// up in both `coreLiveHandles` and `hostLiveHandles`; a difference means a handle leaked on one
/// side.
public struct UndraStats: Sendable, Equatable {
    /// The core's stats document, verbatim (empty when the transport cannot provide it).
    public var json: String
    /// Every integer of the document, flattened with dots: `"live_handles"`, `"crossings.calls"`.
    public var values: [String: Int]

    /// Handles alive in the core's object table.
    public var coreLiveHandles: Int
    /// Stores alive in the core.
    public var coreLiveStores: Int
    /// Tasks alive in the core's executor.
    public var coreTasks: Int
    /// Calls in flight in the core.
    public var coreActiveCalls: Int
    /// Streams open in the core.
    public var coreOpenStreams: Int
    /// Transactions committed by the core.
    public var coreTransactions: Int
    /// Panics the core caught.
    public var corePanics: Int
    /// References to the core's objects the host owns, summed over its object table (`host_refs`,
    /// ADR-040): one per live wrapper. Zero when the document does not report it.
    public var hostRefs: Int
    /// Panic reports the core handed to the `Diagnostics` port (ADR-046): one per contained panic, the
    /// ones ``LoadOptions/onPanic`` receives.
    public var panicReports: Int
    /// The core's background-run counters (ADR-046); all zero when the core has none.
    public var background: UndraBackgroundStats

    /// `UndraObject`s (and stores) created and not yet closed.
    public var hostLiveHandles: Int
    /// Stores registered with the mirror.
    public var hostMirroredStores: Int
    /// Calls sent and not yet answered.
    public var hostPendingCalls: Int
    /// Streams open.
    public var hostOpenStreams: Int
    /// Page calls (target 3) the host has sent to the core so far: what the lazy lists of the stores asked for (ADR-043). Counted
    /// when a list asks, whether the call is answered, fails or is cancelled.
    public var hostPageCalls: Int
    /// Ports registered.
    public var hostRegisteredPorts: Int
    /// The mirror's delivery counters: change-sets and entries received, entries applied after
    /// merging, drains, compactions, resyncs (docs/SPEC.md section 17). All zero unless the
    /// snapshot comes from `UndraCore.stats()`.
    public var mirror: MirrorStats

    /// Statistics with nothing known: all zero, no document.
    public init() {
        self.init(json: "")
    }

    /// Statistics parsed from the core's document; the host counters are zero.
    public init(json: String) {
        let values = UndraStats.flattenIntegers(in: json)
        self.json = json
        self.values = values
        self.coreLiveHandles = values["live_handles"] ?? 0
        self.coreLiveStores = values["live_stores"] ?? 0
        self.coreTasks = values["tasks"] ?? 0
        self.coreActiveCalls = values["active_calls"] ?? 0
        self.coreOpenStreams = values["open_streams"] ?? 0
        self.coreTransactions = values["transactions"] ?? 0
        self.corePanics = values["panics"] ?? 0
        self.hostRefs = values["host_refs"] ?? 0
        self.panicReports = values["panic_reports"] ?? 0
        self.background = UndraBackgroundStats(
            tasks: values["background.tasks"] ?? 0,
            pending: values["background.pending"] ?? 0,
            runs: values["background.runs"] ?? 0,
            finished: values["background.finished"] ?? 0,
            replayed: values["background.replayed"] ?? 0,
            refetched: values["background.refetched"] ?? 0
        )
        self.hostLiveHandles = 0
        self.hostMirroredStores = 0
        self.hostPendingCalls = 0
        self.hostOpenStreams = 0
        self.hostPageCalls = 0
        self.hostRegisteredPorts = 0
        self.mirror = MirrorStats()
    }

    /// Extracts every integer value of a JSON document, keyed by its dotted path. Strings,
    /// booleans, nulls, arrays and fractional numbers are skipped; a document that is not valid
    /// JSON yields whatever was read before the first error.
    static func flattenIntegers(in json: String) -> [String: Int] {
        var parser = IntegerFlattener(bytes: Array(json.utf8))
        parser.parseDocument()
        return parser.result
    }
}

/// A forgiving, allocation-light JSON walker that only records integers.
private struct IntegerFlattener {
    let bytes: [UInt8]
    var index = 0
    var result: [String: Int] = [:]

    init(bytes: [UInt8]) {
        self.bytes = bytes
    }

    mutating func parseDocument() {
        skipSpace()
        _ = parseValue(path: "")
    }

    /// Parses one value; returns `false` when the input is malformed and parsing must stop.
    private mutating func parseValue(path: String) -> Bool {
        skipSpace()
        guard index < bytes.count else {
            return false
        }
        switch bytes[index] {
        case UInt8(ascii: "{"):
            return parseObject(path: path)
        case UInt8(ascii: "["):
            return skipArray()
        case UInt8(ascii: "\""):
            return parseString() != nil
        case UInt8(ascii: "t"):
            return skipLiteral("true")
        case UInt8(ascii: "f"):
            return skipLiteral("false")
        case UInt8(ascii: "n"):
            return skipLiteral("null")
        default:
            return parseNumber(path: path)
        }
    }

    private mutating func parseObject(path: String) -> Bool {
        index += 1
        skipSpace()
        if index < bytes.count, bytes[index] == UInt8(ascii: "}") {
            index += 1
            return true
        }
        while index < bytes.count {
            skipSpace()
            guard let key = parseString() else {
                return false
            }
            skipSpace()
            guard index < bytes.count, bytes[index] == UInt8(ascii: ":") else {
                return false
            }
            index += 1
            let childPath = path.isEmpty ? key : path + "." + key
            if !parseValue(path: childPath) {
                return false
            }
            skipSpace()
            guard index < bytes.count else {
                return false
            }
            if bytes[index] == UInt8(ascii: ",") {
                index += 1
                continue
            }
            if bytes[index] == UInt8(ascii: "}") {
                index += 1
                return true
            }
            return false
        }
        return false
    }

    /// Skips an array, which may contain nested arrays, objects and strings.
    private mutating func skipArray() -> Bool {
        var depth = 0
        while index < bytes.count {
            let byte = bytes[index]
            if byte == UInt8(ascii: "\"") {
                if parseString() == nil {
                    return false
                }
                continue
            }
            if byte == UInt8(ascii: "[") || byte == UInt8(ascii: "{") {
                depth += 1
            } else if byte == UInt8(ascii: "]") || byte == UInt8(ascii: "}") {
                depth -= 1
                if depth == 0 {
                    index += 1
                    return true
                }
            }
            index += 1
        }
        return false
    }

    /// Reads a string starting at the opening quote and returns its (unescaped, best effort)
    /// content, or `nil` if the string is unterminated.
    private mutating func parseString() -> String? {
        guard index < bytes.count, bytes[index] == UInt8(ascii: "\"") else {
            return nil
        }
        index += 1
        var content: [UInt8] = []
        while index < bytes.count {
            let byte = bytes[index]
            if byte == UInt8(ascii: "\"") {
                index += 1
                return String(decoding: content, as: UTF8.self)
            }
            if byte == UInt8(ascii: "\\") {
                index += 1
                guard index < bytes.count else {
                    return nil
                }
                let escaped = bytes[index]
                switch escaped {
                case UInt8(ascii: "n"):
                    content.append(0x0A)
                case UInt8(ascii: "t"):
                    content.append(0x09)
                case UInt8(ascii: "r"):
                    content.append(0x0D)
                case UInt8(ascii: "u"):
                    // Keep a \uXXXX escape as literal text: keys the runtime cares about are ASCII.
                    content.append(UInt8(ascii: "\\"))
                    content.append(escaped)
                default:
                    content.append(escaped)
                }
                index += 1
                continue
            }
            content.append(byte)
            index += 1
        }
        return nil
    }

    private mutating func parseNumber(path: String) -> Bool {
        let start = index
        if index < bytes.count, bytes[index] == UInt8(ascii: "-") {
            index += 1
        }
        let digitsStart = index
        var value = 0
        var overflow = false
        while index < bytes.count, bytes[index] >= UInt8(ascii: "0"), bytes[index] <= UInt8(ascii: "9") {
            let digit = Int(bytes[index] - UInt8(ascii: "0"))
            let scaled = value.multipliedReportingOverflow(by: 10)
            let summed = scaled.partialValue.addingReportingOverflow(digit)
            if scaled.overflow || summed.overflow {
                overflow = true
            } else {
                value = summed.partialValue
            }
            index += 1
        }
        if index == digitsStart {
            return false
        }
        var isInteger = true
        while index < bytes.count {
            let byte = bytes[index]
            if byte == UInt8(ascii: ".") || byte == UInt8(ascii: "e") || byte == UInt8(ascii: "E")
                || byte == UInt8(ascii: "+") || byte == UInt8(ascii: "-") {
                isInteger = false
                index += 1
            } else if byte >= UInt8(ascii: "0"), byte <= UInt8(ascii: "9") {
                index += 1
            } else {
                break
            }
        }
        if isInteger, !overflow {
            result[path] = bytes[start] == UInt8(ascii: "-") ? -value : value
        }
        return true
    }

    private mutating func skipLiteral(_ literal: String) -> Bool {
        let expected = Array(literal.utf8)
        guard index + expected.count <= bytes.count else {
            return false
        }
        var offset = 0
        while offset < expected.count {
            if bytes[index + offset] != expected[offset] {
                return false
            }
            offset += 1
        }
        index += expected.count
        return true
    }

    private mutating func skipSpace() {
        while index < bytes.count {
            let byte = bytes[index]
            if byte == 0x20 || byte == 0x0A || byte == 0x0D || byte == 0x09 {
                index += 1
            } else {
                break
            }
        }
    }
}
