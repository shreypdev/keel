import { type Codec, codecs } from "../src/wire/codec.js";
import { WireError } from "../src/wire/errors.js";

/*
 * Hand-written codecs shaped like the code `undra-bindgen` generates for the
 * records and enums of the contract vectors: a record is its fields in
 * declaration order, an enum is a u16 variant index plus the variant's fields.
 */

export interface Todo {
  id: string;
  title: string;
  done: boolean;
}

export const todoCodec: Codec<Todo> = {
  encode(w, v) {
    codecs.uuid.encode(w, v.id);
    codecs.string.encode(w, v.title);
    codecs.bool.encode(w, v.done);
  },
  decode(r) {
    const id = codecs.uuid.decode(r);
    const title = codecs.string.decode(r);
    const done = codecs.bool.decode(r);
    return { id, title, done };
  },
};

export type Filter = "all" | "active" | "done";
const FILTERS: readonly Filter[] = ["all", "active", "done"];

export const filterCodec: Codec<Filter> = {
  encode(w, v) {
    w.writeU16(FILTERS.indexOf(v));
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    const v = FILTERS[tag];
    if (v === undefined) throw new WireError({ code: "invalid_tag", tag, at, ty: "Filter" });
    return v;
  },
};

export type Shape = { kind: "circle"; radius: number } | { kind: "rect"; w: number; h: number };

export const shapeCodec: Codec<Shape> = {
  encode(w, v) {
    switch (v.kind) {
      case "circle":
        w.writeU16(0);
        w.writeF64(v.radius);
        break;
      case "rect":
        w.writeU16(1);
        w.writeF64(v.w);
        w.writeF64(v.h);
        break;
    }
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return { kind: "circle", radius: r.readF64() };
      case 1: {
        const w = r.readF64();
        return { kind: "rect", w, h: r.readF64() };
      }
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "Shape" });
    }
  },
};
