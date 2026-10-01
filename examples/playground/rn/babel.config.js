module.exports = {
  presets: ['module:@react-native/babel-preset'],
  // Hermes cannot compile `import.meta`, which @undra/runtime's wasm-worker mode (never used here)
  // contains: @undra/react-native's plugin replaces it (docs/REACT_NATIVE.md, "Install").
  plugins: ['@undra/react-native/babel-plugin'],
  overrides: [
    {
      // The Undra runtime and the generated bindings, used from their TypeScript sources, declare
      // fields (`declare readonly kind: ...`). The preset's TypeScript pass does not allow that, and
      // its Flow pass runs first, so strip TypeScript here first, with declare fields allowed.
      test: /\.ts$/,
      plugins: [['@babel/plugin-transform-typescript', { allowDeclareFields: true, allowNamespaces: true }]],
    },
  ],
};
