// A Babel plugin for React Native apps that use @undra/runtime (docs/REACT_NATIVE.md, "Install").
//
// Metro bundles every module the runtime's entry points reach, including code for platforms React
// Native is not, which must still compile under Hermes and pass Metro's dependency collection:
//
//  * `import.meta`: Hermes cannot compile it, and @undra/runtime's `wasm-worker` mode reaches for
//    `import.meta.url` to find its worker script (that mode never runs under React Native: there is
//    no WebAssembly and no Worker). It becomes `{ url: undefined }`.
//  * `import(specifier)` with a specifier that is not a string literal: Metro refuses to bundle it
//    outside `node_modules` (an app that serves @undra/runtime from its sources, as the playground
//    does), and `@undra/runtime/realtime`'s Node WebSocket adapter loads `node:http` that way (it
//    never runs under React Native, which has a `WebSocket`). It becomes a promise that rejects with
//    "dynamic import is not available under React Native"; an `import("literal")` is left alone.
//
// Nothing else changes.
//
//   // babel.config.js
//   module.exports = {
//     presets: ['module:@react-native/babel-preset'],
//     plugins: ['@undra/react-native/babel-plugin'],
//   };
"use strict";

module.exports = function undraReactNative({ types: t }) {
  /** Replaces a dynamic `import` whose specifier is not a literal with a rejected promise. */
  function rejectDynamic(path, specifier) {
    if (specifier !== undefined && specifier.type === "StringLiteral") return;
    if (specifier !== undefined && specifier.type === "TemplateLiteral" && specifier.expressions.length === 0) return;
    path.replaceWith(
      t.callExpression(t.memberExpression(t.identifier("Promise"), t.identifier("reject")), [
        t.newExpression(t.identifier("Error"), [t.stringLiteral("dynamic import is not available under React Native")]),
      ]),
    );
  }

  return {
    name: "@undra/react-native",
    visitor: {
      MetaProperty(path) {
        const { meta, property } = path.node;
        if (meta.name === "import" && property.name === "meta") {
          path.replaceWith(t.objectExpression([t.objectProperty(t.identifier("url"), t.identifier("undefined"))]));
        }
      },
      // Babel 7 parses `import(x)` as a call of `Import`; with `createImportExpressions` it is an `ImportExpression`.
      CallExpression(path) {
        if (path.node.callee.type === "Import") rejectDynamic(path, path.node.arguments[0]);
      },
      ImportExpression(path) {
        rejectDynamic(path, path.node.source);
      },
    },
  };
};
