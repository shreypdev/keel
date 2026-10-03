import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { SRC } from "./support/module-graph.js";

/*
 * The package's declarations are what generated code compiles against (R3: bindings an app generates must typecheck against the
 * `@undra/runtime` it installs). `tsconfig.build.json` sets `stripInternal`, so a declaration whose doc comment says `@internal`
 * is not in `dist/*.d.ts` (ADR-057): a name the TypeScript generator imports from the runtime must therefore be a public export
 * of the package's entry and must not be `@internal`, or every app whose schema uses it fails `tsc` while the repo's suites,
 * which alias the sources, stay green.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../../../../..");

/** Every `.ts` file under `dir`. */
function tsFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    return statSync(path).isDirectory() ? tsFiles(path) : path.endsWith(".ts") ? [path] : [];
  });
}

/** The value and type names generated code imports from `@undra/runtime` (the goldens and the examples' bindings, every TypeScript schema of the repo), plus what the generator names itself. */
function generatedImports(): Set<string> {
  const names = new Set<string>();
  const roots = [join(REPO, "crates/undra-bindgen/tests/golden"), join(REPO, "examples")];
  for (const root of roots) {
    for (const file of tsFiles(root).filter((f) => /[\\/](generated[\\/])?ts[\\/]src[\\/]/.test(f) && !f.includes("node_modules"))) {
      const source = ts.createSourceFile(file, readFileSync(file, "utf8"), ts.ScriptTarget.ES2022, false);
      for (const statement of source.statements) {
        if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier) || statement.moduleSpecifier.text !== "@undra/runtime") continue;
        const bindings = statement.importClause?.namedBindings;
        if (bindings !== undefined && ts.isNamedImports(bindings)) for (const element of bindings.elements) names.add((element.propertyName ?? element.name).text);
      }
    }
  }
  const generator = readFileSync(join(REPO, "crates/undra-bindgen/src/ts.rs"), "utf8");
  for (const m of generator.matchAll(/rt_(?:value|type)\("([A-Za-z_]\w*)"\)/g)) names.add(m[1] as string);
  return names;
}

describe("the declarations generated code compiles against", () => {
  it("export every name the TypeScript generator imports from @undra/runtime, none of them @internal (stripped from dist/*.d.ts)", () => {
    const names = generatedImports();
    expect(names.size, "the generator imports names from the runtime").toBeGreaterThan(20);
    const program = ts.createProgram([join(SRC, "index.ts")], { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler, noEmit: true, types: [] });
    const checker = program.getTypeChecker();
    const entry = program.getSourceFile(join(SRC, "index.ts")) as ts.SourceFile;
    const exported = new Map(checker.getExportsOfModule(checker.getSymbolAtLocation(entry) as ts.Symbol).map((symbol) => [symbol.getName(), symbol]));
    const problems: string[] = [];
    for (const name of [...names].sort()) {
      const symbol = exported.get(name);
      if (symbol === undefined) {
        problems.push(`${name}: not exported by src/index.ts`);
        continue;
      }
      const target = symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
      for (const declaration of target.declarations ?? []) {
        // The compiler's own test (any leading comment that says `@internal`, the tag or not), on the declaration and on the statement of a `const`.
        const statement = ts.isVariableDeclaration(declaration) ? declaration.parent.parent : declaration;
        if ([declaration, statement].some((node) => ts.isInternalDeclaration(node, node.getSourceFile()))) {
          problems.push(`${name}: @internal in ${declaration.getSourceFile().fileName.slice(SRC.length + 1)} (stripInternal drops it from the published declarations)`);
        }
      }
    }
    expect(problems).toEqual([]);
  });
});
