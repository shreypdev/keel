// A module for `LoadOptions.worker.ports` whose port answers later (a promise), inside the worker; see sync.mjs.
export default {
  [0xc0de_c0de]: {
    sync: false,
    methods: {
      [0xdead_beef]: async (args) => {
        await new Promise((resolve) => setTimeout(resolve, 1));
        return Uint8Array.of(3, 0, 0, 0, ...args);
      },
    },
  },
};
