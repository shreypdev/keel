import { afterEach, describe, expect, it, vi } from "vitest";
import { PortIds } from "../src/adapters/ids.js";
import { PortStatus, UndraWriter } from "../src/wire/index.js";
import { type HelloPayload } from "../src/wire/index.js";
import type { TransportHandler } from "../src/transport/transport.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, track } from "./support/harness.js";

/*
 * ts-size-e4's review: an explicit `Timer` adapter on a `remote` core is served by a port the runtime builds from
 * `adapters/ports.js`, which is a dynamic import now (ADR-052). The port has to be there before the first message
 * after the Hello can arrive: a native core may arm a timer the moment it is connected.
 */

afterEach(() => {
  vi.doUnmock("../src/adapters/ports.js");
  vi.resetModules();
});

/** A remote fake that, as a core does the moment it is connected, calls `Timer.set` on the macrotask after its Hello. */
class EagerCore extends FakeCoreTransport {
  /** The reply to the first `Timer.set`, once the core has made the call (`called` settles when it has). */
  firstReply: Promise<{ status: number }> | undefined;
  readonly called: Promise<void>;
  #made!: () => void;
  constructor(...args: ConstructorParameters<typeof FakeCoreTransport>) {
    super(...args);
    this.called = new Promise((resolve) => {
      this.#made = resolve;
    });
  }
  override start(handler: TransportHandler): Promise<HelloPayload> {
    const hello = super.start(handler);
    setTimeout(() => {
      const w = new UndraWriter();
      w.writeU32(0x8000_0001);
      w.writeU64(10n);
      this.firstReply = this.callPort(PortIds.Timer.portId, PortIds.Timer.set, w.finish());
      this.#made();
    }, 0);
    return hello;
  }
}

describe("the Timer port of a remote core with an explicit timer adapter", () => {
  it("is registered before the core can call it, though the module that builds it loads on demand", async () => {
    vi.resetModules();
    vi.doMock("../src/adapters/ports.js", async (importOriginal) => {
      await new Promise((resolve) => setTimeout(resolve, 40)); // the chunk takes a while to arrive
      return importOriginal();
    });
    const { UndraCore } = await import("../src/core.js");
    const armed: number[] = [];
    const fake = new EagerCore({ mode: "remote" });
    track(
      await UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log: captureLog(), http: null, timer: { set: (id) => void armed.push(id) } },
      }),
    );
    await fake.called; // the call is made on a macrotask of its own: wait for it, not for a time
    const reply = await (fake.firstReply as Promise<{ status: number }>);
    expect(reply.status, "the core's first Timer.set was answered, not refused as unavailable").toBe(PortStatus.Ok);
    expect(armed).toEqual([0x8000_0001]);
  });
});
