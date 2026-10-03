/*
 * The built-in codecs, one export per codec: `codecs` of `./codec.js` is this module's namespace (ADR-057). A pure barrel, so a
 * bundle that names `codecs.vec` keeps `vec` and what it uses, and no other module of this directory unless it names a codec
 * from it. `codecs-core.ts` holds the ones the runtime's own first chunk and a typical app use (the ten a hello page needs);
 * `codecs-more.ts` the other fourteen, which only a schema that has such a type pulls in.
 */
export * from "./codecs-core.js";
export * from "./codecs-more.js";
