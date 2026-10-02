// A module for `LoadOptions.worker.ports` whose port fails: with a typed error (an UndraPortError, status 1) on its
// first call and with a bug (a plain Error, status 2 and an error-level log) after that; see sync.mjs.
import { UndraPortError } from "../../../src/errors.ts";

let calls = 0;

export default {
  [0xc0de_c0de]: {
    sync: true,
    methods: {
      [0xdead_beef]: () => {
        calls += 1;
        if (calls === 1) throw new UndraPortError(Uint8Array.of(1, 0));
        throw new Error("a bug in the worker's port");
      },
    },
  },
};
