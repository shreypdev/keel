/** The platform name sent in `Hello` and `RuntimeConfig`: `"node"` under Node.js (and Bun and Deno's Node layer), else `"web"`. */
export function hostPlatform(): string {
  const process = (globalThis as { process?: { versions?: { node?: string } } }).process;
  return typeof process?.versions?.node === "string" ? "node" : "web";
}

/** Text of anything thrown. Never throws itself (`report` relies on that): a value without a string form reads as its tag. */
export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  try {
    return String(error);
  } catch {
    // `Object.create(null)`, a `toString` that throws: `String()` cannot convert it.
    return Object.prototype.toString.call(error);
  }
}
