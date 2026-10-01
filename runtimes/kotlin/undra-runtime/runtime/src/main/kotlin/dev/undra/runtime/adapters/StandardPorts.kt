package dev.undra.runtime.adapters

/**
 * Port and method ids of the standard ports (`undra-ports`, SPEC section 8), so adapters can register
 * themselves without generated code: `port_id = fnv1a32("port.<Trait>")` and
 * `method_id = fnv1a32("<Trait>.<method>")` (SPEC 1.1). A test recomputes every constant with
 * `Fnv.fnv1a32`, so a typo cannot survive.
 */
public object StandardPorts {
    /** `Clock`: `now_ms() -> i64`, `monotonic_ns() -> u64` (sync). */
    public object Clock {
        /** `fnv1a32("port.Clock")`. */
        public const val PORT_ID: UInt = 0xcd99c48eu

        /** `fnv1a32("Clock.now_ms")`. */
        public const val NOW_MS: UInt = 0xccc94d90u

        /** `fnv1a32("Clock.monotonic_ns")`. */
        public const val MONOTONIC_NS: UInt = 0x2cb2b4bfu
    }

    /** `Rng`: `fill(len: u32) -> Bytes` (sync). */
    public object Rng {
        /** `fnv1a32("port.Rng")`. */
        public const val PORT_ID: UInt = 0x25135bf5u

        /** `fnv1a32("Rng.fill")`. */
        public const val FILL: UInt = 0x2832b8edu
    }

    /** `Log`: `log(level: u8, target: String, message: String)` (sync). */
    public object Log {
        /** `fnv1a32("port.Log")`. */
        public const val PORT_ID: UInt = 0x575ff24au

        /** `fnv1a32("Log.log")`. */
        public const val LOG: UInt = 0xd49d5649u
    }

    /** `Http`: `request(req: HttpRequest) -> Result<HttpResponse, HttpError>` (async). */
    public object Http {
        /** `fnv1a32("port.Http")`. */
        public const val PORT_ID: UInt = 0x1ebeb908u

        /** `fnv1a32("Http.request")`. */
        public const val REQUEST: UInt = 0x6b14df26u
    }

    /** `Kv`: `get`, `set`, `delete`, `list` (async). */
    public object Kv {
        /** `fnv1a32("port.Kv")`. */
        public const val PORT_ID: UInt = 0x5389110du

        /** `fnv1a32("Kv.get")`. */
        public const val GET: UInt = 0xf050bb1au

        /** `fnv1a32("Kv.set")`. */
        public const val SET: UInt = 0x62427856u

        /** `fnv1a32("Kv.delete")`. */
        public const val DELETE: UInt = 0x60a386b9u

        /** `fnv1a32("Kv.list")`. */
        public const val LIST: UInt = 0x32f1d03au
    }

    /** `SecureStore`: the same four methods as [Kv], under its own ids (async). */
    public object SecureStore {
        /** `fnv1a32("port.SecureStore")`. */
        public const val PORT_ID: UInt = 0xc01f5beau

        /** `fnv1a32("SecureStore.get")`. */
        public const val GET: UInt = 0x57036f6fu

        /** `fnv1a32("SecureStore.set")`. */
        public const val SET: UInt = 0xe91e017bu

        /** `fnv1a32("SecureStore.delete")`. */
        public const val DELETE: UInt = 0xd57db4e2u

        /** `fnv1a32("SecureStore.list")`. */
        public const val LIST: UInt = 0xf5bab8c9u
    }

    /** `Fs`: `read`, `write`, `delete`, `list` (async). */
    public object Fs {
        /** `fnv1a32("port.Fs")`. */
        public const val PORT_ID: UInt = 0x4ea34cabu

        /** `fnv1a32("Fs.read")`. */
        public const val READ: UInt = 0x01fdbe44u

        /** `fnv1a32("Fs.write")`. */
        public const val WRITE: UInt = 0x6b70d47fu

        /** `fnv1a32("Fs.delete")`. */
        public const val DELETE: UInt = 0xa90a826bu

        /** `fnv1a32("Fs.list")`. */
        public const val LIST: UInt = 0x4fba8678u
    }

    /** `Timer`: `set(timer_id: u32, delay_ms: u64)`, fire-and-forget; completion comes back as `TimerFired`. */
    public object Timer {
        /** `fnv1a32("port.Timer")`. */
        public const val PORT_ID: UInt = 0x00c2cdd9u

        /** `fnv1a32("Timer.set")`. */
        public const val SET: UInt = 0x923a766cu
    }

    /** `Connectivity` (event port): `changed(online: bool, kind: NetKind)`. */
    public object Connectivity {
        /** `fnv1a32("port.Connectivity")`. */
        public const val PORT_ID: UInt = 0x1feff6ffu

        /** `fnv1a32("Connectivity.changed")`. */
        public const val CHANGED: UInt = 0xb4f2a010u
    }

    /** `Lifecycle` (event port): `changed(state: AppState)`. */
    public object Lifecycle {
        /** `fnv1a32("port.Lifecycle")`. */
        public const val PORT_ID: UInt = 0x81c0afd4u

        /** `fnv1a32("Lifecycle.changed")`. */
        public const val CHANGED: UInt = 0x0bc82569u
    }

    /**
     * `WebSocket` (async, opt-in: cargo feature `websocket`, ADR-047): `connect`, `send`, `receive`, `close`. Served by
     * [WebSocketPortAdapter] over a [WebSocketAdapter].
     */
    public object WebSocket {
        /** `fnv1a32("port.WebSocket")`. */
        public const val PORT_ID: UInt = 0x7388b95fu

        /** `fnv1a32("WebSocket.connect")`: `connect(url, protocols, headers) -> Result<WsOpened, WsError>`. */
        public const val CONNECT: UInt = 0x83477638u

        /** `fnv1a32("WebSocket.send")`: `send(conn, message) -> Result<(), WsError>`. */
        public const val SEND: UInt = 0x117b2158u

        /** `fnv1a32("WebSocket.receive")`: `receive(conn, max) -> Result<Vec<WsMessage>, WsError>`. */
        public const val RECEIVE: UInt = 0x8f31f08fu

        /** `fnv1a32("WebSocket.close")`: `close(conn, code, reason) -> Result<(), WsError>`. */
        public const val CLOSE: UInt = 0x60154b86u
    }

    /** `Sse` (async, opt-in: cargo feature `sse`, ADR-047): `open`, `next`, `close`. Served by [SsePortAdapter] over an [SseAdapter]. */
    public object Sse {
        /** `fnv1a32("port.Sse")`. */
        public const val PORT_ID: UInt = 0x75d2ef19u

        /** `fnv1a32("Sse.open")`: `open(url, headers, last_event_id) -> Result<u32, SseError>`. */
        public const val OPEN: UInt = 0xc0033c14u

        /** `fnv1a32("Sse.next")`: `next(stream, max) -> Result<Vec<SseEvent>, SseError>`. */
        public const val NEXT: UInt = 0x4035cbedu

        /** `fnv1a32("Sse.close")`: `close(stream) -> Result<(), SseError>`. */
        public const val CLOSE: UInt = 0x5bfe2c88u
    }

    /**
     * `Db` (async, opt-in: cargo feature `db`, ADR-048): `open`, `execute`, `query`, `begin`, `commit`, `rollback`,
     * `close`. Served by [DbPortAdapter] over a [DbAdapter].
     */
    public object Db {
        /** `fnv1a32("port.Db")`. */
        public const val PORT_ID: UInt = 0x559eda82u

        /** `fnv1a32("Db.open")`: `open(name, migrations) -> Result<DbOpened, DbError>`. */
        public const val OPEN: UInt = 0xee6f26dbu

        /** `fnv1a32("Db.execute")`: `execute(db, sql, params) -> Result<DbExecuted, DbError>`. */
        public const val EXECUTE: UInt = 0xffac2f0au

        /** `fnv1a32("Db.query")`: `query(db, sql, params) -> Result<DbRows, DbError>`. */
        public const val QUERY: UInt = 0x3a4deefdu

        /** `fnv1a32("Db.begin")`: `begin(db) -> Result<u32, DbError>`. */
        public const val BEGIN: UInt = 0xae2ba428u

        /** `fnv1a32("Db.commit")`: `commit(tx) -> Result<(), DbError>`. */
        public const val COMMIT: UInt = 0xf866d5aeu

        /** `fnv1a32("Db.rollback")`: `rollback(tx) -> Result<(), DbError>`. */
        public const val ROLLBACK: UInt = 0x3e7b24b3u

        /** `fnv1a32("Db.close")`: `close(db) -> Result<(), DbError>`. */
        public const val CLOSE: UInt = 0xde3dc7edu
    }
}
