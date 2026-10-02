# Prototypes of ADR-057 (measurement only)

These files measured the levers of ADR-057 (`.10x/adrs/ADR-057-js-runtime-16kb.md`); the decision record
`.10x/decisions/architect/ts-runtime-16k.md` says which commit holds what. They are not part of the runtime and the commit
that follows these reverts them.

* `measure.mjs`: the gate's build (`scripts/web-size-runtime.mjs`) with switches: `--entry=dist/index.js`, `--map`,
  `--mangle[=under|auto|upper]`, `--messages`, `--split-helper`, `--conditions=`. Sizes are Python zlib level 9, as the gate's.
* `proto-plugins.mjs`: `mangle()` renames every `_name` property of the runtime's modules through one cache (what the
  publish-time pass does to `dist`); `messages()` turns every sentence of the runtime into `__m(code, ...values)` (what the
  production flavour ships; `src/msg.ts` is the helper).
* `mangle-dist.mjs`: the same rename applied to a `tsc` output directory, file by file, with one cache (`mangle-cache.json`).
* `split.mjs`: moves top-level declarations between modules (used to split `wire/payloads.ts` and `wire/codecs.ts`).
* `all-features-entry.ts`: the entry of the "all features" page (the playground's bindings, recovery, a panic handler, stats,
  snapshot and restore, a background run).
