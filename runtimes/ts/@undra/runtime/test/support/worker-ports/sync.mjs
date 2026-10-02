// A module for `LoadOptions.worker.ports` (ADR-049), for the tests of worker-ports.test.ts: implementations that run
// inside the worker. The stub core calls its port with method 0xdeadbeef (STUB.PORT_METHOD) on 0xc0dec0de
// (STUB.PORT_ID), or on a standard port's id when it is built with one.

/** What the ports here were asked, in order. */
export const asked = [];

const answer = (name, value) => ({
  sync: true,
  methods: {
    [0xdead_beef]: (args) => {
      asked.push(name);
      return Uint8Array.of(value, 0, 0, 0, ...args);
    },
  },
});

export default {
  // An app's synchronous port.
  [0xc0de_c0de]: answer("custom", 9),
  // An override of the built-in Clock (its id), answered in the worker.
  [0xcd99_c48e]: answer("clock", 4),
};

/** What backs the core's imports in the worker. */
export const adapters = {
  clock: { nowMs: () => 4321.5, monotonicNs: () => 1n },
  rng: { fill: (out) => out.fill(7) },
};
