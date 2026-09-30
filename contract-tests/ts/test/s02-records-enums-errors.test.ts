import { expect, test } from "vitest";
import {
  type Composite,
  type Figure,
  LabError,
  area,
  echoComposite,
  echoFigure,
  parseCount,
} from "@playground/core";
import { boot } from "../src/harness.js";
import { step } from "../src/wait.js";

// S02 records, enums and errors: variants keep their shape, nested records round-trip, and
// failures arrive as the typed `LabError` subclass, never as a generic failure.

/** Runs `run` and returns what it rejected with (failing the scenario if it resolved). */
async function failure(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error("expected the call to fail, but it succeeded");
}

test("S02 records, enums and errors", async () => {
  const { core } = await boot();

  await step("1. every variant of Figure comes back as itself", async () => {
    const figures: Figure[] = [
      { kind: "circle", radius: 2.5 },
      { kind: "rect", width: 2, height: 3.5 },
      { kind: "label", value: "" },
      { kind: "label", value: "héllo" },
      { kind: "empty" },
    ];
    for (const sent of figures) expect(await echoFigure(sent, core)).toEqual(sent);
  });

  await step("2. a record nesting lists, options, maps and enums", async () => {
    const full: Composite = {
      name: "everything",
      tags: ["a", "", "ü"],
      figure: { kind: "rect", width: 2, height: 3.5 },
      history: [{ kind: "circle", radius: 1 }, { kind: "label", value: "x" }, { kind: "empty" }],
      scores: new Map([
        ["high", 99],
        ["low", -3],
      ]),
      names: new Map([
        [7, "seven"],
        [1, "one"],
      ]),
      limit: 7,
    };
    const got = await echoComposite(full, core);
    expect(got).toEqual(full);
    // Maps compare by content, not by insertion order: check the entries survived whatever order the wire sorted them into.
    expect(Object.fromEntries(got.scores)).toEqual({ high: 99, low: -3 });
    expect(Object.fromEntries(got.names)).toEqual({ 1: "one", 7: "seven" });

    const empty: Composite = { name: "", tags: [], figure: null, history: [], scores: new Map(), names: new Map(), limit: null };
    expect(await echoComposite(empty, core)).toEqual(empty);
  });

  await step("3. area", async () => {
    expect(await area({ kind: "rect", width: 2, height: 4 }, core)).toBe(8);
    expect(Math.abs((await area({ kind: "circle", radius: 1 }, core)) - Math.PI)).toBeLessThan(1e-12);
  });

  await step("4. errors as values", async () => {
    const hat = await failure(() => area({ kind: "label", value: "hat" }, core));
    expect(hat).toBeInstanceOf(LabError.Rejected);
    expect(hat).toBeInstanceOf(LabError);
    expect(hat).toMatchObject({ kind: "rejected", code: 1, reason: "`hat` has no area" });

    expect(await failure(() => area({ kind: "empty" }, core))).toBeInstanceOf(LabError.Empty);

    expect(await failure(() => parseCount("", core))).toBeInstanceOf(LabError.Empty);
    const tooLong = await failure(() => parseCount("1234567890", core));
    expect(tooLong).toBeInstanceOf(LabError.TooLong);
    expect(tooLong).toMatchObject({ kind: "tooLong", max: 9 });
    const notANumber = await failure(() => parseCount("4x2", core));
    expect(notANumber).toBeInstanceOf(LabError.NotANumber);
    expect(notANumber).toMatchObject({ kind: "notANumber", value: "4x2" });

    expect(await parseCount(" 42 ", core)).toBe(42);
  });

  await step("5. the message is the core's Display", async () => {
    const tooLong = await failure(() => parseCount("1234567890", core));
    expect((tooLong as LabError).message).toBe("longer than 9 characters");
    const hat = await failure(() => area({ kind: "label", value: "hat" }, core));
    expect((hat as LabError).message).toBe("rejected with code 1: `hat` has no area");
  });
});
