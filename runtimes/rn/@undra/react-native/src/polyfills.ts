// Installs `TextDecoder` and `TextEncoder` where Hermes lacks them (./text-codec.ts). A module of
// its own, imported for its effect only: `@undra/runtime` creates its UTF-8 decoders when its modules
// load, so this must run first, and a bundler that makes imports lazy keeps a side-effect-only import
// eager (an import that also binds names may be deferred to its first use).
import { installPolyfills } from "./text-codec.js";

installPolyfills();
