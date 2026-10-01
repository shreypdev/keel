import Foundation
import UndraRuntime

// The wire layout of the standard `Http` port (docs/SPEC.md section 8), for an app that answers the
// port itself. `UndraRuntime` keeps these records internal (its README explains why) and the
// generated bindings only declare the standard types the app's schema refers to, which for this
// app is `HttpError`: the request and the response are written out here. Every field is the
// encoding of docs/SPEC.md section 3.1, in declaration order.

/// `Header { name: String, value: String }`.
struct WireHeader: UndraCodec {
    var name: String
    var value: String

    static func undraDecode(_ r: inout UndraReader) throws -> WireHeader {
        return WireHeader(name: try r.readString(), value: try r.readString())
    }

    func undraEncode(_ w: inout UndraWriter) {
        w.writeString(name)
        w.writeString(value)
    }
}

/// `HttpRequest { method, url: String, headers: Vec<Header>, body: Option<Bytes>, timeout_ms: Option<u32> }`.
struct WireHttpRequest {
    /// `HttpMethod` is a `u16` index in this order.
    private static let methods = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"]

    /// The method as written in a request line: `"GET"`, `"POST"`, ...
    var method: String
    var url: String
    var headers: [WireHeader]
    var body: Data?

    /// Reads the arguments of `Http.request`.
    static func decode(_ arguments: [UInt8]) throws -> WireHttpRequest {
        var reader = UndraReader(arguments)
        let index = Int(try reader.readU16())
        guard index < methods.count else {
            throw WireError.invalidTag(tag: UInt32(index), at: 0, type: "HttpMethod")
        }
        let url = try reader.readString()
        let headers = try [WireHeader].undraDecode(&reader)
        let body = try Optional<UndraBytes>.undraDecode(&reader)
        _ = try Optional<UInt32>.undraDecode(&reader)
        try reader.finish()
        return WireHttpRequest(method: methods[index], url: url, headers: headers, body: body.map { Data($0.bytes) })
    }

    /// The value of the header called `name`, compared without regard to case.
    func header(_ name: String) -> String? {
        return headers.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?.value
    }
}

/// `HttpResponse { status: u16, headers: Vec<Header>, body: Bytes }`.
struct WireHttpResponse {
    var status: UInt16
    var headers: [WireHeader] = [WireHeader(name: "Content-Type", value: "application/json")]
    var body: Data

    /// The reply of `Http.request`.
    func encoded() -> [UInt8] {
        var writer = UndraWriter()
        writer.writeU16(status)
        headers.undraEncode(&writer)
        writer.writeBytes([UInt8](body))
        return writer.finish()
    }
}

/// The parameters of the `Connectivity.changed(online, kind)` event.
enum WireConnectivity {
    /// `fnv1a32("port.Connectivity")`.
    static let portId = fnv1a32("port.Connectivity")
    /// `fnv1a32("Connectivity.changed")`.
    static let changedMethod = fnv1a32("Connectivity.changed")

    /// `NetKind` is a `u16` index: `Wifi` 0, `Cellular` 1, `Wired` 2, `Unknown` 3, `None` 4.
    static func changed(online: Bool) -> [UInt8] {
        var writer = UndraWriter()
        writer.writeBool(online)
        writer.writeU16(online ? 0 : 4)
        return writer.finish()
    }
}
