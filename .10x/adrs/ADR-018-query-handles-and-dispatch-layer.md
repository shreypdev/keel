# ADR-018: query handles, mutation calls and the runtime's dispatch layer

Status: accepted (implemented on `wt/keel-query`)
Touches: `keel-runtime` dispatch (SPEC 5.6), `keel-macros` query/mutation output, `keel-query`.

## Context

`keel-bindgen` synthesizes, for every `#[keel::query]` `todos(page)`, a `TodosQueryHandle` object
whose type id and constructor method id are the query id, with two methods (`refetch`,
`invalidate`, the same ids on every handle) and five signals; and for every `#[keel::mutation]`
an async free function whose method id is the mutation id. The three platform runtimes and the
generated code are shipped, so the Rust side has to serve exactly that.

The runtime routes a call through a table built from `keel_meta::registrations()`: one
`ObjectMeta` per object type, one `FunctionMeta` per function. A handle type per query and a
function per mutation cannot be expressed there:

* they exist only as generic instantiations (`QueryHandle<Q>`), so no macro can emit a static
  `ObjectMeta` without also emitting a dispatcher per query, and
* registering them as objects/functions would put them in the schema a second time (bindgen
  already synthesizes them from `QueryMeta`), which fails validation and changes the schema hash.

## Decision

1. **`keel_runtime::DispatchLayer`** (inventory): a call the static table cannot route is offered to
   each layer, which answers `Unknown` for ids it does not serve. `keel-query` registers one layer;
   it finds the query or mutation by id through the new `QueryRegistration` /
   `MutationRegistration` that `#[keel::query]` / `#[keel::mutation]` submit next to their
   `QueryMeta` (one extra `inventory::submit!` each; no schema change).
2. **`QueryHandle<Q>` is hand-written, not `#[keel::store]`** (see the module docs of
   `keel-query::handle`): typed signals (`Signal<Option<Q::Output>>`, ...) attached to a `StoreCell`
   in id order 0..4, inserted in the object table as an `AnyObject` whose type id is the query id.
3. **`KeelObjectDyn::transient()`**: query handles are views of the cache; `snapshot()` leaves
   them out (otherwise a snapshot with an open handle could not be restored: no `StoreRestorer`).
   Their handles are stale after a restore, like any object that is not a store (SPEC 5.9).
4. `Ctx::query()` returns an owned `QueryClient` bound to the `Ctx` rather than `&QueryClient`
   (SPEC 5.3): the cache lives in `Runtime::extension`, and a runtime that owned its own `Ctx`
   would never be freed.

## Choices inside the spec's gaps

* `status` is derived: `Fetching` only while a fetch runs and there is no data yet; a refetch of an
  entry with data stays `Success` with `fetching = true` (a screen keeps its content).
* Cache keys are rendered (`todos:{page}` with the parameters spliced in, from the schema's
  parameter names) so `invalidate("todos:3")` and a mutation's `key = "todo:{id}"` work; values
  render as plain text.
* Persisted keys are `keel.query.cache.<query_id:08x>.<fnv1a64(params):016x>`; the queue's value
  starts with the schema hash, an addition to the spec's field list (arguments encoded by another
  schema must never be replayed).
* A network failure is recognised by walking the encoded error by its schema type for
  `HttpError::Network` (the app's own error usually wraps it), or by downcast when the error is
  `HttpError`.
* An idempotent mutation's key is a UUID v4 from the `Rng` port, made once per call and readable in
  the body through `keel_query::idempotency_key()` (a thread-local scoped to each poll).
* The client assumes it is online until told otherwise.

## Consequences

* `keel::query` becomes `keel-query`; the facade links `keel-ports` transitively, so test fixtures
  that reuse the standard port names (`Http`, `Clock`, ...) collide with it (the `keel-macros` test
  fixtures were renamed).
* `interval_ms` (SPEC 9) has no carrier in `QueryDef` or `QueryMeta`; timed refetching is not in the
  v1 contract.
