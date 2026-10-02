import { afterEach, describe, expect, it, vi } from "vitest";

/*
 * The two flavours of the package (ADR-057, D1 and D2): the production build swaps `messages.js` for `messages.prod.js`, so a message is
 * a code, its values and a link; everything else is the same. This test builds both runtimes from the sources (the same modules, one
 * with the swap) and drives a sample of every error the runtime throws on purpose: for each, the class, `name`, `kind` and every field
 * are identical and only `message` differs (R6: nothing a program can branch on changes). `dist-flavour.test.ts` holds the same of
 * the built package.
 */

type Runtime = typeof import("../src/index.js");

afterEach(() => {
  vi.doUnmock("../src/messages.js");
  vi.resetModules();
});

async function runtime(flavour: "development" | "production"): Promise<Runtime> {
  vi.resetModules();
  if (flavour === "production") vi.doMock("../src/messages.js", async () => ({ ...(await import("../src/messages.prod.js")) }));
  else vi.doUnmock("../src/messages.js");
  return import("../src/index.js");
}

/** What a scenario raises, as an error (a rejection or a throw). */
type Scenario = (rt: Runtime) => unknown | Promise<unknown>;

const caught = async (run: () => unknown): Promise<unknown> => {
  try {
    return await run();
  } catch (error) {
    return error;
  }
};

const scenarios: Readonly<Record<string, Scenario>> = {
  "RangeError: a UUID that is not one": (rt) => caught(() => rt.encodeUuid("nope")),
  "RangeError: a u8 out of range": (rt) => caught(() => new rt.UndraWriter().writeU8(300)),
  "RangeError: a decimal scale": (rt) => caught(() => new rt.Decimal(1n, 99)),
  "SyntaxError: not a decimal": (rt) => caught(() => rt.Decimal.parse("x")),
  "WireError: unexpected end of input": (rt) => caught(() => rt.decodeValue(rt.codecs.u32, new Uint8Array(1))),
  "WireError: trailing bytes": (rt) => caught(() => rt.decodeValue(rt.codecs.u8, new Uint8Array(3))),
  "WireError: bad magic": (rt) => caught(() => rt.decodeEnvelope(new Uint8Array(23))),
  "WireError: invalid tag": (rt) => caught(() => rt.decodeValue(rt.codecs.bool, Uint8Array.of(7))),
  "PatchError: out of bounds": (rt) => caught(() => rt.applyPatch([1], [{ op: "remove", index: 5 }])),
  "UndraModeError": (rt) => new rt.UndraModeError("callSync", "remote"),
  "UndraSchemaMismatchError": (rt) => new rt.UndraSchemaMismatchError(1n, 2n),
  "UndraRestoreError": (rt) => new rt.UndraRestoreError(5),
  "UndraSessionLostError": (rt) => new rt.UndraSessionLostError(),
  "UndraTransportError: the core is closed": (rt) => caught(() => rt.UndraCore.unloaded.callSync(rt.CallTarget.FreeFunction, 1, new Uint8Array(0))),
  "UndraError: a mode without its option": (rt) => caught(() => rt.UndraCore.load({ mode: "wasm-main", expectedSchemaHash: 1n })),
  "UndraError: an unknown mode": (rt) => caught(() => rt.UndraCore.load({ mode: "nope" as "remote", expectedSchemaHash: 1n })),
  "UndraError: a namespace that is not one": (rt) =>
    caught(() => rt.UndraCore.load({ mode: "remote", url: "ws://127.0.0.1:1", expectedSchemaHash: 1n, namespace: "A/b" })),
  "UndraError: a handle registered twice": (rt) =>
    caught(() => {
      const mirror = new rt.Mirror();
      mirror.register(1n, () => {});
      mirror.register(1n, () => {});
    }),
  "UndraCallError.Panicked": (rt) => new rt.UndraCallError.Panicked("boom", "at core::foo"),
  "UndraCallError.Refused": (rt) => new rt.UndraCallError.Refused("no such handle"),
  "UndraCallError.CancelledByCore": (rt) => new rt.UndraCallError.CancelledByCore(),
  "UndraCallError.Unavailable": (rt) => new rt.UndraCallError.Unavailable(new rt.UndraTransportError("closed", "gone")),
  "UndraCallError.Malformed": (rt) => new rt.UndraCallError.Malformed("a typed error that does not decode"),
  "UndraCallError.mapped of a reply": (rt) => rt.UndraCallError.mapped(new rt.UndraReplyError(rt.ReplyStatus.Cancelled, new Uint8Array(0))),
  "UndraUnhandledError": (rt) => new rt.UndraUnhandledError("Todos.add", new rt.UndraCallError.Panicked("boom", ""), new Error("x")),
  "UndraReplyError": (rt) => new rt.UndraReplyError(rt.ReplyStatus.Panic, new Uint8Array(0)),
};

/** Every own property of an error that a program can read, as comparable text (`message` and `stack` aside). */
function fields(error: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  for (const key of Object.getOwnPropertyNames(error)) {
    if (key === "message" || key === "stack") continue;
    const value = (error as Record<string, unknown>)[key];
    out[key] = typeof value === "bigint" ? `${value}n` : value instanceof Error ? `${value.name}: ${value.message}` : typeof value === "object" ? JSON.stringify(value, (_k, v: unknown) => (typeof v === "bigint" ? `${v}n` : v)) : String(value);
  }
  return out;
}

describe("the development and the production flavour throw the same errors", () => {
  for (const [name, scenario] of Object.entries(scenarios)) {
    it(`${name}: the same class, name, kind and fields; only message differs`, async () => {
      const dev = await scenario(await runtime("development"));
      const prod = await scenario(await runtime("production"));
      expect(dev, "the development scenario raised an error").toBeInstanceOf(Error);
      expect(prod, "the production scenario raised an error").toBeInstanceOf(Error);
      const [d, p] = [dev as Error, prod as Error];
      expect(p.constructor.name).toBe(d.constructor.name);
      expect(p.name).toBe(d.name);
      expect((p as { kind?: string }).kind).toBe((d as { kind?: string }).kind);
      // A cause that is a runtime error says its own message in each flavour: compare what is not message text.
      const strip = (f: Record<string, string>): Record<string, string> => Object.fromEntries(Object.entries(f).filter(([key]) => key !== "cause" && key !== "error" && key !== "transport"));
      expect(strip(fields(p))).toEqual(strip(fields(d)));
      expect(Object.keys(fields(p)).sort()).toEqual(Object.keys(fields(d)).sort());
    });
  }

  it("says a sentence in the development build and a code with a link in the production build, for each of them that the runtime words itself", async () => {
    const dev = await runtime("development");
    const prod = await runtime("production");
    const link = /^(T\d{4}(: .*)? — |wire: code=\w+( \w+=\S+)* — )https:\/\/shreypdev\.github\.io\/undra\/docs\/errors\.html#(T\d{4}|wire-\w+)$/;
    let worded = 0;
    for (const [name, scenario] of Object.entries(scenarios)) {
      const d = (await scenario(dev)) as Error;
      const p = (await scenario(prod)) as Error;
      if (d.message === p.message) continue;
      worded++;
      expect(p.message, `${name}: production`).toMatch(link);
      expect(d.message, `${name}: development`).not.toMatch(/^T\d{4}\b/);
      expect(d.message.length, `${name}: development says a sentence`).toBeGreaterThan(10);
    }
    expect(worded, "most of the sample is worded by the runtime").toBeGreaterThan(15);
  });

  it("text that leaves the runtime as data is the same sentence in both: a port error's field (the core receives it) and a close frame's reason (the peer does)", async () => {
    /** The data texts of one flavour: public constants of `./realtime`, and the fields of port errors the adapters raise. */
    const texts = async (flavour: "development" | "production"): Promise<Record<string, unknown>> => {
      await runtime(flavour);
      const realtime = await import("../src/realtime.js");
      const db = await import("../src/db.js");
      const { fetchHttp } = await import("../src/adapters/http.js");
      const field = (error: unknown): Record<string, string> => fields(error);
      return {
        didNotKeepUp: realtime.DID_NOT_KEEP_UP,
        headersRefused: realtime.HEADERS_REFUSED,
        badDbName: await caught(() => db.validateDbName("..")).then(field),
        badMigration: await caught(() => db.validateMigrations([{ version: 2, sql: "" }, { version: 1, sql: "" }] as never)).then(field),
        // A platform without fetch: the HttpError the core receives.
        noFetch: await caught(() => fetchHttp({ fetch: "none" as never }).request({ url: "https://x.test/" } as never)).then(field),
      };
    };
    const dev = await texts("development");
    const prod = await texts("production");
    expect(dev.didNotKeepUp).toBe("the core did not keep up");
    expect(prod).toEqual(dev);
    expect(JSON.stringify(prod), "no T code in data").not.toMatch(/T\d{4}/);
  });

  it("a production message carries the core's own text as a value, the code as the key: a panic message travels as it is", async () => {
    const prod = await runtime("production");
    const panicked = new prod.UndraCallError.Panicked("index out of bounds: 7", "at core::foo");
    expect(panicked.panicMessage).toBe("index out of bounds: 7");
    expect(panicked.message).toMatch(/^T\d{4}: index out of bounds: 7 — https:\/\//);
    expect(panicked.kind).toBe("panicked");
  });
});
