# The Undra cookbook

One recipe per module of one Rust crate, each tested against the test runtime and the deterministic fakes. The docs
site's [cookbook pages](https://shreypdev.github.io/undra/docs/cookbook/) quote this code, so a recipe is checked by the
compiler and by `cargo test`, not only read.

| Module (`core/src/`) | Recipe |
|---|---|
| `net.rs` | where the server is, and what a failed request is (shared) |
| `auth.rs` | a session store, tokens in `SecureStore`, a 401 that re-authenticates, a logout that clears state |
| `paging.rs` | a keyed list fed page by page, a derived view, the cursor in a signal |
| `forms.rs` | a signal per field, errors derived from them, a command that refuses typed |
| `upload.rs` | a file from `Fs` in parts over `Http`, progress as a signal, retries through the offline queue |
| `offline.rs` | persisted queries, writes that queue, an outbox, an update (`#[undra(default)]`, `#[undra::migrate]`) |
| `realtime.rs` | a WebSocket that reconnects in the core, server-sent events as the fallback (feature `realtime`) |

```sh
cargo test -p cookbook                                   # every recipe, against the fakes
undra bindgen -C examples/cookbook --check --docs        # the bindings in generated/ are what the core generates
bash examples/cookbook/snippets/check.sh                 # the Swift, Kotlin and TypeScript lines the pages show compile
```

`snippets/` holds those lines (`docs:begin` / `docs:end` mark what a page quotes; `site/scripts/build-cookbook.mjs`
copies it into the pages). `cargo test -p cookbook --features realtime` also runs the real-time recipe; it needs an
`undra` that has the opt-in `WebSocket` and `Sse` ports (ADR-047), and is on in CI once they are on `main`. The recipes
come together in [Fieldbook](../fieldbook), the sample app.
