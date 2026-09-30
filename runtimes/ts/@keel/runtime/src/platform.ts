/** The platform name sent in `Hello` and `RuntimeConfig`: `"node"` under Node.js (and Bun and Deno's Node layer), else `"web"`. */
export function hostPlatform(): string {
  const process = (globalThis as { process?: { versions?: { node?: string } } }).process;
  return typeof process?.versions?.node === "string" ? "node" : "web";
}

/** Text of anything thrown. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
