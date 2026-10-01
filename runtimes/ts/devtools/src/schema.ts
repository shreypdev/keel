/** The part of the schema document (`Schema::to_json`, SPEC 2) the page reads, and an index over it. */

export type TypeRef =
  | { readonly kind: "bool" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" }
  | { readonly kind: "string" | "bytes" | "unit" | "duration" | "timestamp" | "uuid" }
  | { readonly kind: "option" | "vec" | "lazy" | "stream"; readonly of: TypeRef }
  | { readonly kind: "map" | "result"; readonly of: readonly [TypeRef, TypeRef] }
  | { readonly kind: "named"; readonly of: string };

export interface FieldDef {
  readonly name: string;
  readonly ty: TypeRef;
}

export interface RecordDef {
  readonly name: string;
  readonly type_id: number;
  readonly fields: readonly FieldDef[];
}

export interface VariantDef {
  readonly name: string;
  readonly index: number;
  readonly fields: readonly FieldDef[];
  readonly tuple: boolean;
}

export interface EnumDef {
  readonly name: string;
  readonly type_id: number;
  readonly is_error: boolean;
  readonly variants: readonly VariantDef[];
}

export interface ParamDef {
  readonly name: string;
  readonly ty: TypeRef;
}

export interface MethodDef {
  readonly name: string;
  readonly method_id: number;
  readonly params: readonly ParamDef[];
  readonly returns: TypeRef;
  readonly is_async: boolean;
}

export interface SignalDef {
  readonly name: string;
  readonly signal_id: number;
  readonly ty: TypeRef;
  readonly computed: boolean;
  readonly key: string | null;
}

export interface ObjectDef {
  readonly name: string;
  readonly type_id: number;
  readonly constructors: readonly MethodDef[];
  readonly methods: readonly MethodDef[];
  readonly store: { readonly signals: readonly SignalDef[] } | null;
}

export interface PortDef {
  readonly name: string;
  readonly port_id: number;
  readonly kind: "sync" | "async" | "event";
  readonly methods: readonly MethodDef[];
}

export interface QueryDef {
  readonly name: string;
  readonly query_id: number;
  readonly kind: "query" | "mutation";
  readonly key: string;
  readonly params: readonly ParamDef[];
  readonly returns: TypeRef;
}

export interface Schema {
  readonly crate_name: string;
  readonly records: readonly RecordDef[];
  readonly enums: readonly EnumDef[];
  readonly objects: readonly ObjectDef[];
  readonly functions: readonly (MethodDef & { readonly name: string })[];
  readonly ports: readonly PortDef[];
  readonly queries: readonly QueryDef[];
}

/** What a method id names: the type or port it belongs to, and its definition. */
export interface MethodRef {
  readonly owner: string;
  readonly def: MethodDef;
}

/** Lookups over a schema, built once per welcome. */
export class SchemaIndex {
  readonly records = new Map<string, RecordDef>();
  readonly enums = new Map<string, EnumDef>();
  readonly objectsByName = new Map<string, ObjectDef>();
  readonly objectsByTypeId = new Map<number, ObjectDef>();
  readonly methods = new Map<number, MethodRef>();
  readonly ports = new Map<number, PortDef>();
  readonly queries = new Map<number, QueryDef>();

  constructor(readonly schema: Schema) {
    for (const r of schema.records) this.records.set(r.name, r);
    for (const e of schema.enums) this.enums.set(e.name, e);
    for (const o of schema.objects) {
      this.objectsByName.set(o.name, o);
      this.objectsByTypeId.set(o.type_id, o);
      for (const m of [...o.constructors, ...o.methods]) this.methods.set(m.method_id, { owner: o.name, def: m });
    }
    for (const f of schema.functions) this.methods.set(f.method_id, { owner: "", def: f });
    for (const p of schema.ports) this.ports.set(p.port_id, p);
    for (const q of schema.queries) this.queries.set(q.query_id, q);
  }

  /** `Counter.add`, or `sum` for a free function; the id in hex when the schema does not know it. */
  methodName(methodId: number): string {
    const m = this.methods.get(methodId);
    if (m === undefined) return `method ${hex(methodId)}`;
    return m.owner === "" ? m.def.name : `${m.owner}.${m.def.name}`;
  }

  /** `Http.request`, with the ids in hex when the schema does not know them. */
  portMethodName(portId: number, methodId: number): string {
    const port = this.ports.get(portId);
    if (port === undefined) return `port ${hex(portId)}.${hex(methodId)}`;
    const method = port.methods.find((m) => m.method_id === methodId);
    return `${port.name}.${method?.name ?? hex(methodId)}`;
  }

  portMethod(portId: number, methodId: number): MethodDef | undefined {
    return this.ports.get(portId)?.methods.find((m) => m.method_id === methodId);
  }
}

/** Parses the schema document of a welcome. */
export function parseSchema(json: string): SchemaIndex {
  const raw = JSON.parse(json) as Partial<Schema>;
  return new SchemaIndex({
    crate_name: raw.crate_name ?? "",
    records: raw.records ?? [],
    enums: raw.enums ?? [],
    objects: raw.objects ?? [],
    functions: raw.functions ?? [],
    ports: raw.ports ?? [],
    queries: raw.queries ?? [],
  });
}

/** `0x00ab12cd`. */
export function hex(n: number): string {
  return `0x${n.toString(16).padStart(8, "0")}`;
}

/** The type as Rust-ish text: `Vec<Todo>`, `Option<String>`. */
export function typeName(ty: TypeRef): string {
  switch (ty.kind) {
    case "option":
      return `Option<${typeName(ty.of)}>`;
    case "vec":
      return `Vec<${typeName(ty.of)}>`;
    case "lazy":
      return `Lazy<${typeName(ty.of)}>`;
    case "stream":
      return `Stream<${typeName(ty.of)}>`;
    case "map":
      return `Map<${typeName(ty.of[0])}, ${typeName(ty.of[1])}>`;
    case "result":
      return `Result<${typeName(ty.of[0])}, ${typeName(ty.of[1])}>`;
    case "named":
      return ty.of;
    default:
      return ty.kind;
  }
}
