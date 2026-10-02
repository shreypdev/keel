import type { WireErrorDetail } from "./wire/errors.js";

/*
 * The runtime's messages: the development flavour (ADR-057, decisions D1 and D8).
 *
 * Every sentence the runtime throws or logs is `msg(code, ...values)`: a literal code and the values it says. This module holds the
 * table of sentences, one per code, with `{0}`, `{1}` for the values, and formats them as the sentences always read. The
 * production build of the package (`npm run build`, the `default` export condition) swaps this module for `messages.prod.ts`, whose
 * `msg` says the code, the values and a link to the errors page instead (`T0017: callSync, remote — https://.../errors.html#T0017`),
 * so a production page does not carry the prose of an error whose class, `kind`, fields and `code` it already has. Both
 * flavours throw the same classes with the same `kind` and fields; only `message` differs (R6: every error is still a typed value).
 *
 * The table is the one source of the T-codes: `site/scripts/build-errors.mjs` renders it into the "Runtime messages" section of
 * `site/docs/errors.html` (one line per code, `N: "sentence", // where it is raised`: keep that shape), and
 * `test/messages.test.ts` holds the rules: every call site names a code of the table, every code is used, no two codes say the
 * same, and the numbers are never reused (a retired code stays as `null`; a new sentence takes the next number).
 */

/** The sentences by code. A code is never reused: one that is retired stays as `null`. */
const MESSAGES: Readonly<Record<number, string | null>> = {
  1: "unknown {0} variant: {1}", // adapters/codecs.ts: TypeError; adapters/events.ts: RangeError
  2: "unknown WsMessage variant: {0}", // adapters/codecs.ts: TypeError
  3: "unknown WsError variant: {0}", // adapters/codecs.ts: TypeError
  4: "unknown SseError variant: {0}", // adapters/codecs.ts: TypeError
  5: "unknown DbValue variant: {0}", // adapters/codecs.ts: TypeError
  6: "unknown DbError variant: {0}", // adapters/codecs.ts: TypeError
  7: null, // retired: adapters/fs.ts says it in both flavours (data: a port error's field, which the core receives)
  8: null, // retired: adapters/fs.ts says it in both flavours (data: a port error's field, which the core receives)
  9: null, // retired: adapters/http.ts says it in both flavours (data: a port error's field, which the core receives)
  10: null, // retired: adapters/idb.ts says it in both flavours (data: a port error's field, which the core receives)
  11: null, // retired: adapters/idb.ts says it in both flavours (data: a port error's field, which the core receives)
  12: null, // retired: adapters/idb.ts says it in both flavours (data: a port error's field, which the core receives)
  13: null, // retired: adapters/idb.ts says it in both flavours (data: a port error's field, which the core receives)
  14: null, // retired: adapters/idb.ts says it in both flavours (data: a port error's field, which the core receives)
  15: null, // retired: adapters/idb.ts says it in both flavours (data: a port error's field, which the core receives)
  16: null, // retired: adapters/kv.ts says it in both flavours (data: a port error's field, which the core receives)
  17: null, // retired: adapters/kv.ts says it in both flavours (data: a port error's field, which the core receives)
  18: "namespace {0} is not [a-z][a-z0-9_]{0,31}", // adapters/names.ts: UndraError("options")
  19: "Rng.fill: {0} bytes requested, the limit is {1}", // adapters/ports.ts: RangeError
  20: "{0}: {1} ({2})", // adapters/ports.ts: log
  21: null, // retired: adapters/secure.ts says it in both flavours (data: a port error's field, which the core receives)
  22: null, // retired: adapters/secure.ts says it in both flavours (data: a port error's field, which the core receives)
  23: null, // retired: adapters/secure.ts says it in both flavours (data: a port error's field, which the core receives)
  24: "WebCrypto is required (crypto.getRandomValues)", // adapters/system.ts: VariableDeclaration
  25: "a typed error that does not decode ({0} bytes)", // call-error.ts: UndraCallError.Malformed
  26: "a stream without an error type ended with a typed error item ({0} bytes)", // call-error.ts: UndraCallError.Malformed
  27: "an unexpected failure: {0}", // call-error.ts: UndraCallError.Malformed
  28: "the Undra core cancelled the call (a restore replaced its object, or the core shut down)", // call-error.ts: super("cancelledByCore")
  29: "the Undra core panicked: {0}", // call-error.ts: super("panicked")
  30: "the Undra core refused the call: {0}", // call-error.ts: super("refused")
  31: "the Undra core is unavailable: {0}", // call-error.ts: super("unavailable")
  32: "the Undra core sent a reply the bindings cannot read: {0}", // call-error.ts: BinaryExpression
  33: "{0} failed: {1}", // call-error.ts: super("unhandled"); worker.ts: log
  34: "the reply does not decode: {0}", // call-error.ts: UndraCallError.Malformed
  35: "a port implementation's typed failure reached a call", // call-error.ts: UndraCallError.Malformed
  36: "the core answered with a typed error, but this method has none ({0} bytes)", // call-error.ts: UndraCallError.Malformed
  37: "the core opened a stream where a single reply was expected", // call-error.ts: UndraCallError.Malformed
  38: "the core answered with status {0} as a failure", // call-error.ts: UndraCallError.Malformed
  39: "u64 out of range: {0}", // call-head.ts: RangeError
  40: "a bare CallTarget must be FreeFunction; pass { target, handle } for a method", // call-head.ts: TypeError
  41: "u32 out of range: {0}", // call-head.ts: RangeError
  42: "the callback's target was garbage-collected", // callbacks.ts: UndraError("state")
  43: "callback instance {0} was released more often than it was lent", // callbacks.ts: UndraError("state")
  44: "the core is not loaded: load it at app startup (the bindings' Undra<Namespace>.load(...), or UndraCore.load(...)), before creating any Undra object, or pass a core explicitly", // core.ts: VariableDeclaration
  45: "The operation was aborted", // core.ts: Error
  46: "a core was used while it is not loaded (before its load(...) succeeded, or after it was closed); calls on it reject with UndraCallError.Unavailable. Load the core at app startup, before creating any Undra object.", // core.ts: log
  47: "mode 'wasm-main' needs the `wasm` option", // core.ts: UndraError("options")
  48: "import failed: {0}", // core.ts: log
  49: "unknown mode '{0}'", // core.ts: UndraError("options")
  50: "the core is closed", // core.ts: PropertyDeclaration; core.ts: UndraTransportError("closed"); transport/wasm-main.ts: UndraTransportError("closed"); ...
  51: "the core returned a truncated reply", // core.ts: UndraTransportError("protocol")
  52: "the core returned the null handle for a constructor", // core.ts: UndraTransportError("protocol")
  53: "{0} (the connection to the core is down: see UndraCore.connection)", // core.ts: log
  54: "the onError handler threw while handling \"{0}\": {1}", // core.ts: log
  55: "the core was closed", // core.ts: UndraTransportError("closed")
  56: "the core sent a truncated reply", // core.ts: UndraTransportError("protocol")
  57: "the core sent reply status {0}", // core.ts: UndraTransportError("protocol"); stream-support.ts: UndraTransportError("protocol")
  58: "this method is a stream; call it with UndraCore.stream", // core.ts: UndraError("state")
  59: null, // retired: db-worker.ts says it in both flavours (data: a port error's field, which the core receives)
  60: null, // retired: db-worker.ts says it in both flavours (data: a port error's field, which the core receives)
  61: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  62: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  63: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  64: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  65: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  66: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  67: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  68: null, // retired: db/binding.ts says it in both flavours (data: a port error's field, which the core receives)
  69: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  70: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  71: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  72: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  73: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  74: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  75: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  76: null, // retired: db/protocol.ts says it in both flavours (data: a port error's field, which the core receives)
  77: null, // retired: db/protocol.ts says it in both flavours (data: a port error's field, which the core receives)
  78: null, // retired: db/protocol.ts says it in both flavours (data: a port error's field, which the core receives)
  79: null, // retired: db/protocol.ts says it in both flavours (data: a port error's field, which the core receives)
  80: null, // retired: db/protocol.ts says it in both flavours (data: a port error's field, which the core receives)
  81: null, // retired: db/wa-sqlite-engine.ts says it in both flavours (data: a port error's field, which the core receives)
  82: null, // retired: db/wa-sqlite-engine.ts says it in both flavours (data: a port error's field, which the core receives)
  83: null, // retired: db/wa-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  84: "the dev server no longer has this core's objects (it was restarted, or the session expired); load a new core", // errors-rare.ts: Parameter
  85: "the Undra core rejected the snapshot (code {0}); a rejected restore leaves the core unchanged", // errors-rare.ts: super("restore")
  86: "the call failed with a typed error", // errors.ts: ReturnStatement
  87: "the core panicked", // errors.ts: ConditionalExpression
  88: "the core panicked: {0}", // errors.ts: ConditionalExpression
  89: "the call was cancelled", // errors.ts: ReturnStatement
  90: "the core rejected the request", // errors.ts: ConditionalExpression
  91: "the core rejected the request: {0}", // errors.ts: ConditionalExpression
  92: "unexpected reply status {0}", // errors.ts: ReturnStatement
  93: "{0} is not available in mode '{1}'", // errors.ts: super("mode")
  94: "schema mismatch: the bindings expect {0} but the core reports {1}; regenerate the bindings or rebuild the core", // errors.ts: super("schemaMismatch")
  95: "typed port failure", // errors.ts: super("port")
  96: "the core sent the null handle where an object was expected", // identity.ts: UndraTransportError("protocol")
  97: "the core sent option tag {0}", // identity.ts: UndraTransportError("protocol")
  98: "{0} belongs to another core: pass an object of the core the call goes to", // identity.ts: UndraCallError.Refused
  99: "{0} must be an integer between 1 and {1}, not {2}", // lazy.ts: RangeError
  100: "a lazy list was invalidated before its value (and so its page server) reached the host", // lazy.ts: UndraTransportError("protocol")
  101: "the core sent {0} rows for page {1} of a list of {2}, which holds {3}", // lazy.ts: UndraTransportError("protocol")
  102: "the core sent a list length of {0} at version {1}, which it told us had {2}", // lazy.ts: UndraTransportError("protocol")
  103: "the core keeps answering page {0} at version {1}, older than the list's {2}", // lazy.ts: UndraTransportError("protocol")
  104: "no change-set arrived for signal {0} of handle {1} within {2} ms; is the handle a live store?", // mirror-waiters.ts: UndraError("observe")
  105: "handle {0} is already registered with the mirror", // mirror.ts: UndraError("state")
  106: "the mirror applied {0} rounds of change-sets in one flush: a signal subscriber keeps causing changes to a store it observes", // mirror.ts: UndraError("state")
  107: "a keyed patch for signal {0} of handle {1} has no operation count; the signal is re-observed", // mirror.ts: UndraError("state")
  108: "this mirror has no observe waiters: a core installs them for a transport that answers later; for any other mirror call mirrorWaiters(mirror) (from @undra/runtime) once, before whenObserved", // mirror.ts: UndraError("state")
  109: "{0} needs Node; this is not Node", // node-builtin.ts: ConditionalExpression
  110: "{0} needs Node 20.16 or 22.3 or later (process.getBuiltinModule); this is Node {1}", // node-builtin.ts: ConditionalExpression
  111: "this Node has no {0}", // node-builtin.ts: Error
  112: "the runtime's {0} could not be loaded: {1}", // on-demand.ts: UndraTransportError("closed")
  113: "{0} method 0x{1}", // port-dispatch.ts: ReturnStatement
  114: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  115: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  116: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  117: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  118: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  119: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  120: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  121: null, // retired: realtime/browser-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  122: "the events of this stream were already taken", // realtime/fetch-sse.ts: TypeError
  123: null, // retired: realtime/fetch-sse.ts says it in both flavours (data: a port error's field, which the core receives)
  124: null, // retired: realtime/fetch-sse.ts says it in both flavours (data: a port error's field, which the core receives)
  125: null, // retired: realtime/fetch-sse.ts says it in both flavours (data: a port error's field, which the core receives)
  126: null, // retired: realtime/fetch-sse.ts says it in both flavours (data: a port error's field, which the core receives)
  127: null, // retired: realtime/fetch-sse.ts says it in both flavours (data: a port error's field, which the core receives)
  128: "the messages of this connection were already taken", // realtime/inbox.ts: TypeError
  129: "a next() is already pending", // realtime/inbox.ts: TypeError
  130: "ByteQueue.at past the end", // realtime/node-websocket.ts: RangeError
  131: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  132: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  133: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  134: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  135: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  136: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  137: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  138: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  139: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  140: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  141: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  142: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  143: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  144: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  145: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  146: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  147: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  148: null, // retired: realtime/node-websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  149: null, // retired: realtime/sse.ts says it in both flavours (data: a port error's field, which the core receives)
  150: null, // retired: realtime/sse.ts says it in both flavours (data: a port error's field, which the core receives)
  151: null, // retired: realtime/sse.ts says it in both flavours (data: a port error's field, which the core receives)
  152: null, // retired: realtime/websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  153: null, // retired: realtime/websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  154: null, // retired: realtime/websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  155: null, // retired: realtime/websocket.ts says it in both flavours (data: a port error's field, which the core receives)
  156: "the wasm core trapped and was restarted from its last snapshot; this call may or may not have run before the trap, and it is not retried ({0})", // recovery.ts: UndraTransportError("restarted")
  157: "the wasm core trapped ({0}) and was restarted from no snapshot", // recovery.ts: BinaryExpression
  158: "a snapshot of {0} bytes was not kept for crash recovery: it is larger than maxSnapshotBytes ({1}); the previous one stays (said once)", // recovery.ts: log
  159: "a snapshot for crash recovery failed: {0}", // recovery.ts: log
  160: "the core refused its last snapshot (code {0}) after the restart; it runs without its stores", // recovery.ts: log
  161: "this crashRecovery() already belongs to a core: make one per core", // recovery.ts: UndraTransportError("unsupported")
  162: "the wasm core is restarting after a trap", // recovery.ts: UndraTransportError("restarted")
  163: "the wasm core could not be restarted: {0}", // recovery.ts: this.#log
  164: "{0}: {1} call(s) in flight failed, {2} object(s) went stale", // recovery.ts: this.#log
  165: "the onCoreRestarted handler threw: {0}", // recovery.ts: this.#log
  166: "re-creating a query handle after a restart (type 0x{0})", // recovery.ts: core.report
  167: "observing a re-created query handle after a restart", // recovery.ts: core.report
  168: "the core answered a stream call with a plain result", // stream-support.ts: UndraTransportError("protocol")
  169: "the core sent a truncated stream item", // stream-support.ts: UndraTransportError("protocol")
  170: "the core sent a malformed stream failure: {0}", // stream-support.ts: UndraTransportError("protocol")
  171: "the core sent stream flag {0}", // stream-support.ts: UndraTransportError("protocol")
  172: "could not grant stream credit", // stream.ts: UndraError("state")
  173: "adapters.{0} are ignored in wasm-worker mode: set them in LoadOptions.worker.ports", // transport/framed.ts: log
  174: "the connection to the core was lost ({0}); reconnecting", // transport/framed.ts: UndraTransportError("closed")
  175: "this platform has no WebSocket; pass `webSocket` to use another implementation", // transport/remote.ts: UndraTransportError("unsupported")
  176: "could not connect to {0}: {1}", // transport/remote.ts: UndraTransportError("handshake")
  177: "could not send Hello: {0}", // transport/remote.ts: UndraTransportError("handshake")
  178: "the core sent a text message; Undra speaks binary envelopes", // transport/remote.ts: UndraTransportError("protocol")
  179: "the connection to {0} closed (code {1}{2})", // transport/remote.ts: VariableDeclaration
  180: "{0} before the core answered Hello", // transport/remote.ts: UndraTransportError("handshake")
  181: "could not connect to {0}", // transport/remote.ts: UndraTransportError("handshake")
  182: "{0} did not complete the Hello exchange within {1} ms", // transport/remote.ts: UndraTransportError("timeout")
  183: "the connection is closed", // transport/remote.ts: ConditionalExpression
  184: "the core is not connected", // transport/remote.ts: ConditionalExpression
  185: "the core is not connected: reconnecting", // transport/remote.ts: ConditionalExpression
  186: "expected Hello from the core, got {0}", // transport/remote.ts: UndraTransportError("handshake")
  187: "bad Hello from the core: {0}", // transport/remote.ts: UndraTransportError("handshake")
  188: "bad message from the core: {0}", // transport/remote.ts: UndraTransportError("protocol")
  189: "mode 'remote' needs the `url` option", // transport/remote.ts: UndraError("options")
  190: "cannot send a {0} message to a wasm core", // transport/wasm-main-transport.ts: UndraTransportError("protocol")
  191: "the core refused the call (undra_call returned {0})", // transport/wasm-main.ts: encodeValue
  192: "undra_alloc({0}) returned 0 instead of trapping", // transport/wasm-main.ts: Error
  193: "WebAssembly is not available on this platform", // transport/wasm-main.ts: UndraTransportError("unsupported")
  194: "GET {0} answered {1}", // transport/wasm-main.ts: Error
  195: "could not instantiate the wasm core: {0}", // transport/wasm-main.ts: UndraTransportError("handshake")
  196: "the module is not an Undra core: it does not export {0}", // transport/wasm-main.ts: UndraTransportError("handshake")
  197: "the core speaks wasm ABI {0}, this runtime speaks {1}", // transport/wasm-main.ts: UndraTransportError("handshake")
  198: "undra_init failed with code {0}", // transport/wasm-main.ts: UndraTransportError("handshake")
  199: "the core handed out [{0}, {1}) beyond its {2} bytes of memory", // transport/wasm-main.ts: RangeError
  200: "the core returned a null UndraBuf", // transport/wasm-main.ts: UndraTransportError("protocol")
  201: "the core is not started", // transport/wasm-main.ts: ConditionalExpression; transport/wasm-worker.ts: ConditionalExpression
  202: "the wasm core trapped: {0}", // transport/wasm-main.ts: UndraTransportError("trap")
  203: "the core does not export undra_snapshot", // transport/wasm-main.ts: UndraTransportError("unsupported")
  204: "the core does not export undra_restore", // transport/wasm-main.ts: UndraTransportError("unsupported")
  205: "the worker gave an incomplete answer", // transport/wasm-worker.ts: UndraTransportError("protocol")
  206: "worker.ports needs this @undra/runtime's worker script (protocol 3)", // transport/wasm-worker.ts: UndraTransportError("unsupported")
  207: "the worker sent an `envelopes` message without a list of envelopes", // transport/wasm-worker.ts: UndraTransportError("protocol")
  208: "worker error: {0}", // transport/wasm-worker.ts: UndraTransportError
  209: "the worker sent a message that could not be deserialised", // transport/wasm-worker.ts: UndraTransportError("protocol")
  210: "the worker did not start within {0} ms", // transport/wasm-worker.ts: UndraTransportError("timeout")
  211: "could not reach the worker: {0}", // transport/wasm-worker.ts: UndraTransportError("handshake"); transport/wasm-worker.ts: UndraTransportError("closed")
  212: "the worker script cannot {0}: rebuild it with this @undra/runtime", // transport/wasm-worker.ts: UndraTransportError("unsupported")
  213: "this platform has no Worker; pass `worker` to run the core on one", // transport/wasm-worker.ts: UndraTransportError("unsupported")
  214: "bad message from the worker: {0}", // transport/wasm-worker.ts: UndraTransportError("protocol")
  215: "{0} is synchronous: in wasm-worker mode, register it in LoadOptions.worker.ports", // transport/wasm-worker.ts: ReturnStatement
  216: "mode 'wasm-worker' needs the `wasm` option", // transport/wasm-worker.ts: UndraError("options")
  217: "a decimal's scale is an integer from 0 to {0}, got {1}", // wire/decimal.ts: RangeError
  218: "a decimal's mantissa must fit a signed 128-bit integer", // wire/decimal.ts: RangeError
  219: "not a decimal: {0}", // wire/decimal.ts: SyntaxError
  220: "unknown envelope kind: {0}", // wire/envelope.ts: RangeError
  221: "envelope seq out of range: {0}", // wire/envelope.ts: RangeError
  222: "envelope schema hash out of range: {0}", // wire/envelope.ts: RangeError
  223: "raw length out of range: {0}", // wire/reader.ts: RangeError
  224: "handle index out of range: {0}", // wire/types.ts: RangeError
  225: "handle generation out of range: {0}", // wire/types.ts: RangeError
  226: "handle out of range: {0}", // wire/types.ts: RangeError
  227: "cannot convert an invalid Date to a Timestamp", // wire/types.ts: RangeError
  228: "timestamp {0} is outside the range of a Date", // wire/types.ts: RangeError
  229: "duration must be finite: {0}", // wire/types.ts: RangeError
  230: "duration exceeds i64 nanoseconds: {0} ms", // wire/types.ts: RangeError
  231: "invalid UUID \"{0}\": expected 8-4-4-4-12 hex digits (36 characters) and room for 16 bytes at offset {1} of {2}", // wire/types.ts: RangeError
  232: "need 16 bytes for a UUID at offset {0} of {1}", // wire/types.ts: RangeError
  233: "{0} out of range: {1}", // wire/writer.ts: RangeError
  234: "the worker could not import the ports module {0}: {1}", // worker.ts: UndraTransportError("handshake")
  235: "the default export of the ports module {0} must map port ids to implementations", // worker.ts: UndraTransportError("handshake")
  236: "the ports module {0} maps {1} to something that is not a port implementation ({ sync, methods }, as registerPort takes)", // worker.ts: UndraTransportError("handshake")
  237: "the worker's transport cannot snapshot or restore", // worker.ts: UndraTransportError("unsupported")
  238: "{0} port 0x{1} could not release what it held: {2}", // worker.ts: log
  239: "could not deliver a port reply: {0}", // worker.ts: log
  240: "internal error: {0}", // worker.ts: log
  241: "dropped a malformed envelope from the host: {0}", // worker.ts: log
  242: "the worker was not started with recovery", // worker.ts: UndraTransportError("unsupported")
  243: "the wasm core trapped ({0}) and was restarted from a snapshot {1} ms old", // recovery.ts: UndraCoreRestarted
  244: "keyed patch operation #{0} ({1}) index {2} is out of bounds for a list of length {3}", // wire/errors.ts: PatchError
  245: null, // retired: db/node-sqlite.ts says it in both flavours (data: a port error's field, which the core receives)
  246: "the transport passes {0} as calls but not {1}: a transport has all seven control methods (observe, release, cancel, streamCredit, event, timerFired, portReply) or none, and send frames them", // transport/framed.ts: UndraError("options")
};

/**
 * The text of the message `code` for `values`: its sentence with `{0}`, `{1}` replaced by the values as a template literal would
 * (the production flavour says the code and the values instead).
 *
 * @param code A literal of the table above: the call site is the code's only place in the source, so a test can find it.
 */
export function msg(code: number, ...values: readonly unknown[]): string {
  return (MESSAGES[code] as string).replace(/\{(\d+)\}/g, (_, index: string) => String(values[Number(index)]));
}

/** The message of a `WireError`: the sentence for its code and fields (the production flavour says the code and the fields). */
export function wireText(detail: WireErrorDetail): string {
  return `wire: ${describe(detail)}`;
}

/** The exports of an Undra core module (SPEC 7) that the in-process host calls. */
const REQUIRED_FUNCTIONS = [
  "undra_alloc",
  "undra_free",
  "undra_abi_version",
  "undra_schema_hash",
  "undra_init",
  "undra_call",
  "undra_call_sync",
  "undra_cancel",
  "undra_stream_credit",
  "undra_observe",
  "undra_release",
  "undra_port_reply",
  "undra_event",
  "undra_timer_fired",
  "undra_poll",
  "undra_buf_free",
  "undra_stats_json",
] as const;

/**
 * The exports a module must have to be an Undra core, by name (`memory` and every function of SPEC 7 the host calls): a development
 * check, one that says which one is missing. The production flavour asks only whether `undra_abi_version` is there, and lets the ABI
 * version check say the rest (a missing export then fails typed, as a trap of the first call that needs it).
 */
export function missingExports(exported: Readonly<Record<string, unknown>>): readonly string[] {
  return [
    ...(exported.memory instanceof WebAssembly.Memory ? [] : ["memory"]),
    ...REQUIRED_FUNCTIONS.filter((name) => typeof exported[name] !== "function"),
  ];
}

/** The sentence for each wire failure, as the Rust `WireError` of `undra-wire` renders it. */
function describe(d: WireErrorDetail): string {
  switch (d.code) {
    case "unexpected_eof":
      return `unexpected end of input at offset ${d.at}: needed ${d.needed} more byte${d.needed === 1 ? "" : "s"}`;
    case "invalid_utf8":
      return `invalid UTF-8 in string at offset ${d.at}`;
    case "invalid_tag":
      return `invalid ${d.ty} tag ${d.tag} at offset ${d.at}`;
    case "length_too_large":
      return `length ${d.len} at offset ${d.at} exceeds the available input`;
    case "trailing_bytes":
      return `${d.count} trailing byte${d.count === 1 ? "" : "s"} after the end of the message`;
    case "bad_magic":
      return "bad magic: envelope does not start with 554e4452";
    case "unsupported_version":
      return `unsupported wire version ${d.version}`;
    case "schema_mismatch":
      return `schema mismatch: expected 0x${d.expected.toString(16).padStart(16, "0")}, got 0x${d.got.toString(16).padStart(16, "0")}`;
    case "duplicate_key":
      return `duplicate map key at offset ${d.at}`;
    case "negative_duration":
      return `negative duration (${d.nanos} ns)`;
    case "unsafe_integer":
      return `integer ${d.value} at offset ${d.at} is outside the JS safe integer range`;
  }
}
