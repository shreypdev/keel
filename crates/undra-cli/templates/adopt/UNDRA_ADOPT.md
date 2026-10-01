# Adding Undra to @@NAME@@

`undra adopt` created this directory (`@@DIR@@`) and changed nothing else in the repository. It holds:

```
undra/undra.toml     the Undra project file
undra/core/         a Rust core crate with a first `#[undra::store]` to start from
undra/generated/    bindings for that core, in the languages of the platforms found
```

@@UNDRA_SOURCE_NOTE@@

Detected: @@DETECTED@@.

## 0. Build the core and the bindings

```sh
cd @@DIR@@
undra doctor
undra bindgen                   # regenerate after changing the core's public surface
undra build --release           # libraries for the platforms below
```

@@STEPS@@
## Then

Replace the example store with the logic you want to share. The rule of thumb: start with one screen's state
(`#[undra::store]`), keep the existing app's own state management for everything else, and move logic over a
feature at a time. `undra dev` serves the core to a running app while you edit it.
