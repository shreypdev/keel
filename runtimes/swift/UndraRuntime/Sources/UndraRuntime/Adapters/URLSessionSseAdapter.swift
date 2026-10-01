// Server-sent events (ADR-047) on URLSession.bytes(for:) and the runtime's SseParser.

import Foundation

/// `Sse` on `URLSession.bytes(for:)`, parsed by ``SseParser`` (the HTML standard's algorithm).
///
/// The request carries `Accept: text/event-stream`, `Cache-Control: no-cache`, the core's headers
/// and, when resuming, `Last-Event-ID`. `open` returns once the answer's head arrived: a status
/// other than 2xx is ``SseError/refused(status:message:)`` with it (a 204 means "stop"), a content
/// type other than `text/event-stream` is ``SseError/protocol(_:)``. The body is read as the
/// binding pulls events; its end is ``SseError/ended``, a failure while reading
/// ``SseError/network(_:)``, bytes that are not UTF-8 ``SseError/protocol(_:)``. `close` cancels
/// the request, so the server sees the client leave.
///
/// It is also the `Sse` adapter of ``Adapters/platformDefault`` (served by the binding of
/// ``SsePortAdapter``).
public final class URLSessionSseAdapter: SseAdapter, UndraAdapter, @unchecked Sendable {
    private let session: URLSession
    private let bindings = BindingSet<SseBinding>()

    /// Streams over `session`. The default one never times out an idle stream (an event stream
    /// may stay quiet for minutes) and does not cache.
    public init(session: URLSession = URLSessionSseAdapter.makeDefaultSession()) {
        self.session = session
    }

    /// The session used when none is passed: ephemeral, no request timeout while the stream is
    /// quiet, no waiting for connectivity.
    public static func makeDefaultSession() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.waitsForConnectivity = false
        configuration.timeoutIntervalForRequest = 24 * 60 * 60
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: configuration)
    }

    /// Requests `url` and returns once a 2xx `text/event-stream` answer's head arrived.
    public func open(url: String, headers: [Header], lastEventId: String?) async throws(SseError) -> any SseStream {
        guard let target = URL(string: url), let scheme = target.scheme?.lowercased(),
              scheme == "http" || scheme == "https", target.host != nil
        else {
            throw SseError.refused(status: nil, message: "invalid URL: \(url)")
        }
        var request = URLRequest(url: target)
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("no-cache", forHTTPHeaderField: "Cache-Control")
        for header in headers {
            request.addValue(header.value, forHTTPHeaderField: header.name)
        }
        if let lastEventId = lastEventId {
            request.setValue(lastEventId, forHTTPHeaderField: "Last-Event-ID")
        }
        let bytes: URLSession.AsyncBytes
        let response: URLResponse
        do {
            (bytes, response) = try await session.bytes(for: request)
        } catch let error as URLError {
            if error.code == .badURL || error.code == .unsupportedURL {
                throw SseError.refused(status: nil, message: "invalid URL: \(url)")
            }
            throw SseError.network(error.localizedDescription)
        } catch {
            throw SseError.network((error as NSError).localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else {
            bytes.task.cancel()
            throw SseError.protocol("the answer is not HTTP")
        }
        guard (200 ..< 300).contains(http.statusCode), http.statusCode != 204 else {
            bytes.task.cancel()
            let status = http.statusCode
            throw SseError.refused(
                status: UInt16(clamping: status),
                message: "the server answered HTTP \(status) (\(HTTPURLResponse.localizedString(forStatusCode: status)))"
            )
        }
        let contentType = http.value(forHTTPHeaderField: "Content-Type") ?? ""
        let mediaType = contentType.split(separator: ";", maxSplits: 1).first
            .map { $0.trimmingCharacters(in: .whitespaces).lowercased() } ?? ""
        guard mediaType == "text/event-stream" else {
            bytes.task.cancel()
            throw SseError.protocol("expected text/event-stream, got \(contentType.isEmpty ? "no content type" : contentType)")
        }
        return URLSessionSseStream(bytes: bytes, lastEventId: lastEventId)
    }

    // MARK: UndraAdapter

    public var portId: UInt32 {
        return StandardPorts.Sse.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return bindings.add(SseBinding(adapter: self)).portImpl()
    }

    public func detach() {
        bindings.detachAll()
    }
}

/// One response body, parsed as the binding pulls.
final class URLSessionSseStream: SseStream, @unchecked Sendable {
    /// The reader: touched by one `next()` at a time (the binding's pump).
    private final class Reader: @unchecked Sendable {
        var iterator: URLSession.AsyncBytes.AsyncIterator
        var parser: SseParser
        var ready: [SseEvent] = []
        var done = false

        init(_ bytes: URLSession.AsyncBytes, lastEventId: String?) {
            self.iterator = bytes.makeAsyncIterator()
            self.parser = SseParser(lastEventId: lastEventId)
        }
    }

    private let task: URLSessionDataTask
    private let reader: Reader
    private let closing = Guarded(false)

    init(bytes: URLSession.AsyncBytes, lastEventId: String?) {
        self.task = bytes.task
        self.reader = Reader(bytes, lastEventId: lastEventId)
    }

    var events: AsyncThrowingStream<SseEvent, any Error> {
        return AsyncThrowingStream(unfolding: { [self] () async throws -> SseEvent? in
            return try await self.nextEvent()
        })
    }

    func close() async {
        closing.withLock { (value: inout Bool) -> Void in
            value = true
        }
        task.cancel()
    }

    private var isClosing: Bool {
        return closing.withLock { (value: inout Bool) -> Bool in value }
    }

    /// The next event: read bytes until the parser completes one.
    private func nextEvent() async throws -> SseEvent? {
        let reader = self.reader
        while reader.ready.isEmpty {
            if reader.done || isClosing {
                return nil
            }
            let byte: UInt8?
            do {
                byte = try await reader.iterator.next()
            } catch {
                if isClosing {
                    return nil
                }
                reader.done = true
                throw SseError.network((error as NSError).localizedDescription)
            }
            guard let byte = byte else {
                reader.done = true
                throw SseError.ended
            }
            do {
                reader.ready.append(contentsOf: try reader.parser.push(CollectionOfOne(byte)))
            } catch {
                reader.done = true
                task.cancel()
                throw error
            }
        }
        return reader.ready.removeFirst()
    }
}
