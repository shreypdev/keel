import { test } from "vitest";

// S29 panic report, React Native column. contract-tests/ts's S29 is the wasm variant (a panic traps the module and the host
// builds the report from the core's FATAL record and the trap's stack); a native core reports each panic it contained through its
// `Diagnostics` port, which the C++ host queues for the JavaScript thread. The stand-in of this column is a wasm core, so it cannot
// show a native report with its frames: test/diagnostics.test.ts drives the JavaScript half (the record, `onPanic` once per report
// and in order, a throwing handler), cpp/test/host_test.cpp the C++ half (the queue, from any thread), and the app checks the whole
// path on the device (RN04, examples/playground/rn/src/checks.ts) against the real native core.
test("S29 panic report", (context) => {
  context.skip(
    "app-tested: native cores report through the Diagnostics port and the wasm stand-in traps instead; diagnostics.test.ts and the C++ host test cover the halves, RN04 the whole path on the iOS simulator and the Android emulator",
  );
});
