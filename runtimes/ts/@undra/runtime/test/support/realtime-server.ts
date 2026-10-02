/*
 * The shared local server of the real-time scenarios and failure-injection suites
 * (contract-tests/servers/realtime-server.mjs, ADR-047), started in this process. It is plain
 * JavaScript outside the package, so it is imported by URL and typed here.
 */

/** What the server recorded about one connection (its `/stats`). */
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
  readonly port: number;
  /** `http://127.0.0.1:<port>`. */
  readonly url: string;
  /** `ws://127.0.0.1:<port>`. */
  readonly wsUrl: string;
  stats(): { readonly connections: readonly ServedConnection[] };
  reset(): void;
  close(): Promise<void>;
}

const SERVER = new URL("../../../../../../contract-tests/servers/realtime-server.mjs", import.meta.url).href;

/** Starts the server on a free port of 127.0.0.1. */
export async function startRealtimeServer(): Promise<RealtimeServer> {
  const module = (await import(/* @vite-ignore */ SERVER)) as { startRealtimeServer(options?: { port?: number }): Promise<RealtimeServer> };
  return module.startRealtimeServer();
}

/** Stops `server` without waiting longer than a second for sockets a test left open. */
export async function stopRealtimeServer(server: RealtimeServer): Promise<void> {
  await Promise.race([server.close(), new Promise((resolve) => setTimeout(resolve, 1000))]);
}

/** The newest connection the server saw on `path`. */
export function lastOn(server: RealtimeServer, path: string): ServedConnection | undefined {
  return server
    .stats()
    .connections.filter((c) => c.path === path)
    .at(-1);
}

/** Polls `probe` every 10 ms until it is truthy, for at most `ms` (default 5 s, the scenarios' wait). */
export async function until<T>(what: string, probe: () => T | undefined | null | false, ms = 5000): Promise<T> {
  const deadline = Date.now() + ms;
  for (;;) {
    const value = probe();
    if (value !== undefined && value !== null && value !== false) return value;
    if (Date.now() > deadline) throw new Error(`timed out after ${ms} ms waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
