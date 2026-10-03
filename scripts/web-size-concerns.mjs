// The concerns of scripts/web-size-attribute.mjs: rules over (module, declaration) of the TypeScript runtime's first chunk.
// The first rule that matches wins. A declaration no rule names is printed as unassigned by the script, so a module that
// moves or a member that is renamed shows up there instead of silently changing a row: add or adjust a rule then.

const ERRORS = "error classes and messages (the UndraError family, the UndraCallError mapping)";
const WIRE = "wire: reader, writer, codecs, value types";
const WIRE_CONTROL = "wire: control payloads encoded, then decoded again in process (observe, release, cancel, credit, event, timer)";
const WIRE_SESSION = "wire: the framed transports' session payloads (Hello, Log, PortCall)";
const MIRROR = "mirror: queue, fold, drain, compaction, observe waiters, counters";
const PATCHES = "mirror: keyed patches (decodePatch, applyPatch, PatchError)";
const SIGNALS = "signals (Signal, batch)";
const CALLS = "call path (call, callSync, the direct call, the header, pending calls, replies)";
const STREAMS = "streams (StreamCall, stream items, credit, failures)";
const HOST = "transport: the in-process wasm host (WasmMainTransport)";
const SNAPSHOT = "snapshot, restore and the recovery hooks";
const STATS = "stats (UndraStats, the background and panic counters)";
const BACKGROUND = "background window (the run at pagehide and freeze)";
const PANICS = "panic-report trigger";
const REMOTE = "what only a core outside this thread needs, in the core (reconnect, connection-down, dev notice, timer port)";
const REPORT = "error channel (report, onError, log)";
const PORTS = "ports: dispatch, the lazy default ports, port ids (fnv), host events (Connectivity, Lifecycle)";
const OBSERVE = "observe and release bookkeeping";
const LIFECYCLE = "core lifecycle (load, options, start, the placeholder, close)";
const IDENTITY = "object identity (adopt, collected, the finalizer, the object base class)";
const STORE = "store base class (register, observe, close) and the chunk's export list";
const NAMESPACE = "namespace validation";
const SYSTEM = "system adapters (console log, clock, random, timers, the WebCrypto check)";
const BUNDLER = "bundler: Vite's preload helper for dynamic imports, and the chunk's table of on-demand chunks";

const rules = [
  [/^\(vite preload helper\)|^\(bundler/, /.*/, BUNDLER],

  // ADR-057: the production messages, the call header, the on-demand loader and the lazy stream path of a core without the feature.
  [/^src\/messages(\.prod)?\.ts$/, /.*/, ERRORS],
  [/^src\/call-head\.ts$/, /.*/, CALLS],
  [/^src\/on-demand\.ts$/, /.*/, LIFECYCLE],
  [/^src\/core\.ts$/, /^UndraCore\._streamAfterLoad$/, STREAMS],
  [/^src\/core\.ts$/, /^backgroundPending$/, BACKGROUND],
  [/^dist\/object\.js$/, /^UndraStore\./, STORE],

  [/^src\/wire\/errors\.ts$/, /^PatchError/, PATCHES],
  [/^src\/(errors|errors-[a-z-]+|base-error|call-error)\.ts$|^src\/wire\/errors\.ts$/, /.*/, ERRORS],
  [/^src\/platform\.ts$/, /errorMessage/, ERRORS],

  [/^src\/wire\/payloads\.ts$/, /^(applyPatch|decodePatch|inBounds|PATCH_)/, PATCHES],
  [/^src\/wire\/payloads\.ts$/, /^(decode|encode)(Observe|Release|Cancel|StreamCredit|Event|TimerFired)$/, WIRE_CONTROL],
  [/^src\/wire\/payloads\.ts$/, /^(decodeStreamFailure|readStreamFailure|streamFailureReplyBody|StreamFlag)/, STREAMS],
  [/^src\/wire\/payloads\.ts$/, /^(encodePortReply|PortStatus)/, PORTS],
  [/^src\/wire\/payloads\.ts$/, /^(decodeChangeSet|readChangeEntry|ChangeOp|CHANGE_ENTRY_MIN|ALL_SIGNALS)/, MIRROR],
  [/^src\/wire\/payloads\.ts$/, /^(encodeCall|CallTarget|ReplyStatus)/, CALLS],
  [/^src\/wire\/session\.ts$/, /.*/, WIRE_SESSION],
  [/^src\/wire\/kind\.ts$/, /.*/, HOST],
  [/^src\/wire\//, /.*/, WIRE],

  [/^src\/mirror(-[a-z]+)?\.ts$/, /.*/, MIRROR],
  [/^src\/signal\.ts$/, /.*/, SIGNALS],

  [/^src\/transport\/wasm-main\.ts$/, /\.(snapshot|takeSnapshot|restore|_restore|twin)$/, SNAPSHOT],
  [/^src\/transport\/wasm-main\.ts$/, /\.stats$/, STATS],
  [/^src\/transport\/wasm-main\.ts$/, /.*/, HOST],
  [/^src\/panic\.ts$/, /.*/, PANICS],
  [/^src\/platform\.ts$/, /.*/, HOST],

  [/^src\/identity\.ts$/, /.*/, IDENTITY],
  [/^src\/object\.ts$/, /^(leaks|DISPOSE|UndraObject|_rebindObject)/, IDENTITY],
  [/^src\/object\.ts$/, /.*/, STORE],
  [/^src\/stream(-[a-z]+)?\.ts$/, /.*/, STREAMS],

  [/^src\/(port-dispatch|fnv)\.ts$/, /.*/, PORTS],
  [/^src\/adapters\/(ids|port-[a-z]+|default-ports|events|browser-events)\.ts$/, /.*/, PORTS],
  [/^src\/adapters\/names\.ts$/, /.*/, NAMESPACE],
  [/^src\/adapters\/system\.ts$/, /.*/, SYSTEM],
  [/^src\/callid\.ts$/, /.*/, CALLS],

  [
    /^src\/core\.ts$/,
    /^(UndraCore\.(callSync|call|_callDirect|_send|_request|_onReply|construct|_allocCallId|_fromHandler|_startedByHandler|_assertOpen)|DirectCall|writeHead|putHandle|put32|encodeTarget|encodeConstructor|replyBody|abortReason|HEAD_LEN|HANDLE_HALVES|handle(Keys|Lo|Hi|Next))/,
    CALLS,
  ],
  [/^src\/core\.ts$/, /^UndraCore\.(stream|_openStream|_onStreamItem|_streams)$/, STREAMS],
  [/^src\/core\.ts$/, /^UndraCore\.stats$/, STATS],
  [/^src\/core\.ts$/, /^(UndraCore\.(runInBackground|_backgroundWindow)|PAGE_BACKGROUND_MS)$/, BACKGROUND],
  [/^src\/core\.ts$/, /^UndraCore\.(_loadPanics|_panicReport|_lost)$/, PANICS],
  [/^src\/core\.ts$/, /^UndraCore\.(snapshot|restore|_recreatable)$/, SNAPSHOT],
  [/^src\/core\.ts$/, /^(UndraCore\.(_reconnecting|_reconnected|_isConnectionDown|_devNotice|connection|_setConnection|_notifyConnection|timerFired)|DEV_NOTICE_TARGET)$/, REMOTE],
  [/^src\/core\.ts$/, /^UndraCore\.(report|_hand|_reportError|_log)$/, REPORT],
  [/^src\/core\.ts$/, /^UndraCore\.(registerPort|_onPortCall|_sendPortReply|event)$/, PORTS],
  [/^src\/core\.ts$/, /^UndraCore\.(observe|release|_giveBack|_held|_noteObserved|_resync)$/, OBSERVE],
  [
    /^src\/core\.ts$/,
    /^(UndraCore\.(load|attach|_attach|_start|constructor|_placeholder|shared|current|unloaded|namespace|mode|closed|hello|close|_dispose|_failInFlight|_lostForGood|_handler|\(fields\))|mergeAdapters|UNLOADED_MESSAGE|DEFAULT_OBSERVE_TIMEOUT_MS|\(imports\)|\(module\))/,
    LIFECYCLE,
  ],
];

/** The concern of `declaration` of `module` (as the attribution script names them), or `undefined` when no rule names it. */
export function classify(module, declaration) {
  for (const [modulePattern, declarationPattern, concern] of rules) {
    if (modulePattern.test(module) && declarationPattern.test(declaration)) return concern;
  }
  return undefined;
}
