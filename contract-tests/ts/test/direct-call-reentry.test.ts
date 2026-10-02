import { expect, test } from "vitest";
import { CallTarget, UndraCallError, UndraReplyError, UndraWriter } from "@undra/runtime";
import { UndraIds, add, explode } from "@playground/core";
import { CapturingLog } from "../src/capturing-log.js";
import { boot } from "../src/harness.js";

// Not a scenario (no SCENARIO line): ADR-056's direct call (`UndraCore.call` on a core that answers inside `send`) and
// `callSync`, made from inside a host callback of the running core. The Log adapter is called on the thread that holds
// the core lock (the panic hook logs before it traps), so a call from it re-enters: the core refuses it (E_REENTRANT, status 5), and
// the runtime types the refusal. Identical on `main` before the call-path levers (the review ran both).
test("a call made from inside a host callback is refused, typed, on every entry", async () => {
  const log = new CapturingLog();
  let hook: (() => void) | null = null;
  const record = log.log.bind(log);
  log.log = (level: number, target: string, message: string) => {
    record(level, target, message);
    if (hook !== null && target === "undra::panic") {
      const run = hook;
      hook = null;
      run();
    }
  };
  const { core } = await boot({ log });
  const w = new UndraWriter();
  w.writeI32(1);
  w.writeI32(1);
  const args = w.finish();
  const outcome: { generated?: unknown; raw?: unknown; sync?: unknown } = {};
  hook = () => {
    outcome.generated = add(1, 1, core).then(() => "resolved", (e: unknown) => e);
    outcome.raw = core.call({ target: CallTarget.FreeFunction }, UndraIds.Functions.add, args).then(() => "resolved", (e: unknown) => e);
    try {
      core.callSync({ target: CallTarget.FreeFunction }, UndraIds.Functions.add, args);
      outcome.sync = "returned";
    } catch (e) {
      outcome.sync = e;
    }
  };
  await explode("reenter", core).catch(() => undefined); // the wasm core aborts on a panic: the call fails, the core closes
  const generated = await outcome.generated;
  expect(generated, "the generated call").toBeInstanceOf(UndraCallError.Refused);
  const raw = await outcome.raw;
  expect(raw, "core.call").toBeInstanceOf(UndraReplyError);
  expect((raw as UndraReplyError).status).toBe(5);
  expect(outcome.sync, "core.callSync").toBeInstanceOf(UndraReplyError);
  expect((outcome.sync as UndraReplyError).status).toBe(5);
  expect(String((outcome.sync as UndraReplyError).message)).toContain("E_REENTRANT");
});
