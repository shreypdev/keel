# Adding Keel to @@NAME@@

`keel adopt` created this directory (`@@DIR@@`) and changed nothing else in the repository. It holds:

```
keel/keel.toml     the Keel project file
keel/core/         a Rust core crate with a first `#[keel::store]` to start from
keel/generated/    bindings for that core, in the languages of the platforms found
```

@@KEEL_SOURCE_NOTE@@

Detected: @@DETECTED@@.

## 0. Build the core and the bindings

```sh
cd @@DIR@@
keel doctor
keel bindgen                   # regenerate after changing the core's public surface
keel build --release           # libraries for the platforms below
```

@@STEPS@@
## Then

Replace the example store with the logic you want to share. The rule of thumb: start with one screen's state
(`#[keel::store]`), keep the existing app's own state management for everything else, and move logic over a
feature at a time. `keel dev` serves the core to a running app while you edit it.
