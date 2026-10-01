// A Babel plugin for React Native apps that use @undra/runtime (docs/REACT_NATIVE.md, "Install").
//
// Hermes cannot compile `import.meta`, and @undra/runtime's `wasm-worker` mode reaches for
// `import.meta.url` to find its worker script. That mode never runs under React Native (there is no
// WebAssembly and no Worker), but Metro bundles every module the runtime's entry point reaches, so
// the expression must still compile. This plugin replaces `import.meta` with `{ url: undefined }`;
// nothing else changes.
//
//   // babel.config.js
//   module.exports = {
//     presets: ['module:@react-native/babel-preset'],
//     plugins: ['@undra/react-native/babel-plugin'],
//   };
"use strict";

module.exports = function undraReactNative({ types: t }) {
  return {
    name: "@undra/react-native",
    visitor: {
      MetaProperty(path) {
        const { meta, property } = path.node;
        if (meta.name === "import" && property.name === "meta") {
          path.replaceWith(t.objectExpression([t.objectProperty(t.identifier("url"), t.identifier("undefined"))]));
        }
      },
    },
  };
};
