import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { UndraCore } from "./core.js";
import type { LazyList } from "./lazy.js";
import { type UndraClass, isUndraClass, openUndra } from "./lifetime.js";
import type { UndraObject } from "./object.js";
import type { Signal } from "./signal.js";

/*
 * `@undra/runtime/react` (docs/SPEC.md section 10.3): four hooks. `useSignal` reads a core signal
 * the way `useSyncExternalStore` wants it; `useUndra` owns the life of a store or object for a
 * component; `useLazyList` and `useLoadMore` are the two of ADR-043 (a paged list, an infinite
 * query). `react` is a peer dependency; nothing else in `@undra/runtime` imports it.
 */

export type { UndraClass } from "./lifetime.js";

const noop = (): void => {};

/**
 * The current value of a core signal; the component renders again when it changes.
 *
 * Reading is local: the core pushes every change into the signal through the mirror, so a render
 * never calls into the core. Built on `useSyncExternalStore`, so it does not tear in concurrent
 * rendering, and it works for server rendering: on the server (and while hydrating) it reads the
 * signal's value as it is, which for a store nothing has observed is its initial value.
 *
 * `null` or `undefined` give `undefined`, so the hook can be called before a store exists:
 *
 * ```tsx
 * const todos = useUndra(Todos);
 * const visible = useSignal(todos?.visible); // Todo[] | undefined
 * ```
 */
export function useSignal<T>(signal: Signal<T>): T;
export function useSignal<T>(signal: Signal<T> | null | undefined): T | undefined;
export function useSignal<T>(signal: Signal<T> | null | undefined): T | undefined {
  // Stable per signal: a new `subscribe` function makes React unsubscribe and subscribe again.
  const subscribe = useCallback(
    (onChange: () => void): (() => void) => (signal === null || signal === undefined ? noop : signal.subscribe(() => onChange())),
    [signal],
  );
  // `peek()` returns the same reference until the signal changes, as `getSnapshot` requires.
  const getSnapshot = useCallback((): T | undefined => signal?.peek(), [signal]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** What `useUndra` holds: the object, or why creating it failed, for the inputs it was created for. */
interface Held<S> {
  readonly inputs: readonly unknown[];
  readonly outcome: { readonly object: S } | { readonly error: unknown };
}

function sameInputs(a: readonly unknown[], b: readonly unknown[]): boolean {
  return a.length === b.length && a.every((value, index) => Object.is(value, b[index]));
}

/**
 * Creates a store or object when the component mounts and closes it when the component unmounts.
 *
 * ```tsx
 * function Counter() {
 *   const counter = useUndra(Counter);          // Counter | undefined: undefined until it exists
 *   const count = useSignal(counter?.count);
 *   if (counter === undefined) return null;
 *   return <button onClick={() => void counter.increment()}>{count}</button>;
 * }
 * ```
 *
 * The object is created in an effect, so it is `undefined` on the server, on the first render and
 * until the core has answered. If creating it fails, the error is thrown during render, where an
 * error boundary catches it. The component owns the object: two components that need the same state
 * should take one store as a prop (or from context) instead of each creating their own. In
 * `StrictMode` development builds, React mounts components twice, which creates the object twice and
 * closes the first.
 *
 * @param type A generated class (`Todos`). Its `create(core)` is called with `core`, default `UndraCore.shared`.
 * @param core The core to create the object in.
 */
export function useUndra<S extends UndraObject>(type: UndraClass<S>, core?: UndraCore): S | undefined;
/**
 * The same for an object whose `create` takes arguments (a query handle, for instance): `create`
 * is called again, and the previous object closed, whenever one of `deps` changes.
 *
 * ```tsx
 * const page = useUndra(() => TodosQueryHandle.create(pageNumber), [pageNumber]);
 * ```
 */
export function useUndra<S extends UndraObject>(create: () => Promise<S>, deps: readonly unknown[]): S | undefined;
export function useUndra<S extends UndraObject>(
  source: UndraClass<S> | (() => Promise<S>),
  second?: UndraCore | readonly unknown[],
): S | undefined {
  const [held, setHeld] = useState<Held<S> | undefined>(undefined);
  const inputs: readonly unknown[] = isUndraClass(source) ? [source, second] : ((second as readonly unknown[] | undefined) ?? []);
  // A fresh closure each render; the effect below only reads it when its inputs have changed.
  const create = isUndraClass(source) ? () => source.create(second as UndraCore | undefined) : source;
  useEffect(
    () =>
      openUndra(
        create,
        (object) => setHeld({ inputs, outcome: { object } }),
        (error) => setHeld({ inputs, outcome: { error } }),
      ),
    inputs,
  );
  // A result for other inputs belongs to an object that is being closed.
  if (held === undefined || !sameInputs(held.inputs, inputs)) return undefined;
  if ("error" in held.outcome) throw held.outcome.error;
  return held.outcome.object;
}

/** What {@link useLazyList} gives a component. */
export interface LazyListView<T> {
  /** The number of rows; the component renders again when it changes. */
  readonly length: number;
  /**
   * The row at `index`, or `undefined` while its page loads (reading it requests the page). A new function after
   * every arrival of rows, so a memoised row that takes it as a prop renders again when its page arrives.
   */
  getItem(index: number): T | undefined;
}

/**
 * A core `Lazy<T>` list for a component: its length, and a function that reads a row. The component renders again
 * when the length changes and when pages arrive or are replaced (`LazyList.revision`), and reading a row during
 * render is the normal use (it requests the page and returns `undefined` until it is there, never blocking).
 *
 * ```tsx
 * const { length, getItem } = useLazyList(library?.books);
 * const virtualizer = useVirtualizer({ count: length, getScrollElement: () => scroller.current, estimateSize: () => 48 });
 * return virtualizer.getVirtualItems().map(({ index }) => <Row key={index} book={getItem(index)} />);
 * ```
 *
 * Works with TanStack Virtual, react-window or a plain `for` loop. `null` or `undefined` give an empty view, so the
 * hook can be called before the store exists.
 */
export function useLazyList<T>(list: LazyList<T> | null | undefined): LazyListView<T> {
  const length = useSignal(list?.length) ?? 0;
  const revision = useSignal(list?.revision);
  // `revision` is not read inside: it makes the function a new one when rows arrive.
  const getItem = useMemo(() => (index: number): T | undefined => list?.get(index), [list, revision]);
  return useMemo(() => ({ length, getItem }), [length, getItem]);
}

/** What {@link useLoadMore} needs of a query: the signals and method of a generated infinite query handle (`FeedQuery`). */
export interface LoadMoreQuery {
  /** Whether the server has another page. */
  readonly hasNextPage: Signal<boolean>;
  /** Whether a next page is being fetched. */
  readonly fetchingNextPage: Signal<boolean>;
  /**
   * The error of the last fetch, when the handle has one (a generated query handle does): while it is set the hook does
   * not fetch by itself, so a server that keeps failing is not asked again for as long as the sentinel stays in view;
   * the app offers a retry (`fetchNextPage()`), and the next success clears it.
   */
  readonly error?: Signal<unknown>;
  /** Fetches the next page; a generated handle's never rejects (a failure is reported through `onError`). */
  fetchNextPage(): Promise<void>;
}

/** Options of {@link useLoadMore}. */
export interface LoadMoreOptions {
  /** `IntersectionObserver`'s `rootMargin` (default `"0px"`): `"400px"` starts fetching before the sentinel is on screen. */
  readonly rootMargin?: string;
}

/**
 * A ref callback for a sentinel element at the end of an infinite list: when it comes into view (an
 * `IntersectionObserver`), and the query has a next page and is not fetching one, the next page is fetched. When the
 * page arrives and the sentinel is still in view (a short list, a tall screen) the next one follows, until the sentinel
 * is pushed out of view or the pages run out. After a failed fetch (`query.error` set) it waits for the app's retry.
 *
 * ```tsx
 * const feed = useUndra(() => FeedQuery.create(filter), [filter]);
 * const posts = useSignal(feed?.data) ?? [];
 * const sentinel = useLoadMore(feed, { rootMargin: "400px" });
 * return <>{posts.map((p) => <PostRow key={p.id} post={p} />)}<div ref={sentinel} /></>;
 * ```
 *
 * The observer is created when the sentinel is attached, only while there is something to fetch, and disconnected
 * when it is detached, when the component unmounts or when the query changes. Where there is no
 * `IntersectionObserver` (server rendering, an old browser) the hook does nothing. `null` or `undefined` for
 * `query` do nothing until it exists.
 */
export function useLoadMore(query: LoadMoreQuery | null | undefined, options: LoadMoreOptions = {}): (node: Element | null) => void {
  const hasNextPage = useSignal(query?.hasNextPage) ?? false;
  const fetching = useSignal(query?.fetchingNextPage) ?? false;
  const failed = (useSignal(query?.error) ?? null) !== null;
  const [sentinel, setSentinel] = useState<Element | null>(null);
  const rootMargin = options.rootMargin ?? "0px";
  useEffect(() => {
    if (query === null || query === undefined || sentinel === null || !hasNextPage || fetching || failed) return;
    if (typeof IntersectionObserver !== "function") return;
    let asked = false;
    const observer = new IntersectionObserver(
      (entries) => {
        if (asked || !entries.some((entry) => entry.isIntersecting)) return;
        asked = true; // until `fetching` changes, which makes a new observer
        void query.fetchNextPage();
      },
      { rootMargin },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [query, sentinel, hasNextPage, fetching, failed, rootMargin]);
  return setSentinel;
}
