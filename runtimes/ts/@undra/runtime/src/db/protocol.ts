import { DbErrorCodec } from "../adapters/codecs.js";
import { DbError, type DbExecuted, type DbRows, type DbValue } from "../adapters/types.js";
import { errorMessage } from "../platform.js";
import { decodeValue, encodeValue } from "../wire/index.js";
import type { DbAdapter, DbConnection } from "./binding.js";

/*
 * The Db worker protocol: a `DbAdapter` that lives in a worker (wa-sqlite over OPFS, whose
 * synchronous access handles exist only in dedicated workers) served to the main thread over
 * `postMessage`. Requests carry an id and are answered once, in any order; values cross by
 * structured clone (`bigint` and `Uint8Array` included) and a failure crosses as the encoded
 * `DbError` (SPEC 3.1), so it arrives as the same variant with the same fields.
 */

/** What the main thread asks. `conn` is the worker's id of an open connection. */
export type DbWorkerRequest =
  | { readonly t: "open"; readonly id: number; readonly name: string; readonly namespace?: string }
  | { readonly t: "execute" | "query"; readonly id: number; readonly conn: number; readonly sql: string; readonly params: readonly DbValue[] }
  | { readonly t: "script"; readonly id: number; readonly conn: number; readonly sql: string }
  | { readonly t: "close"; readonly id: number; readonly conn: number };

/** What the worker answers: the value (`open`: a connection id; `execute`: `DbExecuted`; `query`: `DbRows`; else `null`), or the encoded `DbError`. */
export type DbWorkerReply =
  | { readonly t: "ok"; readonly id: number; readonly value: number | DbExecuted | DbRows | null }
  | { readonly t: "error"; readonly id: number; readonly error: Uint8Array };

/** The main thread's end of a worker: a `Worker`, or one end of a `MessageChannel`. */
export interface DbWorkerLike {
  postMessage(message: DbWorkerRequest): void;
  addEventListener(type: "message" | "error" | "messageerror", listener: (event: Event) => void): void;
  removeEventListener(type: "message" | "error" | "messageerror", listener: (event: Event) => void): void;
  /** Ends the worker (`Worker.terminate`); optional. */
  terminate?(): void;
}

/** The worker's end: its global scope, or the other end of a `MessageChannel`. */
export interface DbWorkerScope {
  postMessage(message: DbWorkerReply): void;
  addEventListener(type: "message", listener: (event: Event) => void): void;
  removeEventListener(type: "message", listener: (event: Event) => void): void;
}

/** `error` as an encoded {@link DbError} (anything else as `Unavailable(<its text>)`). */
function encodeFailure(error: unknown): Uint8Array {
  return encodeValue(DbErrorCodec, error instanceof DbError ? error : new DbError.Unavailable(errorMessage(error)));
}

/**
 * Serves `adapter` on `scope` (the worker side): answers every request with its value or its typed
 * error. Returns the function that stops serving and closes the connections it opened.
 */
export function serveDb(scope: DbWorkerScope, adapter: DbAdapter): () => void {
  const connections = new Map<number, DbConnection>();
  let next = 1;
  const connection = (id: number): DbConnection => {
    const found = connections.get(id);
    if (found === undefined) throw new DbError.Unavailable(`the database worker has no connection ${id}`);
    return found;
  };
  const answer = async (request: DbWorkerRequest): Promise<number | DbExecuted | DbRows | null> => {
    switch (request.t) {
      case "open": {
        const opened = await adapter.open(request.name, request.namespace === undefined ? undefined : { namespace: request.namespace });
        const id = next++;
        connections.set(id, opened);
        return id;
      }
      case "execute":
        return connection(request.conn).execute(request.sql, request.params);
      case "query":
        return connection(request.conn).query(request.sql, request.params);
      case "script":
        await connection(request.conn).executeScript(request.sql);
        return null;
      case "close": {
        const found = connections.get(request.conn);
        connections.delete(request.conn);
        await found?.close();
        return null;
      }
    }
  };
  const onMessage = (event: Event): void => {
    const request = (event as MessageEvent).data as DbWorkerRequest;
    answer(request).then(
      (value) => {
        scope.postMessage({ t: "ok", id: request.id, value });
      },
      (error: unknown) => {
        scope.postMessage({ t: "error", id: request.id, error: encodeFailure(error) });
      },
    );
  };
  scope.addEventListener("message", onMessage);
  return () => {
    scope.removeEventListener("message", onMessage);
    for (const open of connections.values()) void open.close();
    connections.clear();
  };
}

interface Waiting {
  resolve(value: unknown): void;
  reject(error: unknown): void;
}

/**
 * A {@link DbAdapter} that runs every call in the worker `worker` serves with {@link serveDb}
 * (the main-thread side). The worker is made by `create` on the first `open`; if it fails (its
 * script did not load, it threw), every waiting and later call is `Unavailable`.
 */
export function workerDbAdapter(create: () => DbWorkerLike): DbAdapter {
  let worker: DbWorkerLike | null = null;
  let broken: DbError | null = null;
  let nextId = 1;
  const waiting = new Map<number, Waiting>();

  const fail = (why: string): void => {
    broken ??= new DbError.Unavailable(why);
    for (const pending of waiting.values()) pending.reject(broken);
    waiting.clear();
  };

  const start = (): DbWorkerLike => {
    if (worker !== null) return worker;
    const made = create();
    made.addEventListener("message", (event) => {
      const reply = (event as MessageEvent).data as DbWorkerReply;
      const pending = waiting.get(reply.id);
      if (pending === undefined) return;
      waiting.delete(reply.id);
      if (reply.t === "ok") pending.resolve(reply.value);
      else pending.reject(decodeValue(DbErrorCodec, reply.error));
    });
    made.addEventListener("error", (event) => {
      const message = (event as Partial<ErrorEvent>).message;
      fail(`the database worker failed${typeof message === "string" && message !== "" ? `: ${message}` : ""}`);
    });
    made.addEventListener("messageerror", () => {
      fail("the database worker sent a message that could not be read");
    });
    worker = made;
    return made;
  };

  const ask = <T>(request: (id: number) => DbWorkerRequest): Promise<T> => {
    if (broken !== null) return Promise.reject(broken);
    let target: DbWorkerLike;
    try {
      target = start();
    } catch (error) {
      fail(`the database worker could not start: ${errorMessage(error)}`);
      return Promise.reject(broken);
    }
    const id = nextId++;
    return new Promise<T>((resolve, reject) => {
      waiting.set(id, { resolve: resolve as (value: unknown) => void, reject });
      try {
        target.postMessage(request(id));
      } catch (error) {
        waiting.delete(id);
        reject(new DbError.Unavailable(`the database worker cannot take the call: ${errorMessage(error)}`));
      }
    });
  };

  return {
    async open(name, scope) {
      const conn = await ask<number>((id) => ({ t: "open", id, name, ...(scope !== undefined && { namespace: scope.namespace }) }));
      let closed = false;
      return {
        execute: (sql, params) => ask<DbExecuted>((id) => ({ t: "execute", id, conn, sql, params })),
        query: (sql, params) => ask<DbRows>((id) => ({ t: "query", id, conn, sql, params })),
        executeScript: (sql) => ask<null>((id) => ({ t: "script", id, conn, sql })).then(() => undefined),
        async close() {
          if (closed) return;
          closed = true;
          await ask<null>((id) => ({ t: "close", id, conn })).catch(() => {});
        },
      };
    },
  };
}
