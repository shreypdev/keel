// A module for `LoadOptions.worker.ports` whose port holds something for the core inside the worker (as `dbPort` over
// wa-sqlite would, in the core's own worker): it records when the worker lets it release that (`PortImpl.dispose`).
export const disposed = [];

export default {
  [0xc0de_c0de]: {
    name: "Holder",
    sync: false,
    methods: {
      [0xdead_beef]: async (args) => Uint8Array.of(3, 0, 0, 0, ...args),
    },
    dispose() {
      disposed.push("dispose");
    },
  },
};
