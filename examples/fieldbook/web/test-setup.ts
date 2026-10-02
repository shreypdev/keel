import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

// Builds the core the integration test loads (`undra build --platform web`), unless it is already built. `undra`
// comes from PATH, or from UNDRA_BIN.
export default function setup(): void {
  const project = fileURLToPath(new URL("..", import.meta.url));
  if (existsSync(`${project}build/web/fieldbook_core.wasm`)) return;
  const bin = process.env["UNDRA_BIN"] ?? "undra";
  const built = spawnSync(bin, ["build", "--platform", "web", "-C", project], { stdio: "inherit" });
  if (built.status !== 0) throw new Error(`${bin} build --platform web failed: is \`undra\` on PATH (or UNDRA_BIN set)?`);
}
