import { test } from "vitest";

// S17 panic containment, React Native column. contract-tests/ts's S17 is the wasm variant (a panic
// traps the module and the host restarts it); a native core contains the panic instead (status 2,
// the core keeps working), which is what the Swift and Kotlin columns check. The stand-in of this
// column is a wasm core, so it cannot show native containment: the app checks it on the device
// (RN04, examples/playground/rn/src/checks.ts) against the real native core.
test("S17 panic containment", (context) => {
  context.skip(
    "app-tested: native cores contain panics (status 2) and the wasm stand-in traps instead; RN04 checks containment on the iOS simulator and the Android emulator",
  );
});
