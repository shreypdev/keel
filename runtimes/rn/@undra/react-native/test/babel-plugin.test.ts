import { createRequire } from "node:module";
import { describe, expect, test } from "vitest";

/*
 * The package's Babel plugin (babel-plugin.cjs) on hand-made AST nodes, with the node builders of `@babel/types` as
 * plain objects: the package does not depend on Babel (the app's Babel runs the plugin; the playground's Metro build
 * proves it there).
 */

type Node = { readonly type: string } & Record<string, unknown>;

const types = {
  identifier: (name: string): Node => ({ type: "Identifier", name }),
  stringLiteral: (value: string): Node => ({ type: "StringLiteral", value }),
  objectProperty: (key: Node, value: Node): Node => ({ type: "ObjectProperty", key, value }),
  objectExpression: (properties: Node[]): Node => ({ type: "ObjectExpression", properties }),
};

interface Visitor {
  MetaProperty(path: Path): void;
}

interface Path {
  node: Node;
  replaced: Node | null;
  replaceWith(node: Node): void;
}

const plugin = createRequire(import.meta.url)("../babel-plugin.cjs") as (babel: { types: typeof types }) => { visitor: Visitor };
const { visitor } = plugin({ types });

function pathOf(node: Node): Path {
  const path: Path = {
    node,
    replaced: null,
    replaceWith(next) {
      path.replaced = next;
    },
  };
  return path;
}

describe("babel-plugin", () => {
  test("import.meta becomes { url: undefined }", () => {
    const path = pathOf({ type: "MetaProperty", meta: types.identifier("import"), property: types.identifier("meta") });
    visitor.MetaProperty(path);
    expect(path.replaced).toEqual(types.objectExpression([types.objectProperty(types.identifier("url"), types.identifier("undefined"))]));
    const other = pathOf({ type: "MetaProperty", meta: types.identifier("new"), property: types.identifier("target") });
    visitor.MetaProperty(other);
    expect(other.replaced).toBeNull();
  });
});
