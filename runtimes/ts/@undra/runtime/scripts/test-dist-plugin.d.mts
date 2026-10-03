import type { Plugin } from "vite";

/** Runs a suite against the production build of `@undra/runtime` with the development messages (ADR-057); `sources` maps that directory's modules to the build's. */
export function againstDist(dist: string, options?: { sources?: string }): Plugin;
