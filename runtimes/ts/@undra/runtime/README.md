# @undra/runtime

The TypeScript runtime of Undra: the wire format, the transports to a core (wasm in the page, wasm in a worker, a
remote core), the reactive mirror, and adapters for React, Vue, Svelte and Solid. The bindings `undra bindgen` writes
for TypeScript import it; application code mostly imports the hooks and the signals.

```ts
import { useSignal, useUndra } from "@undra/runtime/react";
```

## Paged lists and infinite queries

A `Lazy<T>` field of a store is a list the core owns and the page reads a window of (ADR-043). The generated store
holds a `LazyList<T>`:

```tsx
import { useLazyList } from "@undra/runtime/react";

const { length, getItem } = useLazyList(library?.books);   // re-renders on `length` and when pages arrive
// getItem(i) is the row, or undefined while its page loads (reading it requests the page and one page each side)
const virtualizer = useVirtualizer({ count: length, getScrollElement: () => scroller.current, estimateSize: () => 48 });
```

`LazyList` is framework-free: `list.length` and `list.revision` are signals (`useSignal(list.length)` in React, Vue and
Solid, `signalStore(list.length)` in Svelte), `list.get(i)` reads a row, `list.prefetch(start, end)` warms a range,
`list.pageSize` (default 50) and `list.maxCachedPages` (default 24) are settable. Page requests of one turn go out
together, synchronously where the core runs in the page (and React Native), asynchronously otherwise; a change of the
list costs one request per page of the window, however long the list. Failures reach `onError`; `get` never throws.

A generated infinite query handle (`#[undra::query(infinite, ..)]`) has `data`, `hasNextPage`, `fetchingNextPage` and
`fetchNextPage()`. `useLoadMore` fetches the next page when a sentinel element scrolls into view:

```tsx
const feed = useUndra(() => FeedQuery.create(filter), [filter]);
const posts = useSignal(feed?.data) ?? [];
const sentinel = useLoadMore(feed, { rootMargin: "400px" });
return <>{posts.map((p) => <PostRow key={p.id} post={p} />)}<div ref={sentinel} /></>;
```

`LazyList` and the hooks are linked only by an app that uses them: a page without a `Lazy<T>` does not carry them.
