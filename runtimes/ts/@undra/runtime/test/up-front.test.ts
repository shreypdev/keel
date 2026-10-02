import { describe, expect, it } from "vitest";
import { codeImporters, exportedValues, namespaceMembersUsed, reachable, sourceFiles } from "./support/module-graph.js";

/*
 * What a page loads up front is what an app's entry reaches of the runtime by static imports (ADR-052, ADR-057: the hello page's
 * JavaScript is gated at 16,000 bytes gzipped, `scripts/web-size-runtime.mjs`). The modules below are code a hello page never
 * runs, so the core fetches each by a dynamic `import()` when it needs it, or only a transport or an app that wants it imports
 * it. One static import of any of them, from the core or from anything the core reaches, puts it in the first chunk, whole; and
 * so does a **re-export** from a module that has code of its own (ADR-057, "the module rule"): a bundler puts a module in the
 * first chunk when such a module re-exports it, even if only an on-demand chunk uses the name. A pure barrel (a file made only of
 * `export .. from` lines: `index.ts`, `wire/index.ts`) has no code of its own, so a name reached through one lands in the
 * module that defines it and nowhere else. This test is the cheap guard of both rules (the gate itself needs a build).
 */

/** What a hello app's entry uses: the core, the object and error base classes, the signal, the codecs. */
const ENTRY = ["core.ts", "object.ts", "call-error.ts", "signal.ts", "wire/codec.ts"];

/** Modules that must not be in the first chunk, and why. Each lever of ADR-057 adds the module it moved out. */
const STAYS_OUT: Readonly<Record<string, string>> = {
  "adapters/ports.ts": "the Diagnostics and Timer ports of a native core, and every port builder",
  "adapters/codecs.ts": "the codecs of the default ports",
  "adapters/standard.ts": "the default ports' implementations",
  "panic-report.ts": "the report of a trap, for an app with `onPanic` or `crashRecovery`",
  "core-extras.ts": "`stats`, `snapshot`, `restore` and `runInBackground`: each answers with a promise, so the first call loads them (ADR-057); a page window reads `background.pending` itself",
  "transport/wasm-snapshot.ts": "snapshot, restore and the twin of the in-process host: functions that load with what asks for them",
  "transport/remote.ts": "the remote transport (`mode: \"remote\"`)",
  "transport/wasm-worker.ts": "the worker transport (`mode: \"wasm-worker\"`)",
  "recovery.ts": "crash recovery, for an app that passes `crashRecovery()`",
  "wire/envelope.ts": "the envelope codec: only the framed transports (remote, the worker's) frame messages",
  "wire/session.ts": "the framed transports' session payloads (`Hello`, `Log`, `PortCall`)",
  "adapters/ids.ts": "`PortIds`, the names and hashes of every standard port: the first chunk spells the nine ids it needs as literals (`adapters/port-literals.ts`)",
  "fnv.ts": "the hash that `PortIds` is computed with",
  "wire/codecs-more.ts": "the fourteen codecs a hello page does not name (`codecs` is a namespace, ADR-057): a first-chunk module that names one puts it up front",
  "stream-support.ts": "the stream support: the generated entry of a schema with a stream imports it (`features: [streams]`); a core without it loads it at its first stream",
  "stream-feature.ts": "what a core without the feature imports at its first stream: `stream-support.ts` through a module nobody imports statically (no INEFFECTIVE_DYNAMIC_IMPORT warning in an app's build)",
  "stream.ts": "`StreamCall`, which only the stream support opens",
  "wire/kind.ts": "the message kinds: the core passes its control messages as calls, only framing transports and recovery need the numbers",
  "wire/framed-payloads.ts": "the encoders and decoders of the framed messages (`Observe`, `Release`, `Cancel`, `Call`, `Reply`, snapshots, ...)",
  "wire/lazy-payloads.ts": "the lazy list pages: only a schema with a lazy list loads them",
  "wire/stream-payloads.ts": "the stream failure: only the stream support reads it",
  "transport/framed.ts": "the adapter that frames the control messages of a send-only transport",
  "transport/wasm-main-transport.ts": "`WasmMainTransport`: the in-process host plus `send(kind, payload)`; `UndraCore.load` runs the host itself",
  "mirror-waiters.ts": "the promises behind `observe` of a core that answers later; an in-process core delivers inside `observe`",
  "errors-rare.ts": "`UndraRestoreError` and `UndraSessionLostError`: only a snapshot operation or a remote core throws them; the first chunk tells them by `kind`",
};

describe("what UndraCore loads up front", () => {
  const upFront = reachable(ENTRY);

  it("reaches the core's own modules (the test walks the right graph)", () => {
    for (const name of ["core.ts", "mirror.ts", "transport/wasm-main.ts", "adapters/default-ports.ts", "adapters/events.ts", "adapters/browser-events.ts", "panic.ts"]) {
      expect(upFront.has(name), name).toBe(true);
    }
  });

  it("does not reach what loads on demand", () => {
    for (const [name, what] of Object.entries(STAYS_OUT)) {
      expect(upFront.has(name), `${name} (${what}) must be loaded by a dynamic import()`).toBe(false);
    }
  });

  it("re-exports what stays out only from pure barrels: a module with code of its own that re-exports it puts it in the first chunk", () => {
    for (const name of Object.keys(STAYS_OUT)) {
      for (const importer of codeImporters(name)) {
        expect(upFront.has(importer), `${importer} imports or re-exports ${name}, and is itself up front`).toBe(false);
      }
    }
  });

  it("names only the core codecs up front: a `codecs.<name>` of the first chunk's modules that is not in codecs-core.ts puts codecs-more.ts there", () => {
    const named = namespaceMembersUsed(upFront, "codecs");
    expect([...named].filter((name) => !exportedValues("wire/codecs-core.ts").has(name))).toEqual([]);
    expect(named.size, "the first chunk names some codecs (the scan finds them)").toBeGreaterThan(0);
  });

  it("knows its own modules (a renamed or deleted module is dropped from the list here)", () => {
    const files = new Set(sourceFiles());
    for (const name of [...ENTRY, ...Object.keys(STAYS_OUT)]) expect(files.has(name), name).toBe(true);
  });
});
