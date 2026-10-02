import { afterAll, beforeAll } from "vitest";

/*
 * The shared local server of S23 and S24 (contract-tests/servers/realtime-server.mjs, ADR-047),
 * started in this process once per test file: `WS` is its `ws://127.0.0.1:<port>`, `HTTP` its
 * `http://127.0.0.1:<port>` (scenarios.md writes `WS/ws/echo`, `HTTP/sse/feed`).
 */

/** What the server recorded about one connection (`/stats`). */
export interface ServedConnection {
  readonly id: number;
  readonly path: string;
  readonly headers: Readonly<Record<string, string | undefined>>;
  readonly protocols: readonly string[];
  readonly closeCode: number | null;
  readonly closeReason: string | null;
  readonly written: number;
  readonly clientClosed: boolean;
}

/** A running realtime server. */
export interface RealtimeServer {
  readonly url: string;
  readonly wsUrl: string;
  stats(): { readonly connections: readonly ServedConnection[] };
  reset(): void;
  close(): Promise<void>;
}

const SERVER = new URL("../../servers/realtime-server.mjs", import.meta.url).href;

/** Starts the server on a free port. */
export async function startRealtimeServer(): Promise<RealtimeServer> {
  const module = (await import(/* @vite-ignore */ SERVER)) as { startRealtimeServer(): Promise<RealtimeServer> };
  return module.startRealtimeServer();
}

/**
 * Starts the server before the file's tests and stops it after them; returns a getter (the server
 * exists only once `beforeAll` ran).
 */
export function useRealtimeServer(): () => RealtimeServer {
  let server: RealtimeServer | undefined;
  beforeAll(async () => {
    server = await startRealtimeServer();
  });
  afterAll(async () => {
    // Sockets a failed scenario left open must not hold the run: a second is plenty for a clean close.
    if (server !== undefined) await Promise.race([server.close(), new Promise((resolve) => setTimeout(resolve, 1000))]);
  });
  return () => {
    if (server === undefined) throw new Error("the realtime server is not running");
    return server;
  };
}

/** The newest connection the server saw on `path`. */
export function lastOn(server: RealtimeServer, path: string): ServedConnection | undefined {
  return server
    .stats()
    .connections.filter((c) => c.path === path)
    .at(-1);
}
