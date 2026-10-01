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
  memberExpression: (object: Node, property: Node): Node => ({ type: "MemberExpression", object, property }),
  callExpression: (callee: Node, args: Node[]): Node => ({ type: "CallExpression", callee, arguments: args }),
  newExpression: (callee: Node, args: Node[]): Node => ({ type: "NewExpression", callee, arguments: args }),
};

interface Visitor {
  MetaProperty(path: Path): void;
  CallExpression(path: Path): void;
  ImportExpression(path: Path): void;
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

const REJECTED = types.callExpression(types.memberExpression(types.identifier("Promise"), types.identifier("reject")), [
  types.newExpression(types.identifier("Error"), [types.stringLiteral("dynamic import is not available under React Native")]),
]);

describe("babel-plugin", () => {
  test("import.meta becomes { url: undefined }", () => {
    const path = pathOf({ type: "MetaProperty", meta: types.identifier("import"), property: types.identifier("meta") });
    visitor.MetaProperty(path);
    expect(path.replaced).toEqual(types.objectExpression([types.objectProperty(types.identifier("url"), types.identifier("undefined"))]));
    const other = pathOf({ type: "MetaProperty", meta: types.identifier("new"), property: types.identifier("target") });
    visitor.MetaProperty(other);
    expect(other.replaced).toBeNull();
  });

  test("an import() whose specifier is not a literal becomes a rejected promise (Metro cannot bundle it)", () => {
    const dynamic = pathOf({ type: "CallExpression", callee: { type: "Import" }, arguments: [types.identifier("name")] });
    visitor.CallExpression(dynamic);
    expect(dynamic.replaced).toEqual(REJECTED);
    const expression = pathOf({ type: "ImportExpression", source: types.identifier("name") });
    visitor.ImportExpression(expression);
    expect(expression.replaced).toEqual(REJECTED);
  });

  test("import('literal') and other calls are left alone", () => {
    const literal = pathOf({ type: "CallExpression", callee: { type: "Import" }, arguments: [types.stringLiteral("./x.js")] });
    visitor.CallExpression(literal);
    expect(literal.replaced).toBeNull();
    const template = pathOf({ type: "CallExpression", callee: { type: "Import" }, arguments: [{ type: "TemplateLiteral", expressions: [] }] });
    visitor.CallExpression(template);
    expect(template.replaced).toBeNull();
    const call = pathOf({ type: "CallExpression", callee: types.identifier("require"), arguments: [types.identifier("name")] });
    visitor.CallExpression(call);
    expect(call.replaced).toBeNull();
  });
});
