import { loadNative, type NativeTransport } from '@undra/react-native';
import type { KvAdapter, UndraCore } from '@undra/runtime';
import { BigList, Todos, UndraIds } from '@playground/core';

/** What the screens share: the core and its two long-lived stores. */
export interface Playground {
  /** The native core, through @undra/react-native. */
  readonly core: UndraCore;
  /** The to-do list. */
  readonly todos: Todos;
  /** The 10,000-row list. */
  readonly bigList: BigList;
  /** Milliseconds from `loadNative` to the stores being observed. */
  readonly startupMs: number;
}

/** Where the app's evidence lines go: the device log (`UNDRA-RN ...`) and the Bench screen. */
export type Log = (line: string) => void;

/**
 * The `Kv` port in memory, as in the web playground (`../web/src/memory-kv.ts`): React Native's core
 * has no key-value store, and an app supplies its own (docs/REACT_NATIVE.md).
 */
export function memoryKv(): KvAdapter {
  const entries = new Map<string, Uint8Array>();
  return {
    get: key => Promise.resolve(entries.get(key)?.slice() ?? null),
    set: (key, value) => {
      entries.set(key, value.slice());
      return Promise.resolve();
    },
    delete: key => {
      entries.delete(key);
      return Promise.resolve();
    },
    list: prefix =>
      Promise.resolve([...entries.keys()].filter(key => key.startsWith(prefix)).sort()),
  };
}

/** Loads the native core (it is linked into the app) and creates the stores the screens show. */
export async function startUndra(log: Log): Promise<Playground> {
  const started = performance.now();
  const core = await loadNative({
    expectedSchemaHash: UndraIds.schemaHash,
    adapters: { kv: memoryKv() },
    onError: error => log(`UNDRA-RN error ${String(error)}`),
  });
  log(
    `UNDRA-RN loaded mode=${core.mode} platform=${core.hello.platform} schema=0x${core.hello.schemaHash.toString(16)} hermes=${String(
      typeof (globalThis as { HermesInternal?: unknown }).HermesInternal === 'object',
    )}`,
  );
  const [todos, bigList] = await Promise.all([Todos.create(core), BigList.create(core)]);
  const startupMs = performance.now() - started;
  log(
    `UNDRA-RN stores observed: todos=${todos.todos.get().length} biglist rows=${bigList.items.get().length} count=${bigList.count.get()} in ${startupMs.toFixed(1)} ms`,
  );
  return { core, todos, bigList, startupMs };
}

/** The transport's native counters, when the core is the native one. */
export function nativeCounters(core: UndraCore): ReturnType<NativeTransport['counters']> | null {
  const native = (globalThis as { __undraNative?: { hostCounters(): ReturnType<NativeTransport['counters']> } }).__undraNative;
  return core.mode === 'native' && native !== undefined ? native.hostCounters() : null;
}
