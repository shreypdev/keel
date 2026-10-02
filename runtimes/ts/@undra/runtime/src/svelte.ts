import type { Readable } from "svelte/store";
import type { Signal } from "./signal.js";

/*
 * `@undra/runtime/svelte` (docs/SPEC.md section 10.3): a signal as a Svelte store. Nothing here
 * imports `svelte` at run time: a store is an object with `subscribe`, and the `Readable` type is
 * all that is taken from it.
 */

/**
 * A core signal as a readable Svelte store, for `$store` in a component or for `derived`.
 *
 * ```svelte
 * <script>
 *   import { signalStore } from "@undra/runtime/svelte";
 *   const visible = signalStore(todos.visible);
 * </script>
 * {#each $visible as todo} ... {/each}
 * ```
 *
 * The store contract wants the current value as soon as there is a subscriber, which `Signal`
 * itself does not do, so `subscribe` calls `run` with `peek()` first. The store is cheap: one
 * object, and a subscription to the signal per subscriber for as long as it lasts.
 *
 * A `LazyList` (ADR-043) needs nothing more: `signalStore(list.length)` is its row count and `signalStore(list.revision)`
 * changes when rows arrive, so a block that reads `list.get(i)` after `$revision` shows the rows as they come.
 */
export function signalStore<T>(signal: Signal<T>): Readable<T> {
  return {
    subscribe(run) {
      run(signal.peek());
      return signal.subscribe(run);
    },
  };
}
