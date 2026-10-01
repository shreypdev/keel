//! The JSON of a closure (ADR-037), written and read without `serde`.
//!
//! A closure's canonical JSON is computed in every core (a snapshot's fingerprints and
//! description) and read back by a restore whose types changed, so both directions ship in every
//! core, the web one included. `serde`'s derived impls for these types, and `serde_json`'s
//! deserializer with them, came to about 100 KB of wasm (before `wasm-opt`) in the hello-world
//! core (ADR-052's budget); this module is a small fraction of that. The output is byte for byte
//! what `serde_json::to_string` writes for the same values (the tests compare them), so the
//! fingerprints do not depend on which one computed them.
//!
//! The reader accepts any JSON with the closure's shape: whitespace, keys in any order, unknown
//! keys ignored (as `serde` does), a missing `default` flag read as `false`. Nesting is bounded
//! ([`MAX_DEPTH`]) and every count is bounded by the input's length.

use core::fmt;

use crate::TypeRef;
use crate::closure::{
    ClosureEnum, ClosureField, ClosureRecord, ClosureRoot, ClosureSignal, ClosureVariant,
    DescribedStore, StoresClosure, TypeClosure,
};

/// Why a persisted closure (a snapshot's description, an `undra.types.<fingerprint>` value) could
/// not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosureJsonError {
    /// The byte offset in the JSON text where reading stopped (the end of the value whose shape
    /// was wrong, for a shape error).
    pub at: usize,
    /// What was wrong.
    pub reason: &'static str,
}

impl fmt::Display for ClosureJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.reason, self.at)
    }
}

impl std::error::Error for ClosureJsonError {}

/// How deeply values may nest: a type nested deeper than this is refused rather than recursed
/// into (a closure's own nesting is a handful of levels plus its types' nesting).
pub const MAX_DEPTH: usize = 128;

// ----- writing ---------------------------------------------------------------------------------

pub(crate) fn write_type_closure(c: &TypeClosure) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("{\"root\":");
    match &c.root {
        ClosureRoot::Type { ty } => {
            out.push_str("{\"kind\":\"type\",\"ty\":");
            type_ref(&mut out, ty);
        }
        ClosureRoot::Params { params } => {
            out.push_str("{\"kind\":\"params\",\"params\":");
            list(&mut out, params, field);
        }
        ClosureRoot::Signals { signals } => {
            out.push_str("{\"kind\":\"signals\",\"signals\":");
            list(&mut out, signals, signal);
        }
    }
    out.push('}');
    records_and_enums(&mut out, &c.records, &c.enums);
    out
}

pub(crate) fn write_stores_closure(c: &StoresClosure) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("{\"stores\":");
    list(&mut out, &c.stores, |out, s| {
        out.push_str("{\"type_id\":");
        number(out, u64::from(s.type_id));
        out.push_str(",\"name\":");
        string(out, &s.name);
        out.push_str(",\"signals\":");
        list(out, &s.signals, signal);
        out.push('}');
    });
    records_and_enums(&mut out, &c.records, &c.enums);
    out
}

/// `,"records":[..],"enums":[..]}`: the common tail.
fn records_and_enums(out: &mut String, records: &[ClosureRecord], enums: &[ClosureEnum]) {
    out.push_str(",\"records\":");
    list(out, records, |out, r| {
        out.push_str("{\"name\":");
        string(out, &r.name);
        out.push_str(",\"fields\":");
        list(out, &r.fields, field);
        out.push('}');
    });
    out.push_str(",\"enums\":");
    list(out, enums, |out, e| {
        out.push_str("{\"name\":");
        string(out, &e.name);
        out.push_str(",\"variants\":");
        list(out, &e.variants, |out, v| {
            out.push_str("{\"name\":");
            string(out, &v.name);
            out.push_str(",\"index\":");
            number(out, u64::from(v.index));
            out.push_str(",\"fields\":");
            list(out, &v.fields, field);
            out.push_str(if v.tuple {
                ",\"tuple\":true}"
            } else {
                ",\"tuple\":false}"
            });
        });
        out.push('}');
    });
    out.push('}');
}

fn list<T>(out: &mut String, items: &[T], mut item: impl FnMut(&mut String, &T)) {
    out.push('[');
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        item(out, x);
    }
    out.push(']');
}

fn field(out: &mut String, f: &ClosureField) {
    out.push_str("{\"name\":");
    string(out, &f.name);
    out.push_str(",\"ty\":");
    type_ref(out, &f.ty);
    if f.default {
        out.push_str(",\"default\":true");
    }
    out.push('}');
}

fn signal(out: &mut String, s: &ClosureSignal) {
    out.push_str("{\"name\":");
    string(out, &s.name);
    out.push_str(",\"signal_id\":");
    number(out, u64::from(s.signal_id));
    out.push_str(",\"ty\":");
    type_ref(out, &s.ty);
    if s.default {
        out.push_str(",\"default\":true");
    }
    out.push('}');
}

/// `TypeRef` as its `serde` form: `{"kind":"i32"}`, `{"kind":"vec","of":T}`,
/// `{"kind":"map","of":[K,V]}`, `{"kind":"named","of":"Todo"}`.
fn type_ref(out: &mut String, ty: &TypeRef) {
    out.push_str("{\"kind\":\"");
    out.push_str(kind_name(ty));
    out.push('"');
    match ty {
        TypeRef::Option(t) | TypeRef::Vec(t) | TypeRef::Lazy(t) | TypeRef::Stream(t) => {
            out.push_str(",\"of\":");
            type_ref(out, t);
        }
        TypeRef::Map(a, b) | TypeRef::Result(a, b) => {
            out.push_str(",\"of\":[");
            type_ref(out, a);
            out.push(',');
            type_ref(out, b);
            out.push(']');
        }
        TypeRef::Named(name) => {
            out.push_str(",\"of\":");
            string(out, name);
        }
        _ => {}
    }
    out.push('}');
}

/// The `kind` tag of every `TypeRef` variant, in declaration order (`serde`'s `snake_case`).
const KINDS: [&str; 24] = [
    "bool",
    "i8",
    "i16",
    "i32",
    "i64",
    "u8",
    "u16",
    "u32",
    "u64",
    "f32",
    "f64",
    "string",
    "bytes",
    "unit",
    "duration",
    "timestamp",
    "uuid",
    "option",
    "vec",
    "map",
    "lazy",
    "named",
    "result",
    "stream",
];

fn kind_name(ty: &TypeRef) -> &'static str {
    let at = match ty {
        TypeRef::Bool => 0,
        TypeRef::I8 => 1,
        TypeRef::I16 => 2,
        TypeRef::I32 => 3,
        TypeRef::I64 => 4,
        TypeRef::U8 => 5,
        TypeRef::U16 => 6,
        TypeRef::U32 => 7,
        TypeRef::U64 => 8,
        TypeRef::F32 => 9,
        TypeRef::F64 => 10,
        TypeRef::String => 11,
        TypeRef::Bytes => 12,
        TypeRef::Unit => 13,
        TypeRef::Duration => 14,
        TypeRef::Timestamp => 15,
        TypeRef::Uuid => 16,
        TypeRef::Option(_) => 17,
        TypeRef::Vec(_) => 18,
        TypeRef::Map(..) => 19,
        TypeRef::Lazy(_) => 20,
        TypeRef::Named(_) => 21,
        TypeRef::Result(..) => 22,
        TypeRef::Stream(_) => 23,
    };
    KINDS[at]
}

fn number(out: &mut String, n: u64) {
    use fmt::Write as _;
    let _ = write!(out, "{n}");
}

/// A JSON string exactly as `serde_json` escapes it: `"` and `\`, the control characters as
/// `\b \t \n \f \r` or `\u00xx`, everything else (non-ASCII included) as it is.
fn string(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let b = c as u32 as usize;
                out.push_str("\\u00");
                out.push(char::from(HEX[b >> 4]));
                out.push(char::from(HEX[b & 0xf]));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ----- reading ---------------------------------------------------------------------------------

pub(crate) fn read_type_closure(text: &str) -> Result<TypeClosure, ClosureJsonError> {
    let (value, end) = parse(text)?;
    let shape = |reason| ClosureJsonError { at: end, reason };
    let o = value.object().ok_or(shape("a closure is an object"))?;
    let root = req(o, "root", end)?
        .object()
        .ok_or(shape("`root` is an object"))?;
    let root = match req(root, "kind", end)?.str() {
        Some("type") => ClosureRoot::Type {
            ty: type_of(req(root, "ty", end)?, end)?,
        },
        Some("params") => ClosureRoot::Params {
            params: items(req(root, "params", end)?, end, field_of)?,
        },
        Some("signals") => ClosureRoot::Signals {
            signals: items(req(root, "signals", end)?, end, signal_of)?,
        },
        _ => return Err(shape("`root.kind` is type, params or signals")),
    };
    Ok(TypeClosure {
        root,
        records: items(req(o, "records", end)?, end, record_of)?,
        enums: items(req(o, "enums", end)?, end, enum_of)?,
    })
}

pub(crate) fn read_stores_closure(text: &str) -> Result<StoresClosure, ClosureJsonError> {
    let (value, end) = parse(text)?;
    let o = value.object().ok_or(ClosureJsonError {
        at: end,
        reason: "a description is an object",
    })?;
    Ok(StoresClosure {
        stores: items(req(o, "stores", end)?, end, |v, end| {
            let o = obj(v, end)?;
            Ok(DescribedStore {
                type_id: u32_of(req(o, "type_id", end)?, end)?,
                name: str_of(req(o, "name", end)?, end)?,
                signals: items(req(o, "signals", end)?, end, signal_of)?,
            })
        })?,
        records: items(req(o, "records", end)?, end, record_of)?,
        enums: items(req(o, "enums", end)?, end, enum_of)?,
    })
}

/// A parsed JSON value.
enum Json {
    Null,
    Bool(bool),
    /// An unsigned integer: the only numbers a closure holds.
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

type Obj = [(String, Json)];

/// The member `key` (the last one, as `serde_json` keeps when a key repeats).
fn get<'a>(o: &'a Obj, key: &str) -> Option<&'a Json> {
    o.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn req<'a>(o: &'a Obj, key: &'static str, at: usize) -> Result<&'a Json, ClosureJsonError> {
    get(o, key).ok_or(ClosureJsonError {
        at,
        reason: "a required member is missing",
    })
}

fn wrong(at: usize, reason: &'static str) -> ClosureJsonError {
    ClosureJsonError { at, reason }
}

fn obj(v: &Json, at: usize) -> Result<&Obj, ClosureJsonError> {
    v.object().ok_or(wrong(at, "expected an object"))
}

fn str_of(v: &Json, at: usize) -> Result<String, ClosureJsonError> {
    v.str()
        .map(str::to_owned)
        .ok_or(wrong(at, "expected a string"))
}

fn u32_of(v: &Json, at: usize) -> Result<u32, ClosureJsonError> {
    match v {
        Json::Num(n) => u32::try_from(*n).map_err(|_| wrong(at, "a number out of range")),
        _ => Err(wrong(at, "expected a number")),
    }
}

fn bool_of(v: Option<&Json>, at: usize) -> Result<bool, ClosureJsonError> {
    match v {
        None => Ok(false),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(wrong(at, "expected a boolean")),
    }
}

fn items<T>(
    v: &Json,
    at: usize,
    item: impl Fn(&Json, usize) -> Result<T, ClosureJsonError>,
) -> Result<Vec<T>, ClosureJsonError> {
    let Json::Arr(values) = v else {
        return Err(wrong(at, "expected an array"));
    };
    values.iter().map(|v| item(v, at)).collect()
}

fn field_of(v: &Json, at: usize) -> Result<ClosureField, ClosureJsonError> {
    let o = obj(v, at)?;
    Ok(ClosureField {
        name: str_of(req(o, "name", at)?, at)?,
        ty: type_of(req(o, "ty", at)?, at)?,
        default: bool_of(get(o, "default"), at)?,
    })
}

fn signal_of(v: &Json, at: usize) -> Result<ClosureSignal, ClosureJsonError> {
    let o = obj(v, at)?;
    Ok(ClosureSignal {
        name: str_of(req(o, "name", at)?, at)?,
        signal_id: u32_of(req(o, "signal_id", at)?, at)?,
        ty: type_of(req(o, "ty", at)?, at)?,
        default: bool_of(get(o, "default"), at)?,
    })
}

fn record_of(v: &Json, at: usize) -> Result<ClosureRecord, ClosureJsonError> {
    let o = obj(v, at)?;
    Ok(ClosureRecord {
        name: str_of(req(o, "name", at)?, at)?,
        fields: items(req(o, "fields", at)?, at, field_of)?,
    })
}

fn enum_of(v: &Json, at: usize) -> Result<ClosureEnum, ClosureJsonError> {
    let o = obj(v, at)?;
    Ok(ClosureEnum {
        name: str_of(req(o, "name", at)?, at)?,
        variants: items(req(o, "variants", at)?, at, |v, at| {
            let o = obj(v, at)?;
            Ok(ClosureVariant {
                name: str_of(req(o, "name", at)?, at)?,
                index: u16::try_from(u32_of(req(o, "index", at)?, at)?)
                    .map_err(|_| wrong(at, "a variant index out of range"))?,
                fields: items(req(o, "fields", at)?, at, field_of)?,
                tuple: bool_of(Some(req(o, "tuple", at)?), at)?,
            })
        })?,
    })
}

fn type_of(v: &Json, at: usize) -> Result<TypeRef, ClosureJsonError> {
    let o = obj(v, at)?;
    let kind = req(o, "kind", at)?
        .str()
        .ok_or(wrong(at, "`kind` is a string"))?;
    let index = KINDS
        .iter()
        .position(|k| *k == kind)
        .ok_or(wrong(at, "an unknown type kind"))?;
    let of = || req(o, "of", at);
    let one = || Ok::<_, ClosureJsonError>(Box::new(type_of(of()?, at)?));
    let two = || match of()? {
        Json::Arr(pair) if pair.len() == 2 => Ok((
            Box::new(type_of(&pair[0], at)?),
            Box::new(type_of(&pair[1], at)?),
        )),
        _ => Err(wrong(at, "expected a pair of types")),
    };
    Ok(match index {
        0 => TypeRef::Bool,
        1 => TypeRef::I8,
        2 => TypeRef::I16,
        3 => TypeRef::I32,
        4 => TypeRef::I64,
        5 => TypeRef::U8,
        6 => TypeRef::U16,
        7 => TypeRef::U32,
        8 => TypeRef::U64,
        9 => TypeRef::F32,
        10 => TypeRef::F64,
        11 => TypeRef::String,
        12 => TypeRef::Bytes,
        13 => TypeRef::Unit,
        14 => TypeRef::Duration,
        15 => TypeRef::Timestamp,
        16 => TypeRef::Uuid,
        17 => TypeRef::Option(one()?),
        18 => TypeRef::Vec(one()?),
        19 => {
            let (k, v) = two()?;
            TypeRef::Map(k, v)
        }
        20 => TypeRef::Lazy(one()?),
        21 => TypeRef::Named(str_of(of()?, at)?),
        22 => {
            let (k, v) = two()?;
            TypeRef::Result(k, v)
        }
        _ => TypeRef::Stream(one()?),
    })
}

/// Parses one JSON value that fills `text` (whitespace around it allowed); gives back the value
/// and the length of the text.
fn parse(text: &str) -> Result<(Json, usize), ClosureJsonError> {
    let mut p = Parser {
        b: text.as_bytes(),
        at: 0,
    };
    let value = p.value(0)?;
    p.ws();
    if p.at != p.b.len() {
        return Err(p.err("trailing characters after the value"));
    }
    Ok((value, p.b.len()))
}

struct Parser<'a> {
    b: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn err(&self, reason: &'static str) -> ClosureJsonError {
        ClosureJsonError {
            at: self.at,
            reason,
        }
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.b.get(self.at) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.ws();
        if self.b.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &[u8], value: Json) -> Result<Json, ClosureJsonError> {
        if self.b[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.err("an unexpected character"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ClosureJsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        self.ws();
        match self.b.get(self.at) {
            None => Err(self.err("the text ends before the value")),
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                if self.eat(b'}') {
                    return Ok(Json::Obj(members));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    if !self.eat(b':') {
                        return Err(self.err("expected `:`"));
                    }
                    members.push((key, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Ok(Json::Obj(members));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("expected `,` or `}`"));
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut values = Vec::new();
                if self.eat(b']') {
                    return Ok(Json::Arr(values));
                }
                loop {
                    values.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Ok(Json::Arr(values));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("expected `,` or `]`"));
                    }
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal(b"true", Json::Bool(true)),
            Some(b'f') => self.literal(b"false", Json::Bool(false)),
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b'0'..=b'9') => {
                let start = self.at;
                let mut n: u64 = 0;
                while let Some(&d @ b'0'..=b'9') = self.b.get(self.at) {
                    n = n
                        .checked_mul(10)
                        .and_then(|n| n.checked_add(u64::from(d - b'0')))
                        .ok_or(self.err("a number out of range"))?;
                    self.at += 1;
                }
                if matches!(self.b.get(self.at), Some(b'.' | b'e' | b'E'))
                    || (self.at - start > 1 && self.b[start] == b'0')
                {
                    return Err(self.err("a closure holds only unsigned integers"));
                }
                Ok(Json::Num(n))
            }
            Some(_) => Err(self.err("an unexpected character")),
        }
    }

    /// A string at `self.at` (which must be its opening quote), unescaped.
    fn string(&mut self) -> Result<String, ClosureJsonError> {
        if self.b.get(self.at) != Some(&b'"') {
            return Err(self.err("expected a string"));
        }
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let Some(&byte) = self.b.get(self.at) else {
                return Err(self.err("an unterminated string"));
            };
            self.at += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let Some(&esc) = self.b.get(self.at) else {
                        return Err(self.err("an unterminated string"));
                    };
                    self.at += 1;
                    let c = match esc {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{08}',
                        b'f' => '\u{0c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode()?,
                        _ => return Err(self.err("an invalid escape")),
                    };
                    let mut buf = [0; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return Err(self.err("a control character in a string")),
                _ => out.push(byte),
            }
        }
        // The input is a `&str` and escapes produce whole characters, so the bytes are UTF-8.
        String::from_utf8(out).map_err(|_| self.err("a string that is not UTF-8"))
    }

    /// The character of a `\uXXXX` escape (a surrogate pair takes two).
    fn unicode(&mut self) -> Result<char, ClosureJsonError> {
        let high = self.hex4()?;
        let code = if (0xd800..0xdc00).contains(&high) {
            if !self.b[self.at..].starts_with(b"\\u") {
                return Err(self.err("a lone surrogate"));
            }
            self.at += 2;
            let low = self.hex4()?;
            if !(0xdc00..0xe000).contains(&low) {
                return Err(self.err("a lone surrogate"));
            }
            0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
        } else {
            high
        };
        char::from_u32(code).ok_or(self.err("a lone surrogate"))
    }

    fn hex4(&mut self) -> Result<u32, ClosureJsonError> {
        let digits = self
            .b
            .get(self.at..self.at + 4)
            .ok_or(self.err("a short `\\u` escape"))?;
        let mut n = 0;
        for &d in digits {
            let v = match d {
                b'0'..=b'9' => d - b'0',
                b'a'..=b'f' => d - b'a' + 10,
                b'A'..=b'F' => d - b'A' + 10,
                _ => return Err(self.err("an invalid `\\u` escape")),
            };
            n = n * 16 + u32::from(v);
        }
        self.at += 4;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::closure::{ClosureRoot, TypeClosure};

    fn every_type() -> Vec<TypeRef> {
        let mut types = vec![
            TypeRef::Bool,
            TypeRef::I8,
            TypeRef::I16,
            TypeRef::I32,
            TypeRef::I64,
            TypeRef::U8,
            TypeRef::U16,
            TypeRef::U32,
            TypeRef::U64,
            TypeRef::F32,
            TypeRef::F64,
            TypeRef::String,
            TypeRef::Bytes,
            TypeRef::Unit,
            TypeRef::Duration,
            TypeRef::Timestamp,
            TypeRef::Uuid,
            TypeRef::named("Todo"),
        ];
        types.push(TypeRef::option(TypeRef::named(
            "Ünïcødé \"q\" \\ \u{1}\t\u{7f}🦀",
        )));
        types.push(TypeRef::vec(TypeRef::option(TypeRef::U8)));
        types.push(TypeRef::map(TypeRef::String, TypeRef::vec(TypeRef::I64)));
        types.push(TypeRef::lazy(TypeRef::named("Row")));
        types.push(TypeRef::result(TypeRef::Unit, TypeRef::named("E")));
        types.push(TypeRef::Stream(Box::new(TypeRef::Bytes)));
        types
    }

    fn sample() -> TypeClosure {
        let fields: Vec<ClosureField> = every_type()
            .into_iter()
            .enumerate()
            .map(|(i, ty)| ClosureField {
                name: format!("f{i}\n"),
                ty,
                default: i % 3 == 0,
            })
            .collect();
        TypeClosure {
            root: ClosureRoot::Params {
                params: fields.clone(),
            },
            records: vec![ClosureRecord {
                name: "Todo".into(),
                fields: fields.clone(),
            }],
            enums: vec![ClosureEnum {
                name: "E".into(),
                variants: vec![
                    ClosureVariant {
                        name: "A".into(),
                        index: 0,
                        fields: vec![],
                        tuple: false,
                    },
                    ClosureVariant {
                        name: "B".into(),
                        index: 65_535,
                        fields,
                        tuple: true,
                    },
                ],
            }],
        }
    }

    #[test]
    fn the_writer_writes_what_serde_json_writes() {
        let mut c = sample();
        assert_eq!(write_type_closure(&c), serde_json::to_string(&c).unwrap());
        for ty in every_type() {
            c.root = ClosureRoot::Type { ty };
            assert_eq!(write_type_closure(&c), serde_json::to_string(&c).unwrap());
        }
        c.root = ClosureRoot::Signals {
            signals: vec![ClosureSignal {
                name: "s".into(),
                signal_id: u32::MAX,
                ty: TypeRef::named("Todo"),
                default: true,
            }],
        };
        assert_eq!(write_type_closure(&c), serde_json::to_string(&c).unwrap());
        let stores = StoresClosure {
            stores: vec![DescribedStore {
                type_id: 7,
                name: "Profile".into(),
                signals: vec![],
            }],
            records: c.records.clone(),
            enums: c.enums.clone(),
        };
        assert_eq!(
            write_stores_closure(&stores),
            serde_json::to_string(&stores).unwrap()
        );
        let control: String = (0u8..0x20).map(char::from).collect();
        let mut out = String::new();
        string(&mut out, &control);
        assert_eq!(out, serde_json::to_string(&control).unwrap());
    }

    #[test]
    fn the_reader_reads_what_serde_json_reads() {
        let c = sample();
        assert_eq!(read_type_closure(&write_type_closure(&c)).unwrap(), c);
        let pretty = serde_json::to_string_pretty(&c).unwrap();
        assert_eq!(read_type_closure(&pretty).unwrap(), c, "whitespace");
        // Keys in another order, an unknown key, an escaped slash and a surrogate pair.
        let shuffled = r#"{"enums":[],"extra":[1,{"x":null}],"records":[{"fields":[],"name":"R\/\ud83e\udd80"}],
            "root":{"ty":{"of":{"kind":"u8"},"kind":"vec"},"kind":"type"}}"#;
        let back = read_type_closure(shuffled).unwrap();
        assert_eq!(back.records[0].name, "R/🦀");
        assert_eq!(
            back.root,
            ClosureRoot::Type {
                ty: TypeRef::vec(TypeRef::U8)
            }
        );
        let stores = StoresClosure {
            stores: vec![DescribedStore {
                type_id: u32::MAX,
                name: "P".into(),
                signals: vec![ClosureSignal {
                    name: "s".into(),
                    signal_id: 3,
                    ty: TypeRef::I32,
                    default: false,
                }],
            }],
            records: vec![],
            enums: vec![],
        };
        assert_eq!(
            read_stores_closure(&serde_json::to_string(&stores).unwrap()).unwrap(),
            stores
        );
    }

    #[test]
    fn the_reader_refuses_what_is_not_a_closure() {
        let ok = write_type_closure(&sample());
        for bad in [
            "",
            "{",
            "[]",
            "{not json",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"i32\"}},\"records\":[]}",
            "{\"root\":{\"kind\":\"nope\"},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"i33\"}},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"map\",\"of\":[{\"kind\":\"u8\"}]}},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"signals\",\"signals\":[{\"name\":\"s\",\"signal_id\":-1,\"ty\":{\"kind\":\"u8\"}}]},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"signals\",\"signals\":[{\"name\":\"s\",\"signal_id\":4294967296,\"ty\":{\"kind\":\"u8\"}}]},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"signals\",\"signals\":[{\"name\":\"s\",\"signal_id\":1.5,\"ty\":{\"kind\":\"u8\"}}]},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"named\",\"of\":\"\\ud800\"}},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"named\",\"of\":\"\\x\"}},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"named\",\"of\":\"a\nb\"}},\"records\":[],\"enums\":[]}",
            "{\"root\":{\"kind\":\"type\",\"ty\":{\"kind\":\"bool\"}},\"records\":[],\"enums\":[]} x",
        ] {
            assert!(read_type_closure(bad).is_err(), "{bad}");
        }
        for cut in (0..ok.len()).filter(|&cut| ok.is_char_boundary(cut)) {
            assert!(read_type_closure(&ok[..cut]).is_err(), "cut at {cut}");
        }
        let deep = format!(
            "{}{{\"kind\":\"u8\"}}{}",
            "{\"kind\":\"vec\",\"of\":".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        );
        let deep =
            format!("{{\"root\":{{\"kind\":\"type\",\"ty\":{deep}}},\"records\":[],\"enums\":[]}}");
        let err = read_type_closure(&deep).unwrap_err();
        assert_eq!(err.reason, "nested too deeply");
        assert!(err.to_string().contains("nested too deeply (at byte"));
    }
}
