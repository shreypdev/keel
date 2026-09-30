import { describe, expect, it } from "vitest";
import { STUB, compileStub, stubWat } from "./support/stub-core.js";

describe("the WAT stub", () => {
  it("assembles and validates", async () => {
    const bytes = await compileStub();
    expect(WebAssembly.validate(bytes as BufferSource)).toBe(true);
    expect(stubWat()).toContain("keel_call");
    expect(STUB.PORT_ID).toBeGreaterThan(2 ** 31);
  });
});
