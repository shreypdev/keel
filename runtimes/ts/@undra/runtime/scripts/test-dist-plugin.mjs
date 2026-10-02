// A Vite plugin for suites that run against the production build of @undra/runtime (`UNDRA_TEST_DIST=1` for the runtime's own,
// `UNDRA_TS_DIST=<runtime>/dist/index.js` for the packages beside it; ADR-057): the code is the build's (private properties renamed, modules as
// `tsc` wrote them), except that `messages.js` is the development table. The tests word their expectations as the sentences, so they read
// those; what the production table says is held by `flavours.test.ts` and `dist-flavour.test.ts`.
//
// With `sources`, an import of a module of that directory (the runtime's own `src/`) resolves to the same module of `dist`, so the runtime's tests
// (`../src/core.js`) run the build.
import { existsSync } from "node:fs";

/** @param {string} dist the runtime's built production flavour (an absolute path, no trailing slash) @param {{ sources?: string }} [options] */
export function againstDist(dist, { sources } = {}) {
  const src = sources === undefined ? undefined : `${sources}/`;
  return {
    name: "undra-test-dist",
    enforce: "pre",
    async resolveId(source, importer, options) {
      const resolved = await this.resolve(source, importer, { ...options, skipSelf: true });
      if (resolved === null || resolved.external) return null;
      const id = resolved.id;
      if (id === `${dist}/messages.js` || (src !== undefined && id === `${src}messages.ts`)) return `${dist}/dev/messages.js`;
      if (src === undefined || !id.startsWith(src)) return null;
      const built = `${dist}/${id.slice(src.length).replace(/\.ts$/, ".js")}`;
      return existsSync(built) ? built : null;
    },
  };
}
