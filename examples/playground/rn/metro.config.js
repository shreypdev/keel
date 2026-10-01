// Metro for the React Native playground.
//
// Like the web app (../web/vite.config.ts), the app uses the Undra runtime, @undra/react-native and
// the generated bindings from their TypeScript sources in this checkout, so a change to any of them
// needs no build step. Those sources are ESM TypeScript that import their siblings as "./x.js";
// Metro is told to serve "./x.ts" for them.
const fs = require('fs');
const path = require('path');
const { getDefaultConfig, mergeConfig } = require('@react-native/metro-config');

const repo = path.resolve(__dirname, '../../..');
const runtime = path.join(repo, 'runtimes/ts/@undra/runtime/src');
const reactNative = path.join(repo, 'runtimes/rn/@undra/react-native/src');
const generated = path.resolve(__dirname, '../generated/ts/src');

const aliases = {
  '@undra/runtime/react': path.join(runtime, 'react.ts'),
  '@undra/runtime': path.join(runtime, 'index.ts'),
  '@undra/react-native': path.join(reactNative, 'index.ts'),
  '@playground/core': path.join(generated, 'index.ts'),
};

const config = {
  watchFolders: [runtime, reactNative, generated],
  resolver: {
    // Files outside this directory (the runtime's sources) import `react` too: one copy, this app's.
    nodeModulesPaths: [path.join(__dirname, 'node_modules')],
    resolveRequest(context, moduleName, platform) {
      const alias = aliases[moduleName];
      if (alias !== undefined) {
        return { type: 'sourceFile', filePath: alias };
      }
      if (moduleName.startsWith('.') && moduleName.endsWith('.js')) {
        const base = path.resolve(path.dirname(context.originModulePath), moduleName.slice(0, -3));
        for (const extension of ['.ts', '.tsx']) {
          if (fs.existsSync(base + extension)) {
            return { type: 'sourceFile', filePath: base + extension };
          }
        }
      }
      return context.resolveRequest(context, moduleName, platform);
    },
  },
};

module.exports = mergeConfig(getDefaultConfig(__dirname), config);
