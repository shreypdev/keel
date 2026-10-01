# Architect: derived keyed lists (2026-10-01)

**Problem.** ADR-027 made a keyed list cost the change, but the first `Computed<Vec<T>>` over it costs the
list again: recomputed with a clone of every passing row, sent as a full value, decoded whole on every
platform. Measured (M5 Pro host, throwaway probes; `visible` = 75% of the rows, one title edit): core 17.8 us /
179 us / 2.7 ms and 41 KB / 413 KB / 4.1 MB per change at 1k / 10k / 100k rows (80% of it the clone), against
0.2 us and 93 bytes for the keyed source alone; platform apply of that full value at 10k: TypeScript 1.13 ms,
Swift 0.72-0.80 ms, Kotlin (JVM) 0.12 ms, against 0.2-1.6 us for a one-op patch.

**Decision (ADR-039, proposed).** `DerivedList<T>`: `source.derive().filter(..) / filter_with(&param, ..) /
map(..) / sort_by_key(..) / sort_by_key_with(..)`, then `.build()` (a store field, keyed) or `.count()` (a
`Computed<u32>`). Semantics: `stable_sort_by_key(filter_map(source))`, ties in source order. Maintained from the
source's recorded ops through per-list taps (the ADR-027 log generalised to several consumers), on an index of
two arena AVL order-statistic trees with parent pointers (positional with passing counts; sorted by
`(key, source position)`): O(log n) per source op, O(log^2 n) with heavy ties, removal never searches. One
source op emits at most two derived ops (a sort-key change is `Move` + `Update`); a transaction's ops are one
patch; an op that changes nothing in the view sends nothing. Raw writes and more than 4,096 pending ops
rebuild and send the full value; a parameter change re-walks the rows and sends what changed as at most 256
ops, else the full value (measured platform break-evens: Kotlin ~150, Swift ~300, TypeScript ~2,500). The schema and the
generated code do not change: a derived list is `computed: true` + `key`, a combination E0008 used to refuse,
which every generator already emits as a read-only keyed list (R3 holds; existing hashes pinned). Derived lists
and `Lazy<T>` stay separate in v1.2; the index is pageable for E3. Prototype of the index (throwaway): 256-588
ns per source op at 10k, 515-1,240 ns at 100k; 60,000 random ops on three views matched filter + stable sort
after every op.

**Rejected.** Diffing on the platform (does not touch the cost); diffing in the core (O(list) twice: recompute
plus a 435 us diff at 10k; kept for computeds that are not pipelines); materialised views through the query
layer or the `Db` port (not incremental, async, ties E2 to G3); a general incremental engine; `BTreeMap`
(no rank), treaps and skip lists (randomness, R12), order-maintenance labels (more code for what heavy ties do
not need); `Remove` + `Insert` for a key change (loses row identity); ties by key (scrambles equal keys); a
`derived` schema flag (no consumer, moves hashes).

**Cost.** New `undra-signals` API (`Derive`, `DerivedList`, `attach_derived`), one store field form, E0008
texts; the playground's `Todos` moves to recorded ops; contract grid 19 x 3 (S19); nine budget rows, two
ratios, one stress scenario. O(change) only for sources written with the recorded operations.

**Open for the integrator.** Parameters in v1 (recommended yes: the playground's `visible` needs them; closures
that read other signals stay out); names (`DerivedList<T>` vs `Derived<Vec<T>>`); the 4,096 / 256 caps;
`count()` in v1; tie order and `Move` + `Update` (recommended as written).

Brief: `.10x/specs/2026-10-01-derived-keyed-lists-impl.md`. Implementation waits for Track A (same files).

## Integrator decisions (2026-10-01)

ADR-039 is accepted in direction; it flips to Accepted with its implementation, which starts
after Track A (ADR-034/035 and the ADR-019 amendment) lands because they rewrite the same files.
1. Parameters in v1 (`filter_with`, `sort_by_key_with`): **yes** — the playground's `visible`
   needs them; closures that read other signals stay out of v1 and panic in debug.
2. Names: **`DerivedList<T>`** and **`derive()`**.
3. Caps: 4,096 pending ops; 256 ops for a parameter change, else a full value — **yes**.
4. `count()` in v1: **yes**.
5. Ties in source order; a sort-key change is Move + Update — **yes**.
The playground moves to recorded ops as part of the implementation so the benefit is visible on
the landing page's numbers; contract scenario S19 brings the grid to 19 × 3.
