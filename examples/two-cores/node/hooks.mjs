// Module resolution for the two-core Node app: the runtime and both generated packages are used from
// their TypeScript sources, as the contract tests and the playground's web app use them, so the app
// needs no build step and no installed packages. Node runs the TypeScript itself
// (--experimental-transform-types); this hook maps the package names to the sources and a `.js`
// import of a TypeScript file to the `.ts` file (the sources import each other as `./x.js`).
import { existsSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";

const at = (path) => new URL(path, import.meta.url).href;
const packages = {
  "@undra/runtime": at("../../../runtimes/ts/@undra/runtime/src/index.ts"),
  "@two-cores/a": at("../a/generated/ts/src/index.ts"),
  "@two-cores/b": at("../b/generated/ts/src/index.ts"),
};

export async function resolve(specifier, context, next) {
  if (specifier in packages) return { url: packages[specifier], shortCircuit: true };
  if (specifier.endsWith(".js") && (specifier.startsWith("./") || specifier.startsWith("../")) && context.parentURL) {
    const ts = new URL(specifier.replace(/\.js$/, ".ts"), context.parentURL);
    if (ts.protocol === "file:" && existsSync(fileURLToPath(ts))) return { url: pathToFileURL(fileURLToPath(ts)).href, shortCircuit: true };
  }
  return next(specifier, context);
}
