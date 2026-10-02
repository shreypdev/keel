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
//! The reader reads the canonical form only (exactly what the writer writes), so it is the writer
//! in reverse rather than a JSON parser: a description is only ever written by a core, and a
//! restore checks it against its fingerprint. Nesting is bounded ([`MAX_DEPTH`]) and every count
//! is bounded by the input's length.

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
        if r.transparent {
            out.push_str(",\"transparent\":true");
        }
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
pub(crate) fn type_ref(out: &mut String, ty: &TypeRef) {
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
        TypeRef::Named(name) | TypeRef::Object(name) | TypeRef::Callback(name) => {
            out.push_str(",\"of\":");
            string(out, name);
        }
        _ => {}
    }
    out.push('}');
}

/// The types without parameters, in the order of [`KINDS`].
const LEAVES: [TypeRef; 18] = [
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
    TypeRef::Decimal,
];

/// The `kind` tag of every `TypeRef` variant, in declaration order (`serde`'s `snake_case`).
const KINDS: [&str; 27] = [
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
    "decimal",
    "option",
    "vec",
    "map",
    "lazy",
    "named",
    "result",
    "stream",
    "object",
    "callback",
];

pub(crate) fn kind_name(ty: &TypeRef) -> &'static str {
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
        TypeRef::Decimal => 17,
        TypeRef::Option(_) => 18,
        TypeRef::Vec(_) => 19,
        TypeRef::Map(..) => 20,
        TypeRef::Lazy(_) => 21,
        TypeRef::Named(_) => 22,
        TypeRef::Result(..) => 23,
        TypeRef::Stream(_) => 24,
        TypeRef::Object(_) => 25,
        TypeRef::Callback(_) => 26,
    };
    KINDS[at]
}

/// `n` in decimal, as `serde_json` writes an unsigned integer. Written digit by digit: a
/// schema's canonical form holds a few hundred ids, and `fmt`'s machinery for each was a
/// measurable part of hashing one at start-up.
pub(crate) fn number(out: &mut String, mut n: u64) {
    // `u64::MAX` has 20 digits.
    let mut digits = [0_u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for &digit in &digits[at..] {
        out.push(char::from(digit));
    }
}

/// A JSON string exactly as `serde_json` escapes it: `"` and `\`, the control characters as
/// `\b \t \n \f \r` or `\u00xx`, everything else (non-ASCII included) as it is.
pub(crate) fn string(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    // Nearly every string is a name with nothing to escape: copied whole. (Every byte of a
    // multi-byte character is 0x80 or above, so the test below never splits one.)
    if s.bytes().all(|b| b >= 0x20 && b != b'"' && b != b'\\') {
        out.push_str(s);
        out.push('"');
        return;
    }
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
    let mut c = Cursor::new(text);
    c.lit("{\"root\":{\"kind\":")?;
    let mut closure = TypeClosure {
        root: ClosureRoot::Params { params: Vec::new() },
        records: Vec::new(),
        enums: Vec::new(),
    };
    match c.string()?.as_str() {
        "type" => {
            c.lit(",\"ty\":")?;
            closure.root = ClosureRoot::Type { ty: c.ty(0)? };
        }
        "params" => {
            c.lit(",\"params\":")?;
            if let ClosureRoot::Params { params } = &mut closure.root {
                c.list(params, Cursor::field)?;
            }
        }
        "signals" => {
            c.lit(",\"signals\":")?;
            let mut signals = Vec::new();
            c.list(&mut signals, Cursor::signal)?;
            closure.root = ClosureRoot::Signals { signals };
        }
        _ => return Err(c.err("`root.kind` is type, params or signals")),
    }
    c.lit("}")?;
    c.records_and_enums(&mut closure.records, &mut closure.enums)?;
    Ok(closure)
}

pub(crate) fn read_stores_closure(text: &str) -> Result<StoresClosure, ClosureJsonError> {
    let mut c = Cursor::new(text);
    let mut closure = StoresClosure {
        stores: Vec::new(),
        records: Vec::new(),
        enums: Vec::new(),
    };
    c.lit("{\"stores\":")?;
    c.list(&mut closure.stores, |c| {
        // Filled in place, so a refusal drops one value whatever it holds so far.
        let mut store = DescribedStore {
            type_id: 0,
            name: String::new(),
            signals: Vec::new(),
        };
        c.lit("{\"type_id\":")?;
        store.type_id = c.u32()?;
        c.lit(",\"name\":")?;
        store.name = c.string()?;
        c.lit(",\"signals\":")?;
        c.list(&mut store.signals, Cursor::signal)?;
        c.lit("}")?;
        Ok(store)
    })?;
    c.records_and_enums(&mut closure.records, &mut closure.enums)?;
    Ok(closure)
}

/// Reads the canonical form and nothing else: the exact text the writer produces (keys in their
/// order, no whitespace, a `default` flag only when `true`). A description is only ever written
/// by a core, and a restore checks it against its fingerprint; refusing every other spelling keeps
/// this reader the writer in reverse, a fraction of a general JSON parser's size.
struct Cursor<'a> {
    text: &'a str,
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn new(text: &str) -> Cursor<'_> {
        Cursor {
            text,
            b: text.as_bytes(),
            at: 0,
        }
    }

    fn err(&self, reason: &'static str) -> ClosureJsonError {
        ClosureJsonError {
            at: self.at,
            reason,
        }
    }

    /// Whether `text` comes next; consumes it if so. Out of line: called at every key.
    #[inline(never)]
    fn eat(&mut self, text: &str) -> bool {
        let ok = self.b[self.at..].starts_with(text.as_bytes());
        if ok {
            self.at += text.len();
        }
        ok
    }

    #[inline(never)]
    fn lit(&mut self, text: &str) -> Result<(), ClosureJsonError> {
        if self.eat(text) {
            Ok(())
        } else {
            Err(self.err("not a closure's canonical JSON"))
        }
    }

    /// A list, its items pushed onto `out`.
    fn list<T>(
        &mut self,
        out: &mut Vec<T>,
        mut item: impl FnMut(&mut Self) -> Result<T, ClosureJsonError>,
    ) -> Result<(), ClosureJsonError> {
        self.lit("[")?;
        if self.eat("]") {
            return Ok(());
        }
        loop {
            out.push(item(self)?);
            if self.eat("]") {
                return Ok(());
            }
            self.lit(",")?;
        }
    }

    /// `,"records":[..],"enums":[..]}` and the end of the text.
    fn records_and_enums(
        &mut self,
        records: &mut Vec<ClosureRecord>,
        enums: &mut Vec<ClosureEnum>,
    ) -> Result<(), ClosureJsonError> {
        self.lit(",\"records\":")?;
        self.list(records, |c| {
            let mut record = ClosureRecord {
                name: String::new(),
                fields: Vec::new(),
                transparent: false,
            };
            c.lit("{\"name\":")?;
            record.name = c.string()?;
            c.lit(",\"fields\":")?;
            c.list(&mut record.fields, Cursor::field)?;
            record.transparent = c.eat(",\"transparent\":true");
            c.lit("}")?;
            Ok(record)
        })?;
        self.lit(",\"enums\":")?;
        self.list(enums, |c| {
            let mut en = ClosureEnum {
                name: String::new(),
                variants: Vec::new(),
            };
            c.lit("{\"name\":")?;
            en.name = c.string()?;
            c.lit(",\"variants\":")?;
            c.list(&mut en.variants, |c| {
                let mut v = ClosureVariant {
                    name: String::new(),
                    index: 0,
                    fields: Vec::new(),
                    tuple: false,
                };
                c.lit("{\"name\":")?;
                v.name = c.string()?;
                c.lit(",\"index\":")?;
                v.index = u16::try_from(c.u32()?).map_err(|_| c.err("a number out of range"))?;
                c.lit(",\"fields\":")?;
                c.list(&mut v.fields, Cursor::field)?;
                v.tuple = c.eat(",\"tuple\":true}");
                if !v.tuple {
                    c.lit(",\"tuple\":false}")?;
                }
                Ok(v)
            })?;
            c.lit("}")?;
            Ok(en)
        })?;
        self.lit("}")?;
        if self.at != self.b.len() {
            return Err(self.err("trailing characters after the value"));
        }
        Ok(())
    }

    fn field(&mut self) -> Result<ClosureField, ClosureJsonError> {
        let mut field = ClosureField {
            name: String::new(),
            ty: TypeRef::Unit,
            default: false,
        };
        self.lit("{\"name\":")?;
        field.name = self.string()?;
        self.lit(",\"ty\":")?;
        field.ty = self.ty(0)?;
        field.default = self.eat(",\"default\":true");
        self.lit("}")?;
        Ok(field)
    }

    fn signal(&mut self) -> Result<ClosureSignal, ClosureJsonError> {
        let mut signal = ClosureSignal {
            name: String::new(),
            signal_id: 0,
            ty: TypeRef::Unit,
            default: false,
        };
        self.lit("{\"name\":")?;
        signal.name = self.string()?;
        self.lit(",\"signal_id\":")?;
        signal.signal_id = self.u32()?;
        self.lit(",\"ty\":")?;
        signal.ty = self.ty(0)?;
        signal.default = self.eat(",\"default\":true");
        self.lit("}")?;
        Ok(signal)
    }

    fn ty(&mut self, depth: usize) -> Result<TypeRef, ClosureJsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        self.lit("{\"kind\":")?;
        let kind = self.string()?;
        let index = KINDS
            .iter()
            .position(|k| *k == kind)
            .ok_or(self.err("an unknown type kind"))?;
        let ty = match index {
            0..=17 => LEAVES[index].clone(),
            22 | 25 | 26 => {
                self.lit(",\"of\":")?;
                let name = self.string()?;
                match index {
                    22 => TypeRef::Named(name),
                    25 => TypeRef::Object(name),
                    _ => TypeRef::Callback(name),
                }
            }
            20 | 23 => {
                self.lit(",\"of\":[")?;
                let a = Box::new(self.ty(depth + 1)?);
                self.lit(",")?;
                let b = Box::new(self.ty(depth + 1)?);
                self.lit("]")?;
                if index == 20 {
                    TypeRef::Map(a, b)
                } else {
                    TypeRef::Result(a, b)
                }
            }
            _ => {
                self.lit(",\"of\":")?;
                let inner = Box::new(self.ty(depth + 1)?);
                match index {
                    18 => TypeRef::Option(inner),
                    19 => TypeRef::Vec(inner),
                    21 => TypeRef::Lazy(inner),
                    _ => TypeRef::Stream(inner),
                }
            }
        };
        self.lit("}")?;
        Ok(ty)
    }

    /// An unsigned integer as the writer spells it (no sign, no leading zero, no fraction).
    fn u32(&mut self) -> Result<u32, ClosureJsonError> {
        let start = self.at;
        let mut n: u32 = 0;
        while let Some(&d @ b'0'..=b'9') = self.b.get(self.at) {
            n = n
                .checked_mul(10)
                .and_then(|n| n.checked_add(u32::from(d - b'0')))
                .ok_or(self.err("a number out of range"))?;
            self.at += 1;
        }
        if self.at == start || (self.at - start > 1 && self.b[start] == b'0') {
            return Err(self.err("not a closure's canonical JSON"));
        }
        Ok(n)
    }

    /// A string, unescaped (every escape JSON has, though the writer uses only some). The text
    /// between escapes is copied as it is: it is part of a `&str` and splits only at ASCII.
    fn string(&mut self) -> Result<String, ClosureJsonError> {
        self.lit("\"")?;
        let mut out = String::new();
        let mut run = self.at;
        loop {
            let Some(&byte) = self.b.get(self.at) else {
                return Err(self.err("an unterminated string"));
            };
            match byte {
                b'"' | b'\\' => {
                    out.push_str(&self.text[run..self.at]);
                    self.at += 1;
                    if byte == b'"' {
                        return Ok(out);
                    }
                    let Some(&esc) = self.b.get(self.at) else {
                        return Err(self.err("an unterminated string"));
                    };
                    self.at += 1;
                    out.push(match esc {
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
                    });
                    run = self.at;
                }
                0..=0x1f => return Err(self.err("a control character in a string")),
                _ => self.at += 1,
            }
        }
    }

    /// The character of a `\uXXXX` escape (a surrogate pair takes two).
    fn unicode(&mut self) -> Result<char, ClosureJsonError> {
        let high = self.hex4()?;
        let code = if (0xd800..0xdc00).contains(&high) {
            self.lit("\\u")?;
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
pub(crate) mod tests {
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
            TypeRef::Decimal,
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
            records: vec![
                ClosureRecord {
                    name: "Todo".into(),
                    fields: fields.clone(),
                    transparent: false,
                },
                ClosureRecord {
                    name: "UserId".into(),
                    fields: vec![ClosureField {
                        name: "value".into(),
                        ty: TypeRef::Uuid,
                        default: false,
                    }],
                    transparent: true,
                },
            ],
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
    fn the_reader_reads_what_serde_json_writes_and_only_that() {
        let c = sample();
        assert_eq!(read_type_closure(&write_type_closure(&c)).unwrap(), c);
        assert_eq!(
            read_type_closure(&serde_json::to_string(&c).unwrap()).unwrap(),
            c
        );
        // Escapes the writer does not produce still read: a slash and a surrogate pair.
        let escaped = r#"{"root":{"kind":"type","ty":{"kind":"named","of":"R\/\ud83e\udd80"}},"records":[],"enums":[]}"#;
        assert_eq!(
            read_type_closure(escaped).unwrap().root,
            ClosureRoot::Type {
                ty: TypeRef::named("R/🦀")
            }
        );
        // Another spelling of the same closure is refused: whitespace, key order, a `false` flag.
        let pretty = serde_json::to_string_pretty(&c).unwrap();
        assert_eq!(
            read_type_closure(&pretty).unwrap_err().reason,
            "not a closure's canonical JSON"
        );
        assert!(
            read_type_closure(
                r#"{"records":[],"enums":[],"root":{"kind":"type","ty":{"kind":"u8"}}}"#
            )
            .is_err()
        );
        assert!(read_type_closure(r#"{"root":{"kind":"params","params":[{"name":"a","ty":{"kind":"u8"},"default":false}]},"records":[],"enums":[]}"#).is_err());
        let stores = StoresClosure {
            stores: vec![DescribedStore {
                type_id: u32::MAX,
                name: "P".into(),
                signals: vec![ClosureSignal {
                    name: "s".into(),
                    signal_id: 3,
                    ty: TypeRef::I32,
                    default: true,
                }],
            }],
            records: c.records.clone(),
            enums: c.enums.clone(),
        };
        assert_eq!(
            read_stores_closure(&serde_json::to_string(&stores).unwrap()).unwrap(),
            stores
        );
    }

    // ----- review (2026-10-02): differential against serde_json on generated closures -------

    use proptest::prelude::*;

    /// Names with what the writer must escape and what it must not: quotes, backslashes, every
    /// control character, DEL, non-ASCII, astral characters, the empty string.
    pub(crate) fn arb_name() -> BoxedStrategy<String> {
        prop_oneof![
            "[a-zA-Z_][a-zA-Z0-9_]{0,8}",
            any::<String>(),
            proptest::collection::vec(
                prop_oneof![
                    Just('"'),
                    Just('\\'),
                    Just('/'),
                    (0_u32..0x20).prop_map(|c| char::from_u32(c).unwrap()),
                    Just('\u{7f}'),
                    Just('\u{2028}'),
                    Just('é'),
                    Just('🦀'),
                    Just('\u{fffd}'),
                ],
                0..6
            )
            .prop_map(|cs| cs.into_iter().collect()),
        ]
        .boxed()
    }

    pub(crate) fn arb_ty() -> BoxedStrategy<TypeRef> {
        let leaf = prop_oneof![
            (0_usize..LEAVES.len()).prop_map(|i| LEAVES[i].clone()),
            arb_name().prop_map(TypeRef::Named),
            arb_name().prop_map(TypeRef::Object),
            arb_name().prop_map(TypeRef::Callback),
        ];
        leaf.prop_recursive(6, 32, 2, |inner| {
            prop_oneof![
                inner.clone().prop_map(|t| TypeRef::Option(Box::new(t))),
                inner.clone().prop_map(|t| TypeRef::Vec(Box::new(t))),
                inner.clone().prop_map(|t| TypeRef::Lazy(Box::new(t))),
                inner.clone().prop_map(|t| TypeRef::Stream(Box::new(t))),
                (inner.clone(), inner.clone())
                    .prop_map(|(a, b)| TypeRef::Map(Box::new(a), Box::new(b))),
                (inner.clone(), inner).prop_map(|(a, b)| TypeRef::Result(Box::new(a), Box::new(b))),
            ]
        })
        .boxed()
    }

    fn arb_field() -> BoxedStrategy<ClosureField> {
        (arb_name(), arb_ty(), any::<bool>())
            .prop_map(|(name, ty, default)| ClosureField { name, ty, default })
            .boxed()
    }

    fn arb_signal() -> BoxedStrategy<ClosureSignal> {
        (
            arb_name(),
            prop_oneof![Just(0_u32), Just(u32::MAX), any::<u32>()],
            arb_ty(),
            any::<bool>(),
        )
            .prop_map(|(name, signal_id, ty, default)| ClosureSignal {
                name,
                signal_id,
                ty,
                default,
            })
            .boxed()
    }

    fn arb_records_and_enums() -> BoxedStrategy<(Vec<ClosureRecord>, Vec<ClosureEnum>)> {
        let record = (arb_name(), proptest::collection::vec(arb_field(), 0..3)).prop_map(
            |(name, fields)| ClosureRecord {
                name,
                fields,
                transparent: false,
            },
        );
        let variant = (
            arb_name(),
            prop_oneof![Just(0_u16), Just(u16::MAX), any::<u16>()],
            proptest::collection::vec(arb_field(), 0..3),
            any::<bool>(),
        )
            .prop_map(|(name, index, fields, tuple)| ClosureVariant {
                name,
                index,
                fields,
                tuple,
            });
        let en = (arb_name(), proptest::collection::vec(variant, 0..3))
            .prop_map(|(name, variants)| ClosureEnum { name, variants });
        (
            proptest::collection::vec(record, 0..3),
            proptest::collection::vec(en, 0..3),
        )
            .boxed()
    }

    fn arb_type_closure() -> BoxedStrategy<TypeClosure> {
        let root = prop_oneof![
            arb_ty().prop_map(|ty| ClosureRoot::Type { ty }),
            proptest::collection::vec(arb_field(), 0..4)
                .prop_map(|params| ClosureRoot::Params { params }),
            proptest::collection::vec(arb_signal(), 0..4)
                .prop_map(|signals| ClosureRoot::Signals { signals }),
        ];
        (root, arb_records_and_enums())
            .prop_map(|(root, (records, enums))| TypeClosure {
                root,
                records,
                enums,
            })
            .boxed()
    }

    fn arb_stores_closure() -> BoxedStrategy<StoresClosure> {
        let store = (
            prop_oneof![Just(0_u32), Just(u32::MAX), any::<u32>()],
            arb_name(),
            proptest::collection::vec(arb_signal(), 0..3),
        )
            .prop_map(|(type_id, name, signals)| DescribedStore {
                type_id,
                name,
                signals,
            });
        (
            proptest::collection::vec(store, 0..3),
            arb_records_and_enums(),
        )
            .prop_map(|(stores, (records, enums))| StoresClosure {
                stores,
                records,
                enums,
            })
            .boxed()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        /// The writer is `serde_json::to_string` on any closure, the reader reads both back to the
        /// same value, and `serde_json` reads the writer's text as the same value.
        #[test]
        fn the_writer_and_reader_agree_with_serde_json(c in arb_type_closure(), s in arb_stores_closure()) {
            let ours = write_type_closure(&c);
            prop_assert_eq!(&ours, &serde_json::to_string(&c).unwrap());
            prop_assert_eq!(read_type_closure(&ours).unwrap(), c.clone());
            prop_assert_eq!(serde_json::from_str::<TypeClosure>(&ours).unwrap(), c);
            let ours = write_stores_closure(&s);
            prop_assert_eq!(&ours, &serde_json::to_string(&s).unwrap());
            prop_assert_eq!(read_stores_closure(&ours).unwrap(), s.clone());
            prop_assert_eq!(serde_json::from_str::<StoresClosure>(&ours).unwrap(), s);
        }

        /// Damaged canonical text (a byte replaced, removed or inserted at any place): the reader
        /// never panics, and what it accepts is what `serde_json` reads too (it never accepts a
        /// spelling that means something else).
        #[test]
        fn damaged_text_never_panics_the_reader(
            c in arb_type_closure(),
            at in any::<prop::sample::Index>(),
            byte in prop_oneof![Just(b'"'), Just(b'\\'), Just(b'{'), Just(b'}'), Just(b','), Just(b'0'), Just(b'9'), Just(b'u'), any::<u8>()],
            how in 0_u8..3,
        ) {
            let mut text = write_type_closure(&c).into_bytes();
            let i = at.index(text.len() + 1);
            match how {
                0 if i < text.len() => text[i] = byte,
                1 if i < text.len() => { text.remove(i); }
                _ => text.insert(i, byte),
            }
            if let Ok(text) = String::from_utf8(text) {
                if let Ok(ours) = read_type_closure(&text) {
                    prop_assert_eq!(Some(ours), serde_json::from_str::<TypeClosure>(&text).ok());
                }
                let _ = read_stores_closure(&text);
            }
        }
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
