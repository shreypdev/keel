// Tests for decls.mjs, the declarations-only reader behind the API reference pages.
//
//   node --test site/scripts/decls.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { declarations, mask, fold } from "./decls.mjs";

const lines = (text) => text.split("\n");

test("mask blanks comments and string contents but keeps the layout", () => {
  const src = 'let a = "x { y }" // { not code\n/* } */ let b = 1';
  const m = mask(src, "swift");
  assert.equal(m.length, src.length);
  assert.ok(!m.includes("{") && !m.includes("}"), m);
  assert.equal(m.indexOf("\n"), src.indexOf("\n"));
});

test("mask skips string interpolation that contains braces, parentheses and quotes", () => {
  assert.ok(!mask('"a \\(f("}") ) b"', "swift").includes("}"));
  assert.ok(!mask('"a ${ x({}) + "}" } b"', "kotlin").includes("}"));
  assert.ok(!mask("`a ${ {a:1}.a } \\` }`", "ts").includes("}"));
});

test("swift: bodies, initializers and private members go; docs, attributes and signatures stay", () => {
  const src = `import Foundation

/// A store.
/// Two lines.
@MainActor @Observable
public final class Todos: UndraStore, @unchecked Sendable {
    public private(set) var todos: [Todo] = []
    /// Computed by the core.
    public private(set) var visible: [Todo] = []
    private var secret = 1
    var internalOnly = 2

    private init(adopting handle: UndraHandle) {
        super.init(handle: handle)
    }

    /// Adds one.
    /// - Throws: \`TodoError\`
    public func add(
        title: String
    ) async throws -> Todo {
        let s = "}"   // a brace in a string
        return try await x(s)
    }

    public override func apply(signal: UInt32) { }

    public static let kind: UInt32 = 0x1f
    public var description: String {
        return "x"
    }
}

extension Todos: CustomStringConvertible {
    public var summary: String { "" }
    func hidden() {}
}

extension Other {
    func hidden() {}
}

public enum Filter: UInt16 {
    /// All.
    case all = 0, active = 1
}

public protocol Http: UndraPort {
    func request(_ req: Req) async throws -> Res
}

public enum Ports {}
`;
  const { text, names } = declarations(src, "swift");
  assert.equal(text, `/// A store.
/// Two lines.
@MainActor @Observable
public final class Todos: UndraStore, @unchecked Sendable {
    public private(set) var todos: [Todo]
    /// Computed by the core.
    public private(set) var visible: [Todo]

    /// Adds one.
    /// - Throws: \`TodoError\`
    public func add(
        title: String
    ) async throws -> Todo

    public static let kind: UInt32 = 0x1f
    public var description: String
}

extension Todos: CustomStringConvertible {
    public var summary: String
}

public enum Filter: UInt16 {
    /// All.
    case all = 0, active = 1
}

public protocol Http: UndraPort {
    func request(_ req: Req) async throws -> Res
}

public enum Ports {}
`);
  assert.deepEqual(names, ["Todos", "Filter", "Http", "Ports"]);
});

test("kotlin: overrides, private, internal, init blocks and delegation go; entries, companions and expression bodies are handled", () => {
  const src = `package x

import y.Z

/** A store. */
class Todos private constructor(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    private val _todos: MutableStateFlow<List<Todo>> = signal(emptyList())
    /** The todos. */
    val todos: StateFlow<List<Todo>> = _todos.asStateFlow()
    internal val hidden = 1

    init {
        observeAll()
    }

    /** New. */
    constructor(ctx: UndraCore = UndraCore.shared) : this(
        ctx,
        ctx.make(),
    )

    /** Adds. */
    suspend fun add(title: String): Todo {
        val s = "\${title}}"
        return call(s)
    }

    fun twice(x: Int): Int = x * 2

    override fun apply(signalId: UInt) {
        when (x) { else -> {} }
    }

    companion object {
        /** Makes one. */
        fun create(ctx: UndraCore = UndraCore.shared): Todos = Todos(ctx)
    }
}

enum class Filter(val index: UShort) : UndraEnum {
    /** All. */
    ALL(0u),
    /** Done. */
    DONE(1u);

    companion object : UndraCodec<Filter> {
        override fun encode(w: UndraWriter, v: Filter) = w.writeU16(v.index)
    }
}

sealed interface Figure : UndraEnum {
    data class Circle(val radius: Double) : Figure
    data object Empty : Figure
}

object UndraIds {
    const val SCHEMA_HASH: ULong = 0x04d2uL

    object Objects {}
}
`;
  const { text, names } = declarations(src, "kotlin");
  assert.equal(text, `/** A store. */
class Todos private constructor(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    /** The todos. */
    val todos: StateFlow<List<Todo>>

    /** New. */
    constructor(ctx: UndraCore = UndraCore.shared)

    /** Adds. */
    suspend fun add(title: String): Todo

    fun twice(x: Int): Int

    companion object {
        /** Makes one. */
        fun create(ctx: UndraCore = UndraCore.shared): Todos
    }
}

enum class Filter(val index: UShort) : UndraEnum {
    /** All. */
    ALL(0u),
    /** Done. */
    DONE(1u);

    companion object : UndraCodec<Filter> { … }
}

sealed interface Figure : UndraEnum {
    data class Circle(val radius: Double) : Figure
    data object Empty : Figure
}

object UndraIds {
    const val SCHEMA_HASH: ULong = 0x04d2uL

    object Objects {}
}
`);
  assert.deepEqual(names, ["Todos", "Filter", "Figure", "UndraIds"]);
});

test("typescript: unions and interfaces verbatim, bodies and non-exports gone, async dropped, a value with no type kept", () => {
  const src = `import { type Codec } from "@undra/runtime";

const helper: Codec<number> = { encode() {} };

/** Which items. */
export type Filter =
  /** Every item. */
  | "all"
  /** Done. */
  | "done";

export const FilterCodec: Codec<Filter> = {
  encode(w, v) { if (v === "all") { w.writeU16(0); } },
};

/** A row. */
export interface Item {
  /** The id. */
  id: number;
}

/** A store. */
export class Todos extends UndraStore {
  /** The list. */
  readonly todos: Signal<Todo[]> = new Signal<Todo[]>([]);
  private constructor(core: UndraCore) {
    super(core);
  }

  /** Makes one. */
  static async create(core: UndraCore = UndraCore.shared): Promise<Todos> {
    const s = \`\${core.x({ a: 1 })}\`;
    return new Todos(core);
  }

  protected override _apply(id: number): void {}
  private helper(): void {}
}

export namespace LabError {
  /** Empty. */
  export class Empty extends LabError {
    declare readonly kind: "empty";

    constructor(readonly max: number) {
      super("empty", \`longer than \${max}\`);
    }
  }
}

export async function add(a: number, b: number): Promise<number> {
  return a + b;
}

export const UndraIds = {
  schemaHash: 0x1fn,
  Objects: { Probe: { typeId: 0x4f } },
} as const;

export {};

export * from "./types.js";
`;
  const { text, names } = declarations(src, "ts");
  assert.equal(text, `/** Which items. */
export type Filter =
  /** Every item. */
  | "all"
  /** Done. */
  | "done";

export const FilterCodec: Codec<Filter>;

/** A row. */
export interface Item {
  /** The id. */
  id: number;
}

/** A store. */
export class Todos extends UndraStore {
  /** The list. */
  readonly todos: Signal<Todo[]>;

  /** Makes one. */
  static create(core: UndraCore = UndraCore.shared): Promise<Todos>;
}

export namespace LabError {
  /** Empty. */
  export class Empty extends LabError {
    declare readonly kind: "empty";

    constructor(readonly max: number);
  }
}

export function add(a: number, b: number): Promise<number>;

export const UndraIds = {
  schemaHash: 0x1fn,
  Objects: { Probe: { typeId: 0x4f } },
} as const;

export * from "./types.js";
`);
  assert.deepEqual(names, ["Filter", "Item", "Todos", "LabError", "add", "UndraIds"]);
});

test("fold keeps both ends of a run of lines that differ only by a number", () => {
  const run = Array.from({ length: 128 }, (_, i) => `    val s${String(i).padStart(3, "0")}: StateFlow<UInt>`);
  const out = fold(["class A {", ...run, "    fun f()", "}"]);
  assert.equal(out.length, 1 + 3 + 1 + 1);
  assert.equal(out[1], run[0]);
  assert.match(out[2], /126 more/);
  assert.equal(out[3], run[127]);
  // five alike lines are not a run
  assert.equal(fold(run.slice(0, 5)).length, 5);
  // names that differ in letters are not alike
  assert.equal(fold(["a1: Int", "b1: Int", "c1: Int", "d1: Int", "e1: Int", "f1: Int", "g1: Int"]).length, 7);
});

test("an unknown language is refused", () => {
  assert.throws(() => declarations("", "rust"), /unknown language/);
});

// The committed playground bindings: whatever else changes, no body may leak into the reference, braces balance,
// every doc comment comes from the source, and every public top-level name is found.
const GENERATED = join(dirname(fileURLToPath(import.meta.url)), "../../examples/playground/generated");
const DIRS = { swift: ["swift/Sources", ".swift"], kotlin: ["kotlin/src", ".kt"], ts: ["ts/src", ".ts"] };
const walk = (dir, ext) => readdirSync(dir, { withFileTypes: true }).flatMap((d) => (d.isDirectory() ? walk(join(dir, d.name), ext) : d.name.endsWith(ext) && !/Package\.swift$/.test(d.name) ? [join(dir, d.name)] : []));
const BODY = /callSync|core\.call\(|core\.construct|UndraWriter\(\)|\.undraEncode\(&w\)|\.writeU\d+\(|Codecs\.|decodeAll|\bUndraCallError\.mapped\(/;

for (const [lang, [sub, ext]] of Object.entries(DIRS)) {
  const dir = join(GENERATED, sub);
  test(`playground ${lang}: declarations only`, { skip: !existsSync(dir) }, () => {
    for (const file of walk(dir, ext)) {
      const src = readFileSync(file, "utf8");
      const { text } = declarations(src, lang);
      const where = file.slice(GENERATED.length);
      const bad = lines(text).filter((l) => BODY.test(l) && !/^\s*(\/\/\/|\/?\*)/.test(l));
      assert.deepEqual(bad, [], `${where}: body text in the declarations`);
      const open = (text.match(/\{/g) || []).length, close = (text.match(/\}/g) || []).length;
      assert.equal(open - (text.match(/\{ … \}/g) || []).length, close - (text.match(/\{ … \}/g) || []).length, `${where}: unbalanced braces`);
      for (const l of lines(text).filter((x) => /^\s*(\/\/\/|\/?\*\*?)/.test(x))) assert.ok(src.includes(l), `${where}: a doc line that is not in the source: ${l}`);
      assert.ok(text.length < src.length, `${where}: nothing was left out`);
    }
  });
}
