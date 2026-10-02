import { expect, test } from "vitest";
import { ALL_SIGNALS, PortIds, UndraError } from "@undra/runtime";
import { RemoteTodosQueryHandle, UndraIds, configureRemote, createRemoteTodo, localePortImpl, localizedGreeting } from "@playground/core";
import { BASE_URL, bootWorker } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { step, waitFor } from "../src/wait.js";

// S21 worker mode answers synchronous ports in the worker (ADR-049; TypeScript only): the playground core in
// `wasm-worker` mode, with `worker.ports` pointing at a module that implements the app's synchronous `Locale` port in
// the worker. The worker is `runWorker` on a MessageChannel in this thread (src/harness.ts, `bootWorker`).

const LIST = "s21";
const TODOS = `${BASE_URL}/lists/${LIST}/todos`;
const PORTS_MODULE = new URL("../src/locale-ports.ts", import.meta.url).href;
const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

test("S21 worker mode answers synchronous ports in the worker", async () => {
  const worker = await bootWorker({ workerPorts: PORTS_MODULE });
  const { core, server, log, closed } = worker;
  expect(core.mode).toBe("wasm-worker");
  await configureRemote({ baseUrl: BASE_URL }, core);

  await step("1. Clock, Rng and Log are answered in the worker: a query, two keyed mutations, a log record; nothing traps", async () => {
    server.on("GET", TODOS, replies.json(200, [{ id: 1, title: "one", done: false }]));
    let next = 10;
    server.on("POST", TODOS, (request) => replies.json(201, { id: next++, title: (request.json() as { title: string }).title, done: false }));
    const handle = await RemoteTodosQueryHandle.create(LIST, core);
    await waitFor("the query to show the server's data", () => handle.data.peek()?.[0]?.id === 1);
    const updatedAt = handle.updatedAt.peek();
    expect(updatedAt, "the fetch was stamped").not.toBeNull();
    expect(Math.abs((updatedAt as number) - Date.now()), "stamped with the worker's clock, within a minute of now").toBeLessThan(60_000);

    await createRemoteTodo(LIST, "x", core);
    await createRemoteTodo(LIST, "y", core);
    const keys = server.requestsTo("POST", TODOS).map((request) => request.header("Idempotency-Key"));
    expect(keys).toHaveLength(2);
    for (const key of keys) expect(key, "a UUID v4 from the worker's crypto.getRandomValues").toMatch(UUID_V4);
    expect(keys[0], "two calls carry different keys").not.toBe(keys[1]);

    // A record the core writes (an observation of a handle it never issued is a warning) reaches this thread's Log.
    void core.observe(0x7777_7777_0000_0001n, ALL_SIGNALS, true).catch(() => {});
    await waitFor("the core's record to reach the Log adapter", () => log.find({ minLevel: 3, target: "undra::runtime" }).length > 0);
    expect(log.find({ minLevel: 5 }), "nothing fatal").toEqual([]);
    expect(core.closed).toBe(false);
    expect(closed).toEqual([]);
    handle.close();
  });

  await step("2. the app's synchronous Locale port answered in the worker", async () => {
    expect(await localizedGreeting("Ada", core)).toBe("Hola, Ada");
  });

  await step("3. Locale registered on the main thread in worker mode fails at load, names it and says to use worker.ports", async () => {
    const posted: unknown[] = [];
    const failure = await bootWorker({
      posted,
      ports: { [UndraIds.Ports.Locale.portId]: localePortImpl({ hello: () => "Hola" }) },
    }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure).toBeInstanceOf(UndraError);
    expect((failure as UndraError).kind).toBe("options");
    expect((failure as UndraError).message).toContain("Locale");
    expect((failure as UndraError).message).toContain("worker.ports");
    expect(posted, "no core was started: nothing was sent to the worker").toEqual([]);
  });

  await step("4. the worker protocol is version 3: init carries the host's asynchronous ports and the ports module", () => {
    const init = worker.posted[0] as { t: string; protocol: number; asyncPorts: number[]; portsModule: string };
    expect(init.t).toBe("init");
    expect(init.protocol).toBe(3);
    expect(init.asyncPorts).toEqual(expect.arrayContaining([PortIds.Http.portId, PortIds.Kv.portId]));
    expect(init.asyncPorts, "a synchronous port is never forwarded").not.toContain(UndraIds.Ports.Locale.portId);
    expect(init.portsModule).toBe(PORTS_MODULE);
  });
});
