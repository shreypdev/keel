import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

/*
 * The module graph of `src/`, at the granularity a bundler places modules in chunks (ADR-057, "the module rule"):
 * a module is in the first chunk when a module of the first chunk that has code of its own imports it **or re-exports
 * it**, even if only an on-demand chunk would use the name; a *pure barrel* (a file made only of `export .. from`
 * lines) has no code of its own, so a name reached through one lands in the module that defines it and nowhere else.
 * `reachable` models exactly that, so a test can say "this module must stay out of the first chunk" without a build.
 */

export const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "../../src");

interface Edge {
  /** The module imported or re-exported from, as a path relative to `src/` (`wire/codec.ts`). */
  readonly target: string;
  /** The names used, or `"all"` for `import * as`, a bare import and `export *`. */
  readonly names: readonly string[] | "all";
}

export interface ModuleInfo {
  readonly file: string;
  /** Every edge a bundler follows: run-time imports and re-exports (type-only ones are erased). */
  readonly edges: readonly Edge[];
  /** Made only of re-exports (and types): contributes no code of its own. */
  readonly pureBarrel: boolean;
  /** Names this module declares as values. */
  readonly own: ReadonlySet<string>;
  /** `export { a as b } from "m"` and `import { a } from "m"; export { a }`: exported name to where it comes from. */
  readonly named: ReadonlyMap<string, { readonly target: string; readonly name: string | "all" }>;
  /** `export * from "m"` targets. */
  readonly stars: readonly string[];
}

const infos = new Map<string, ModuleInfo>();

/** All `.ts` files under `src/` (relative paths), declarations excluded. */
export function sourceFiles(dir: string = SRC): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) files.push(...sourceFiles(path));
    else if (entry.endsWith(".ts") && !entry.endsWith(".d.ts")) files.push(relative(SRC, path));
  }
  return files.sort();
}

function specifierTarget(from: string, specifier: string): string | undefined {
  if (!specifier.startsWith(".")) return undefined;
  return relative(SRC, resolve(SRC, dirname(from), specifier.replace(/\.js$/, ".ts")));
}

const has = (node: ts.Node, kind: ts.SyntaxKind): boolean => ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((m) => m.kind === kind);

/** What `file` (a path relative to `src/`) imports, exports and declares. */
export function info(file: string): ModuleInfo {
  const known = infos.get(file);
  if (known !== undefined) return known;
  const source = ts.createSourceFile(file, readFileSync(join(SRC, file), "utf8"), ts.ScriptTarget.ES2022, true);
  const edges: Edge[] = [];
  const own = new Set<string>();
  const named = new Map<string, { target: string; name: string | "all" }>();
  const stars: string[] = [];
  const imported = new Map<string, { target: string; name: string | "all" }>();
  let code = false;
  for (const node of source.statements) {
    if (ts.isImportDeclaration(node)) {
      const target = specifierTarget(file, (node.moduleSpecifier as ts.StringLiteral).text);
      const clause = node.importClause;
      if (target === undefined) {
        code = true;
        continue;
      }
      if (clause === undefined) {
        edges.push({ target, names: "all" });
        code = true;
        continue;
      }
      if (clause.isTypeOnly) continue;
      const bindings = clause.namedBindings;
      if (bindings !== undefined && ts.isNamedImports(bindings)) {
        const values = bindings.elements.filter((e) => !e.isTypeOnly);
        if (values.length === 0 && clause.name === undefined) continue; // `import { type A }` is erased as a whole
        edges.push({ target, names: values.map((e) => (e.propertyName ?? e.name).text) });
        for (const e of values) imported.set(e.name.text, { target, name: (e.propertyName ?? e.name).text });
      } else {
        edges.push({ target, names: "all" });
        if (bindings !== undefined && ts.isNamespaceImport(bindings)) imported.set(bindings.name.text, { target, name: "all" });
      }
    } else if (ts.isExportDeclaration(node)) {
      if (node.isTypeOnly) continue;
      const clause = node.exportClause;
      if (node.moduleSpecifier === undefined) {
        // `export { a, b as c }` of local or imported bindings.
        if (clause !== undefined && ts.isNamedExports(clause)) {
          for (const e of clause.elements) {
            if (e.isTypeOnly) continue;
            const local = (e.propertyName ?? e.name).text;
            const from = imported.get(local);
            if (from !== undefined) named.set(e.name.text, from);
            else {
              own.add(e.name.text);
              code = true;
            }
          }
        }
        continue;
      }
      const target = specifierTarget(file, (node.moduleSpecifier as ts.StringLiteral).text);
      if (target === undefined) continue;
      if (clause === undefined) {
        edges.push({ target, names: "all" });
        stars.push(target);
      } else if (ts.isNamespaceExport(clause)) {
        // `export * as ns from "m"`: a bundler resolves a member access (`ns.vec`) to the module that defines it and keeps no
        // other member of `m`, so no member is followed here (`namespaceMembersUsed` finds the ones the first chunk names).
        edges.push({ target, names: [] });
        named.set(clause.name.text, { target, name: "all" });
      } else {
        const values = clause.elements.filter((e) => !e.isTypeOnly);
        if (values.length === 0) continue;
        edges.push({ target, names: values.map((e) => (e.propertyName ?? e.name).text) });
        for (const e of values) named.set(e.name.text, { target, name: (e.propertyName ?? e.name).text });
      }
    } else if (ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node)) {
      continue;
    } else {
      code = true;
      if (!has(node, ts.SyntaxKind.ExportKeyword)) continue;
      if (ts.isVariableStatement(node)) {
        for (const d of node.declarationList.declarations) if (ts.isIdentifier(d.name)) own.add(d.name.text);
      } else if ((ts.isFunctionDeclaration(node) || ts.isClassDeclaration(node) || ts.isEnumDeclaration(node)) && node.name !== undefined) {
        own.add(node.name.text);
      }
    }
  }
  const result: ModuleInfo = { file, edges, pureBarrel: !code, own, named, stars };
  infos.set(file, result);
  return result;
}

/** The module that declares `name` as exported from `file`, following re-exports; `undefined` when it is a type or unknown. */
export function definedIn(file: string, name: string, seen = new Set<string>()): string | undefined {
  if (seen.has(file)) return undefined;
  seen.add(file);
  const module = info(file);
  if (module.own.has(name)) return file;
  const via = module.named.get(name);
  if (via !== undefined) return via.name === "all" ? via.target : definedIn(via.target, via.name, seen);
  for (const star of module.stars) {
    const found = definedIn(star, name, seen);
    if (found !== undefined) return found;
  }
  return undefined;
}

/**
 * The modules a bundler puts in the first chunk of an app whose entry uses what `entries` define (here: the modules themselves,
 * all of whose code counts): the entries, and every module reached from a module that has code of its own by an import or a
 * re-export, a pure barrel resolved to the modules that define the imported names.
 */
export function reachable(entries: readonly string[]): Set<string> {
  const seen = new Set<string>();
  const follow = (edge: Edge): void => {
    const target = info(edge.target);
    if (!target.pureBarrel) {
      include(edge.target);
    } else if (edge.names === "all") {
      for (const inner of target.edges) follow(inner);
    } else {
      for (const name of edge.names) {
        const home = definedIn(edge.target, name);
        if (home === undefined) continue;
        // A name that is a namespace of a barrel (`codecs`): the barrel is reached, none of what it re-exports.
        if (info(home).pureBarrel) seen.add(home);
        else include(home);
      }
    }
  };
  const include = (file: string): void => {
    if (seen.has(file)) return;
    seen.add(file);
    for (const edge of info(file).edges) follow(edge);
  };
  for (const entry of entries) include(entry);
  return seen;
}

/** The files with code of their own that import or re-export `module` at run time (not pure barrels). */
export function codeImporters(module: string): string[] {
  return sourceFiles().filter((file) => {
    const i = info(file);
    return !i.pureBarrel && i.edges.some((edge) => edge.target === module);
  });
}

/** The members of the namespace `ns` that `files` name by `ns.member` (`codecs.vec` gives `vec`). */
export function namespaceMembersUsed(files: Iterable<string>, ns: string): Set<string> {
  const used = new Set<string>();
  for (const file of files) {
    for (const match of readFileSync(join(SRC, file), "utf8").matchAll(new RegExp(`\\b${ns}\\.(?!(?:js|ts)\\b)([A-Za-z0-9_]+)`, "g"))) used.add(match[1] as string);
  }
  return used;
}

/** The names `file` exports as values. */
export function exportedValues(file: string): Set<string> {
  const module = info(file);
  const names = new Set([...module.own, ...module.named.keys()]);
  for (const star of module.stars) for (const name of exportedValues(star)) names.add(name);
  return names;
}
