/**
 * What a core that was not given the stream feature loads at its first stream (`UndraCore.stream`, ADR-057): `stream-support.ts` through a
 * module that nothing else imports. An app whose generated entry passes `features: [streams]` has `stream-support.ts` in its first chunk
 * (through `index.ts`), and a module that is both an `import()` target and a static import makes a bundler say so (Rolldown's
 * INEFFECTIVE_DYNAMIC_IMPORT) at every build of that app; this module is the target, so it never does.
 */
export { streams } from "./stream-support.js";
