//! Persisted values across app updates (ADR-037): dynamic values, structural migration and the
//! migration hooks apps register with `#[undra::migrate]`.
//!
//! Data an older build wrote (a snapshot, a cached query result, a queued mutation's input) is
//! stored with the [`TypeClosure`] it was written under. When the current build's closure has a
//! different [fingerprint](TypeClosure::fingerprint), the old bytes cannot be decoded with the
//! current Rust types. They are decoded instead into a [`DynValue`], a value tree that names its
//! record fields and enum variants, by walking the **old** closure ([`decode_dyn`]), and written
//! back against the **current** type:
//!
//! * [`migrate`] converts by structure, from the old type to the new one, and only when every step
//!   is lossless (decision 4 of the ADR): record fields and enum variants matched **by name**
//!   (order and indices are irrelevant), a field the old value lacks filled when it is an `Option`
//!   (`None`) or `#[undra(default)]` (its zero value), a field the new type lacks dropped, integers
//!   widened without loss, `f32` to `f64`, `T` to `Option<T>`, `Bytes` and `Vec<u8>` either way, and
//!   `Vec`, `Option` and `Map` recursively. A record or enum that does not convert is offered to the
//!   `ty = ".."` hook of its current type, at any depth.
//! * [`encode_dyn`] writes a value an app built (a mutation hook's [`DynRecord`]) against a type
//!   with the same rules, judged by the value: an integer must fit, a float must survive the trip.
//!
//! Anything else (a narrowing, a type change, a renamed field, variant or signal, a variant the new
//! type does not have) is a [`MigrateError`], and the caller turns to the app's hooks
//! ([`Migration`]) and then to its refusal: a restore fails with `RestoreError::Incompatible`, a
//! cache entry is dropped, a queued mutation is dead-lettered (never silently discarded).
//!
//! Decoding is bounded like [`Reader`]: nesting stops at [`MAX_DEPTH`], and every count is checked
//! against the bytes that remain before anything is allocated.

use core::fmt;
use std::collections::BTreeMap;

use undra_meta::{
    ClosureEnum, ClosureField, ClosureRecord, ClosureRoot, TypeClosure, TypeRef, TypeRefMeta,
};
use undra_wire::{MAX_DEPTH, Reader, WireError, Writer};

use crate::guard;

// ---------------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------------

/// A value decoded without its Rust type: what a migration hook receives (ADR-037).
///
/// Integers of every width are an [`Int`](DynValue::Int); `f64` is a [`Float`](DynValue::Float)
/// and `f32` a [`Float32`](DynValue::Float32), which keeps an `f32` bit-exact across a decode and
/// an encode (a NaN's payload included). `Bytes` and `Vec<u8>` decode to what their type says
/// ([`Bytes`](DynValue::Bytes) or a [`List`](DynValue::List) of `Int`s) and either encodes as the
/// other.
#[derive(Clone, Debug, PartialEq)]
pub enum DynValue {
    /// `bool`.
    Bool(bool),
    /// Any integer, `i8` to `u64`.
    Int(i128),
    /// `f64`.
    Float(f64),
    /// `f32`.
    Float32(f32),
    /// `String`.
    String(String),
    /// `Bytes`.
    Bytes(Vec<u8>),
    /// `Duration`, in nanoseconds (never negative).
    Duration(i64),
    /// `Timestamp`, in milliseconds since the Unix epoch.
    Timestamp(i64),
    /// `Uuid`, its 16 bytes.
    Uuid([u8; 16]),
    /// `Option::None`.
    None,
    /// `Option::Some`.
    Some(Box<DynValue>),
    /// `Vec<T>`.
    List(Vec<DynValue>),
    /// `Map<K, V>`, entries in their encoded order.
    Map(Vec<(DynValue, DynValue)>),
    /// A record: its type name and its fields by name, in wire order.
    Record(DynRecord),
    /// An enum value: the enum's name, the variant's name and its fields by name (a tuple
    /// variant's fields are `"0"`, `"1"`, ..; a unit variant has none).
    Enum {
        /// The enum's type name.
        name: String,
        /// The variant's name.
        variant: String,
        /// The variant's payload.
        fields: DynRecord,
    },
}

impl DynValue {
    /// The field `name` of a record or an enum variant.
    ///
    /// ```
    /// use undra_runtime::persist::{DynRecord, DynValue};
    ///
    /// let todo = DynValue::Record(DynRecord::new("Todo").with("title", DynValue::String("milk".into())));
    /// assert_eq!(todo.field("title").and_then(DynValue::as_str), Some("milk"));
    /// assert_eq!(todo.field("missing"), None);
    /// ```
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&DynValue> {
        match self {
            DynValue::Record(record) | DynValue::Enum { fields: record, .. } => record.get(name),
            _ => None,
        }
    }

    /// The text of a `String`.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            DynValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// An integer as `i64`, if it fits.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            DynValue::Int(i) => i64::try_from(*i).ok(),
            _ => None,
        }
    }

    /// A float of either width as `f64`.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            DynValue::Float(f) => Some(*f),
            DynValue::Float32(f) => Some(f64::from(*f)),
            _ => None,
        }
    }

    /// A `bool`.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            DynValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The variant name of an enum value.
    #[must_use]
    pub fn variant(&self) -> Option<&str> {
        match self {
            DynValue::Enum { variant, .. } => Some(variant),
            _ => None,
        }
    }

    /// A short name of the value's kind, for messages.
    fn kind(&self) -> &'static str {
        match self {
            DynValue::Bool(_) => "a bool",
            DynValue::Int(_) => "an integer",
            DynValue::Float(_) | DynValue::Float32(_) => "a float",
            DynValue::String(_) => "a string",
            DynValue::Bytes(_) => "bytes",
            DynValue::Duration(_) => "a duration",
            DynValue::Timestamp(_) => "a timestamp",
            DynValue::Uuid(_) => "a uuid",
            DynValue::None | DynValue::Some(_) => "an option",
            DynValue::List(_) => "a list",
            DynValue::Map(_) => "a map",
            DynValue::Record(_) => "a record",
            DynValue::Enum { .. } => "an enum",
        }
    }
}

/// Fields by name, in order: a record's, an enum variant's, or a mutation's parameters.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DynRecord {
    /// The type name (the record's, the enum's, or the mutation's for its parameters).
    pub name: String,
    /// `(field name, value)`, in wire order.
    pub fields: Vec<(String, DynValue)>,
}

impl DynRecord {
    /// An empty record called `name`.
    pub fn new(name: impl Into<String>) -> DynRecord {
        DynRecord {
            name: name.into(),
            fields: Vec::new(),
        }
    }

    /// `self` with `name` set to `value` (builder form of [`set`](Self::set)).
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: DynValue) -> DynRecord {
        self.set(name, value);
        self
    }

    /// The field `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DynValue> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// Sets the field `name`, replacing it in place or appending it.
    pub fn set(&mut self, name: impl Into<String>, value: DynValue) {
        let name = name.into();
        match self.fields.iter_mut().find(|(n, _)| *n == name) {
            Some((_, slot)) => *slot = value,
            None => self.fields.push((name, value)),
        }
    }

    /// Removes the field `name` and returns its value.
    pub fn remove(&mut self, name: &str) -> Option<DynValue> {
        let at = self.fields.iter().position(|(n, _)| n == name)?;
        Some(self.fields.remove(at).1)
    }

    /// Renames the field `from` to `to` (the usual hook for a renamed field). Does nothing if
    /// there is no field `from`.
    pub fn rename(&mut self, from: &str, to: impl Into<String>) {
        if let Some((name, _)) = self.fields.iter_mut().find(|(n, _)| n == from) {
            *name = to.into();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Why a persisted value could not be migrated: what a migration hook returns to refuse a value,
/// and what a failed structural conversion reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrateError {
    path: String,
    message: String,
}

impl MigrateError {
    /// A refusal with `message` (what a hook returns for a value it cannot convert).
    ///
    /// ```
    /// use undra_runtime::persist::MigrateError;
    ///
    /// let e = MigrateError::new("a negative age cannot be a float age");
    /// assert_eq!(e.to_string(), "a negative age cannot be a float age");
    /// ```
    pub fn new(message: impl Into<String>) -> MigrateError {
        MigrateError {
            path: String::new(),
            message: message.into(),
        }
    }

    /// What went wrong.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Where in the value, as `field.field[index]`; empty at the root.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Prefixes the path with `segment` (a field name, or `[i]`).
    #[must_use]
    pub(crate) fn within(mut self, segment: &str) -> MigrateError {
        if self.path.is_empty() {
            self.path = segment.to_owned();
        } else if self.path.starts_with('[') {
            self.path = format!("{segment}{}", self.path);
        } else {
            self.path = format!("{segment}.{}", self.path);
        }
        self
    }

    fn wire(e: WireError) -> MigrateError {
        MigrateError::new(format!("the stored bytes do not decode: {e}"))
    }
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for MigrateError {}

impl From<WireError> for MigrateError {
    fn from(e: WireError) -> MigrateError {
        MigrateError::wire(e)
    }
}

// ---------------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------------

/// Decodes `bytes` (exactly: trailing bytes are an error) as one value of `ty`, resolving named
/// types in `closure`.
///
/// # Errors
///
/// A [`MigrateError`] for bytes that do not decode as `ty`, a named type the closure does not
/// describe, nesting deeper than [`MAX_DEPTH`], and types that are never persisted values
/// (`Unit`, `Lazy`, `Result`, `Stream`).
///
/// ```
/// use undra_meta::{Schema, TypeRef};
/// use undra_runtime::persist::{DynValue, decode_dyn};
///
/// let closure = Schema::new("demo").closure(&TypeRef::vec(TypeRef::I16));
/// let value = decode_dyn(&[2, 0, 0, 0, 7, 0, 0xff, 0xff], &TypeRef::vec(TypeRef::I16), &closure).unwrap();
/// assert_eq!(value, DynValue::List(vec![DynValue::Int(7), DynValue::Int(-1)]));
/// ```
pub fn decode_dyn(
    bytes: &[u8],
    ty: &TypeRef,
    closure: &TypeClosure,
) -> Result<DynValue, MigrateError> {
    let mut r = Reader::new(bytes);
    let value = decode_from(&mut r, ty, closure, 0)?;
    r.finish()?;
    Ok(value)
}

/// Decodes a mutation's encoded parameters (`closure`'s root must be [`ClosureRoot::Params`])
/// into a record named `name` whose fields are the parameters.
///
/// # Errors
///
/// As [`decode_dyn`], and when the closure does not describe parameters.
pub fn decode_params(
    bytes: &[u8],
    name: &str,
    closure: &TypeClosure,
) -> Result<DynRecord, MigrateError> {
    let ClosureRoot::Params { params } = &closure.root else {
        return Err(MigrateError::new(
            "the closure does not describe parameters",
        ));
    };
    let mut r = Reader::new(bytes);
    let mut record = DynRecord::new(name);
    for param in params {
        let value =
            decode_from(&mut r, &param.ty, closure, 1).map_err(|e| e.within(&param.name))?;
        record.fields.push((param.name.clone(), value));
    }
    r.finish()?;
    Ok(record)
}

/// The smallest number of bytes one value of `ty` takes (for count checks); `1` for named types,
/// whose size the closure would have to be consulted for.
fn min_len(ty: &TypeRef) -> usize {
    match ty {
        TypeRef::Bool | TypeRef::I8 | TypeRef::U8 | TypeRef::Option(_) | TypeRef::Named(_) => 1,
        TypeRef::I16 | TypeRef::U16 => 2,
        TypeRef::I32
        | TypeRef::U32
        | TypeRef::F32
        | TypeRef::String
        | TypeRef::Bytes
        | TypeRef::Vec(_)
        | TypeRef::Map(..) => 4,
        TypeRef::I64
        | TypeRef::U64
        | TypeRef::F64
        | TypeRef::Duration
        | TypeRef::Timestamp
        | TypeRef::Lazy(_) => 8,
        TypeRef::Uuid => 16,
        TypeRef::Unit | TypeRef::Result(..) | TypeRef::Stream(_) => 1,
    }
}

fn decode_from(
    r: &mut Reader<'_>,
    ty: &TypeRef,
    closure: &TypeClosure,
    depth: u32,
) -> Result<DynValue, MigrateError> {
    if depth > MAX_DEPTH {
        return Err(WireError::NestingTooDeep { at: r.position() }.into());
    }
    Ok(match ty {
        TypeRef::Bool => DynValue::Bool(r.read_bool()?),
        TypeRef::I8 => DynValue::Int(i128::from(r.read_i8()?)),
        TypeRef::I16 => DynValue::Int(i128::from(r.read_i16()?)),
        TypeRef::I32 => DynValue::Int(i128::from(r.read_i32()?)),
        TypeRef::I64 => DynValue::Int(i128::from(r.read_i64()?)),
        TypeRef::U8 => DynValue::Int(i128::from(r.read_u8()?)),
        TypeRef::U16 => DynValue::Int(i128::from(r.read_u16()?)),
        TypeRef::U32 => DynValue::Int(i128::from(r.read_u32()?)),
        TypeRef::U64 => DynValue::Int(i128::from(r.read_u64()?)),
        TypeRef::F32 => DynValue::Float32(r.read_f32()?),
        TypeRef::F64 => DynValue::Float(r.read_f64()?),
        TypeRef::String => DynValue::String(r.read_str()?.to_owned()),
        TypeRef::Bytes => DynValue::Bytes(r.read_bytes()?.to_vec()),
        TypeRef::Duration => {
            let at = r.position();
            let nanos = r.read_i64()?;
            if nanos < 0 {
                return Err(WireError::NegativeDuration { at }.into());
            }
            DynValue::Duration(nanos)
        }
        TypeRef::Timestamp => DynValue::Timestamp(r.read_i64()?),
        TypeRef::Uuid => DynValue::Uuid(r.read_array::<16>()?),
        TypeRef::Option(inner) => {
            let at = r.position();
            match r.read_u8()? {
                0 => DynValue::None,
                1 => DynValue::Some(Box::new(decode_from(r, inner, closure, depth + 1)?)),
                tag => {
                    return Err(WireError::InvalidTag {
                        tag: u32::from(tag),
                        at,
                        ty: "Option",
                    }
                    .into());
                }
            }
        }
        TypeRef::Vec(item) => {
            let count = r.read_count(min_len(item))?;
            let mut items = Vec::with_capacity(count.min(1024));
            for i in 0..count {
                items.push(
                    decode_from(r, item, closure, depth + 1)
                        .map_err(|e| e.within(&format!("[{i}]")))?,
                );
            }
            DynValue::List(items)
        }
        TypeRef::Map(key, value) => {
            let count = r.read_count(min_len(key) + min_len(value))?;
            let mut entries = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                let k = decode_from(r, key, closure, depth + 1)?;
                let v = decode_from(r, value, closure, depth + 1)?;
                entries.push((k, v));
            }
            DynValue::Map(entries)
        }
        TypeRef::Named(name) => {
            if let Some(record) = closure.record(name) {
                DynValue::Record(decode_fields(
                    r,
                    &record.name,
                    &record.fields,
                    closure,
                    depth,
                )?)
            } else if let Some(en) = closure.enum_def(name) {
                let at = r.position();
                let index = r.read_u16()?;
                let Some(variant) = en.variants.iter().find(|v| v.index == index) else {
                    return Err(WireError::InvalidTag {
                        tag: u32::from(index),
                        at,
                        ty: "enum variant",
                    }
                    .into());
                };
                let fields = decode_fields(r, &en.name, &variant.fields, closure, depth)?;
                DynValue::Enum {
                    name: en.name.clone(),
                    variant: variant.name.clone(),
                    fields,
                }
            } else {
                return Err(MigrateError::new(format!(
                    "the stored description has no record or enum `{name}`"
                )));
            }
        }
        TypeRef::Unit | TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => {
            return Err(MigrateError::new(format!(
                "{ty} is not a persisted value type"
            )));
        }
    })
}

fn decode_fields(
    r: &mut Reader<'_>,
    name: &str,
    fields: &[ClosureField],
    closure: &TypeClosure,
    depth: u32,
) -> Result<DynRecord, MigrateError> {
    let mut record = DynRecord::new(name);
    for field in fields {
        let value =
            decode_from(r, &field.ty, closure, depth + 1).map_err(|e| e.within(&field.name))?;
        record.fields.push((field.name.clone(), value));
    }
    Ok(record)
}

// ---------------------------------------------------------------------------------------------
// Encoding (value-directed)
// ---------------------------------------------------------------------------------------------

/// Encodes `value` as one value of `ty` (named types resolved in `closure`), converting with the
/// structural rules judged by the value itself: record fields and variants by name, a missing
/// field filled when it is an `Option` or `#[undra(default)]` and has a zero value, extra fields
/// dropped, an integer that fits the target width, a float that survives the trip, `T` as `Some`
/// of an `Option<T>`, `Bytes` and a list of bytes either way. This is how a value an app built
/// (a mutation hook's [`DynRecord`]) becomes bytes; [`migrate`] is the type-directed conversion
/// of old stored data.
///
/// # Errors
///
/// A [`MigrateError`] naming the place that does not convert.
///
/// ```
/// use undra_meta::{Schema, TypeRef};
/// use undra_runtime::persist::{DynValue, encode_dyn};
///
/// let ty = TypeRef::option(TypeRef::I64);
/// let closure = Schema::new("demo").closure(&ty);
/// // An i64 7, as `Some`.
/// assert_eq!(encode_dyn(&DynValue::Int(7), &ty, &closure).unwrap(), [1, 7, 0, 0, 0, 0, 0, 0, 0]);
/// assert!(encode_dyn(&DynValue::String("7".into()), &ty, &closure).is_err());
/// ```
pub fn encode_dyn(
    value: &DynValue,
    ty: &TypeRef,
    closure: &TypeClosure,
) -> Result<Vec<u8>, MigrateError> {
    let mut w = Writer::new();
    put_value(&mut w, value, ty, closure, 0)?;
    Ok(w.into_vec())
}

/// Encodes a parameter record against the parameters of `closure` (its root must be
/// [`ClosureRoot::Params`]): each parameter by name with [`encode_dyn`]'s rules; a missing
/// `Option` parameter is `None`, a missing other one is an error, an extra one is dropped.
///
/// # Errors
///
/// As [`encode_dyn`].
pub fn encode_params(record: &DynRecord, closure: &TypeClosure) -> Result<Vec<u8>, MigrateError> {
    let ClosureRoot::Params { params } = &closure.root else {
        return Err(MigrateError::new(
            "the closure does not describe parameters",
        ));
    };
    let mut w = Writer::new();
    put_fields(&mut w, record, params, closure, 0)?;
    Ok(w.into_vec())
}

fn not_structural(message: impl Into<String>) -> MigrateError {
    MigrateError::new(message)
}

fn put_int(w: &mut Writer, i: i128, ty: &TypeRef) -> Result<(), MigrateError> {
    let out_of_range = || not_structural(format!("{i} does not fit {ty}"));
    match ty {
        TypeRef::I8 => w.write_i8(i8::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::I16 => w.write_i16(i16::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::I32 => w.write_i32(i32::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::I64 => w.write_i64(i64::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::U8 => w.write_u8(u8::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::U16 => w.write_u16(u16::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::U32 => w.write_u32(u32::try_from(i).map_err(|_| out_of_range())?),
        TypeRef::U64 => w.write_u64(u64::try_from(i).map_err(|_| out_of_range())?),
        _ => return Err(not_structural(format!("an integer cannot become {ty}"))),
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // one arm per wire type
fn put_value(
    w: &mut Writer,
    value: &DynValue,
    ty: &TypeRef,
    closure: &TypeClosure,
    depth: u32,
) -> Result<(), MigrateError> {
    if depth > MAX_DEPTH {
        return Err(not_structural("the value nests too deeply"));
    }
    let mismatch = || not_structural(format!("{} cannot become {ty}", value.kind()));
    match (ty, value) {
        (TypeRef::Bool, DynValue::Bool(b)) => w.write_bool(*b),
        (
            TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::I64
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::U64,
            DynValue::Int(i),
        ) => put_int(w, *i, ty)?,
        (TypeRef::F64, DynValue::Float(f)) => w.write_f64(*f),
        (TypeRef::F64, DynValue::Float32(f)) => w.write_f64(f64::from(*f)),
        (TypeRef::F32, DynValue::Float32(f)) => w.write_f32(*f),
        (TypeRef::F32, DynValue::Float(f)) => {
            #[allow(clippy::cast_possible_truncation)] // checked to be exact just below
            let narrow = *f as f32;
            if f64::from(narrow).to_bits() != f.to_bits() && !(f.is_nan() && narrow.is_nan()) {
                return Err(not_structural(format!("{f} does not survive as an f32")));
            }
            w.write_f32(narrow);
        }
        (TypeRef::String, DynValue::String(s)) => w.write_str(s),
        (TypeRef::Bytes, DynValue::Bytes(b)) => w.write_bytes(b),
        (TypeRef::Bytes, DynValue::List(items)) => {
            let bytes = list_bytes(items).ok_or_else(mismatch)?;
            w.write_bytes(&bytes);
        }
        (TypeRef::Vec(item), DynValue::Bytes(b)) if **item == TypeRef::U8 => w.write_bytes(b),
        (TypeRef::Duration, DynValue::Duration(n)) if *n >= 0 => w.write_i64(*n),
        (TypeRef::Timestamp, DynValue::Timestamp(t)) => w.write_i64(*t),
        (TypeRef::Uuid, DynValue::Uuid(u)) => w.write_raw(u),
        (TypeRef::Option(_), DynValue::None) => w.write_u8(0),
        (TypeRef::Option(inner), DynValue::Some(v)) => {
            w.write_u8(1);
            put_value(w, v, inner, closure, depth + 1)?;
        }
        (TypeRef::Option(inner), other) => {
            // `T` becomes `Some(T)`.
            w.write_u8(1);
            put_value(w, other, inner, closure, depth + 1)?;
        }
        (TypeRef::Vec(item), DynValue::List(items)) => {
            w.write_len(len_u32(items.len())?);
            for (i, v) in items.iter().enumerate() {
                put_value(w, v, item, closure, depth + 1)
                    .map_err(|e| e.within(&format!("[{i}]")))?;
            }
        }
        (TypeRef::Map(key, val), DynValue::Map(entries)) => {
            // Keys in the order of their encoded bytes (SPEC 3.1), each once.
            let mut sorted: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
            for (k, v) in entries {
                let mut kw = Writer::new();
                put_value(&mut kw, k, key, closure, depth + 1)?;
                let mut vw = Writer::new();
                put_value(&mut vw, v, val, closure, depth + 1)?;
                if sorted.insert(kw.into_vec(), vw.into_vec()).is_some() {
                    return Err(not_structural("two map keys became the same key"));
                }
            }
            w.write_len(len_u32(sorted.len())?);
            for (k, v) in sorted {
                w.write_raw(&k);
                w.write_raw(&v);
            }
        }
        (TypeRef::Named(name), _) => {
            if let Some(record) = closure.record(name) {
                let DynValue::Record(fields) = value else {
                    return Err(mismatch());
                };
                put_fields(w, fields, &record.fields, closure, depth)?;
            } else if let Some(en) = closure.enum_def(name) {
                let DynValue::Enum {
                    variant, fields, ..
                } = value
                else {
                    return Err(mismatch());
                };
                let Some(target) = en.variants.iter().find(|v| v.name == *variant) else {
                    return Err(not_structural(format!(
                        "`{}` has no variant `{variant}`",
                        en.name
                    )));
                };
                w.write_u16(target.index);
                put_fields(w, fields, &target.fields, closure, depth)
                    .map_err(|e| e.within(variant))?;
            } else {
                return Err(not_structural(format!(
                    "the current schema has no record or enum `{name}`"
                )));
            }
        }
        _ => return Err(mismatch()),
    }
    Ok(())
}

fn list_bytes(items: &[DynValue]) -> Option<Vec<u8>> {
    items
        .iter()
        .map(|i| match i {
            DynValue::Int(b) => u8::try_from(*b).ok(),
            _ => None,
        })
        .collect()
}

fn len_u32(len: usize) -> Result<u32, MigrateError> {
    u32::try_from(len).map_err(|_| not_structural("a collection too large to encode"))
}

fn put_fields(
    w: &mut Writer,
    record: &DynRecord,
    fields: &[ClosureField],
    closure: &TypeClosure,
    depth: u32,
) -> Result<(), MigrateError> {
    for field in fields {
        match record.get(&field.name) {
            Some(value) => put_value(w, value, &field.ty, closure, depth + 1)
                .map_err(|e| e.within(&field.name))?,
            None => put_missing(w, field).map_err(|e| e.within(&field.name))?,
        }
    }
    Ok(())
}

/// A field the value lacks: `None` for an `Option`, the zero value for an `#[undra(default)]`
/// field whose type has one, else not structural.
fn put_missing(w: &mut Writer, field: &ClosureField) -> Result<(), MigrateError> {
    if matches!(field.ty, TypeRef::Option(_)) {
        w.write_u8(0);
        return Ok(());
    }
    if field.default && put_zero(w, &field.ty) {
        return Ok(());
    }
    Err(not_structural(
        "the stored value has no such field and the field has no default (an `Option`, or `#[undra(default)]` with a zero value)",
    ))
}

/// Writes the zero value of `ty` (the value generated constructors default a field to: `false`,
/// `0`, `""`, empty, `None`, the nil UUID, the epoch). `false` (and nothing written) for a named
/// type, which has no zero value the schema knows.
fn put_zero(w: &mut Writer, ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Bool | TypeRef::I8 | TypeRef::U8 | TypeRef::Option(_) => w.write_u8(0),
        TypeRef::I16 | TypeRef::U16 => w.write_u16(0),
        TypeRef::I32
        | TypeRef::U32
        | TypeRef::String
        | TypeRef::Bytes
        | TypeRef::Vec(_)
        | TypeRef::Map(..) => w.write_u32(0),
        TypeRef::F32 => w.write_f32(0.0),
        TypeRef::F64 => w.write_f64(0.0),
        TypeRef::I64 | TypeRef::U64 | TypeRef::Duration | TypeRef::Timestamp => w.write_u64(0),
        TypeRef::Uuid => w.write_raw(&[0; 16]),
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Structural migration (type-directed) with hooks
// ---------------------------------------------------------------------------------------------

/// Whether an integer of type `from` converts to `to` without loss for every value (decision 4:
/// `i8 -> i16 -> i32 -> i64`, `u8 -> u16 -> u32 -> u64`, `uN -> i2N` and beyond).
fn widens(from: &TypeRef, to: &TypeRef) -> bool {
    fn width(t: &TypeRef) -> Option<(bool, u32)> {
        Some(match t {
            TypeRef::I8 => (true, 8),
            TypeRef::I16 => (true, 16),
            TypeRef::I32 => (true, 32),
            TypeRef::I64 => (true, 64),
            TypeRef::U8 => (false, 8),
            TypeRef::U16 => (false, 16),
            TypeRef::U32 => (false, 32),
            TypeRef::U64 => (false, 64),
            _ => return None,
        })
    }
    match (width(from), width(to)) {
        (Some((fs, fw)), Some((ts, tw))) => match (fs, ts) {
            (true, true) | (false, false) => tw >= fw,
            (false, true) => tw > fw,
            (true, false) => false,
        },
        _ => false,
    }
}

/// Where the hooks of a migration come from. The runtime answers from the `#[undra::migrate]`
/// registrations ([`Migration`]); tests pass their own.
pub trait HookSource {
    /// The `ty = "<name>"` hook for an old value of that (current) type whose old closure has
    /// fingerprint `from`, if any.
    fn type_hook(&self, name: &str, from: u64) -> Option<&Migration>;
}

/// No hooks: structural migration only.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl HookSource for NoHooks {
    fn type_hook(&self, _name: &str, _from: u64) -> Option<&Migration> {
        None
    }
}

/// The registered `#[undra::migrate]` hooks.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegisteredHooks;

impl HookSource for RegisteredHooks {
    fn type_hook(&self, name: &str, from: u64) -> Option<&Migration> {
        find_hook(
            |m| matches!(m.target, MigrationTarget::Type(t) if t == name),
            from,
        )
    }
}

/// The best hook registered for `target`: one whose `from` is exactly `fingerprint` first, else
/// one without `from`.
pub(crate) fn find_hook(
    target: impl Fn(&Migration) -> bool,
    fingerprint: u64,
) -> Option<&'static Migration> {
    let mut fallback = None;
    for hook in inventory::iter::<Migration> {
        if !target(hook) {
            continue;
        }
        match hook.from {
            Some(from) if from == fingerprint => return Some(hook),
            Some(_) => {}
            None => fallback = fallback.or(Some(hook)),
        }
    }
    fallback
}

/// Converts `old` (bytes of `old_ty`, named types in `old_closure`) into bytes of `new_ty` (named
/// types in `new_closure`), by structure (module docs), offering every record or enum that does
/// not convert to its current type's `ty = ".."` hook from `hooks`.
///
/// # Errors
///
/// A [`MigrateError`] naming the first place that neither converts nor has a hook that accepts
/// it, or the stored bytes do not decode.
pub fn migrate(
    old: &[u8],
    old_ty: &TypeRef,
    old_closure: &TypeClosure,
    new_ty: &TypeRef,
    new_closure: &TypeClosure,
    hooks: &dyn HookSource,
) -> Result<Vec<u8>, MigrateError> {
    let value = decode_dyn(old, old_ty, old_closure)?;
    migrate_value(&value, old_ty, old_closure, new_ty, new_closure, hooks)
}

/// [`migrate`] for a value already decoded with [`decode_dyn`].
///
/// # Errors
///
/// As [`migrate`].
pub fn migrate_value(
    value: &DynValue,
    old_ty: &TypeRef,
    old_closure: &TypeClosure,
    new_ty: &TypeRef,
    new_closure: &TypeClosure,
    hooks: &dyn HookSource,
) -> Result<Vec<u8>, MigrateError> {
    let mut w = Writer::new();
    Converter {
        old: old_closure,
        new: new_closure,
        hooks,
    }
    .convert(&mut w, value, old_ty, new_ty, 0, false)?;
    Ok(w.into_vec())
}

/// Converts a mutation's decoded parameters (`old_closure` describes them) to the current
/// parameters (`new_closure`): each by name, structurally; a parameter the old input lacks is
/// `None` if it is an `Option`, else the input does not convert; one the current mutation lacks
/// is dropped.
///
/// # Errors
///
/// As [`migrate`].
pub fn migrate_params(
    old: &DynRecord,
    old_closure: &TypeClosure,
    new_closure: &TypeClosure,
    hooks: &dyn HookSource,
) -> Result<Vec<u8>, MigrateError> {
    let (ClosureRoot::Params { params: old_params }, ClosureRoot::Params { params: new_params }) =
        (&old_closure.root, &new_closure.root)
    else {
        return Err(MigrateError::new("the closures do not describe parameters"));
    };
    let converter = Converter {
        old: old_closure,
        new: new_closure,
        hooks,
    };
    let mut w = Writer::new();
    converter.convert_fields(&mut w, old, old_params, new_params, 0)?;
    Ok(w.into_vec())
}

struct Converter<'a> {
    old: &'a TypeClosure,
    new: &'a TypeClosure,
    hooks: &'a dyn HookSource,
}

impl Converter<'_> {
    #[allow(clippy::too_many_lines)] // one arm per conversion rule
    fn convert(
        &self,
        w: &mut Writer,
        value: &DynValue,
        old_ty: &TypeRef,
        new_ty: &TypeRef,
        depth: u32,
        hook_here: bool,
    ) -> Result<(), MigrateError> {
        if depth > MAX_DEPTH {
            return Err(not_structural("the value nests too deeply"));
        }
        let refuse = || not_structural(format!("{old_ty} cannot become {new_ty}"));
        match (old_ty, new_ty) {
            (TypeRef::Option(old_inner), TypeRef::Option(new_inner)) => match value {
                DynValue::None => w.write_u8(0),
                DynValue::Some(inner) => {
                    w.write_u8(1);
                    self.convert(w, inner, old_inner, new_inner, depth + 1, true)?;
                }
                _ => return Err(refuse()),
            },
            (_, TypeRef::Option(new_inner)) if !matches!(old_ty, TypeRef::Option(_)) => {
                w.write_u8(1);
                self.convert(w, value, old_ty, new_inner, depth + 1, hook_here)?;
            }
            (TypeRef::Vec(old_item), TypeRef::Vec(new_item)) => {
                let DynValue::List(items) = value else {
                    return Err(refuse());
                };
                w.write_len(len_u32(items.len())?);
                for (i, item) in items.iter().enumerate() {
                    self.convert(w, item, old_item, new_item, depth + 1, true)
                        .map_err(|e| e.within(&format!("[{i}]")))?;
                }
            }
            (TypeRef::Vec(old_item), TypeRef::Bytes) if **old_item == TypeRef::U8 => {
                let DynValue::List(items) = value else {
                    return Err(refuse());
                };
                w.write_bytes(&list_bytes(items).ok_or_else(refuse)?);
            }
            (TypeRef::Bytes, TypeRef::Vec(new_item)) if **new_item == TypeRef::U8 => {
                let DynValue::Bytes(bytes) = value else {
                    return Err(refuse());
                };
                w.write_bytes(bytes);
            }
            (TypeRef::Map(old_k, old_v), TypeRef::Map(new_k, new_v)) => {
                let DynValue::Map(entries) = value else {
                    return Err(refuse());
                };
                let mut sorted: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
                for (k, v) in entries {
                    let mut kw = Writer::new();
                    self.convert(&mut kw, k, old_k, new_k, depth + 1, true)?;
                    let mut vw = Writer::new();
                    self.convert(&mut vw, v, old_v, new_v, depth + 1, true)?;
                    if sorted.insert(kw.into_vec(), vw.into_vec()).is_some() {
                        return Err(not_structural("two map keys became the same key"));
                    }
                }
                w.write_len(len_u32(sorted.len())?);
                for (k, v) in sorted {
                    w.write_raw(&k);
                    w.write_raw(&v);
                }
            }
            (TypeRef::F32, TypeRef::F64) => {
                let DynValue::Float32(f) = value else {
                    return Err(refuse());
                };
                w.write_f64(f64::from(*f));
            }
            (TypeRef::Named(old_name), TypeRef::Named(new_name)) => {
                let mut attempt = Writer::new();
                match self.convert_named(&mut attempt, value, old_name, new_name, depth) {
                    Ok(()) => w.write_raw(attempt.as_slice()),
                    Err(error) => {
                        if !hook_here {
                            return Err(error);
                        }
                        match self.type_hook(value, old_name, new_name)? {
                            Some(bytes) => w.write_raw(&bytes),
                            None => return Err(error),
                        }
                    }
                }
            }
            (a, b) if a == b => {
                // The same primitive type: re-encode what was decoded.
                put_value(w, value, b, self.new, depth)?;
            }
            (a, b) if widens(a, b) => {
                let DynValue::Int(i) = value else {
                    return Err(refuse());
                };
                put_int(w, *i, b)?;
            }
            _ => return Err(refuse()),
        }
        Ok(())
    }

    /// The `ty = "<new_name>"` hook's bytes for `value`, if a hook is registered for the old
    /// value's fingerprint (or for any). A hook that refuses or panics is an error.
    fn type_hook(
        &self,
        value: &DynValue,
        old_name: &str,
        new_name: &str,
    ) -> Result<Option<Vec<u8>>, MigrateError> {
        let from = self
            .old
            .narrowed(&TypeRef::named(old_name.to_owned()))
            .fingerprint();
        let Some(hook) = self.hooks.type_hook(new_name, from) else {
            return Ok(None);
        };
        let MigrationHook::Value(run) = hook.hook else {
            return Ok(None);
        };
        run_hook(hook.name, || run(value)).map(Some)
    }

    fn convert_named(
        &self,
        w: &mut Writer,
        value: &DynValue,
        old_name: &str,
        new_name: &str,
        depth: u32,
    ) -> Result<(), MigrateError> {
        match (
            self.old.record(old_name),
            self.new.record(new_name),
            self.old.enum_def(old_name),
            self.new.enum_def(new_name),
        ) {
            (Some(old), Some(new), _, _) => {
                let DynValue::Record(fields) = value else {
                    return Err(not_structural("the stored value is not a record"));
                };
                self.convert_record(w, fields, old, new, depth)
            }
            (_, _, Some(old), Some(new)) => {
                let DynValue::Enum {
                    variant, fields, ..
                } = value
                else {
                    return Err(not_structural("the stored value is not an enum"));
                };
                self.convert_enum(w, variant, fields, old, new, depth)
            }
            (_, None, _, None) => Err(not_structural(format!(
                "the current schema has no record or enum `{new_name}`"
            ))),
            _ => Err(not_structural(format!(
                "`{old_name}` and `{new_name}` are not both records or both enums"
            ))),
        }
    }

    fn convert_record(
        &self,
        w: &mut Writer,
        fields: &DynRecord,
        old: &ClosureRecord,
        new: &ClosureRecord,
        depth: u32,
    ) -> Result<(), MigrateError> {
        self.convert_fields(w, fields, &old.fields, &new.fields, depth)
    }

    fn convert_enum(
        &self,
        w: &mut Writer,
        variant: &str,
        fields: &DynRecord,
        old: &ClosureEnum,
        new: &ClosureEnum,
        depth: u32,
    ) -> Result<(), MigrateError> {
        let Some(old_variant) = old.variants.iter().find(|v| v.name == variant) else {
            return Err(not_structural(format!(
                "the stored `{}` has no variant `{variant}`",
                old.name
            )));
        };
        let Some(new_variant) = new.variants.iter().find(|v| v.name == variant) else {
            return Err(not_structural(format!(
                "`{}` no longer has the variant `{variant}`",
                new.name
            )));
        };
        w.write_u16(new_variant.index);
        self.convert_fields(w, fields, &old_variant.fields, &new_variant.fields, depth)
            .map_err(|e| e.within(variant))
    }

    fn convert_fields(
        &self,
        w: &mut Writer,
        values: &DynRecord,
        old_fields: &[ClosureField],
        new_fields: &[ClosureField],
        depth: u32,
    ) -> Result<(), MigrateError> {
        for field in new_fields {
            match (
                old_fields.iter().find(|f| f.name == field.name),
                values.get(&field.name),
            ) {
                (Some(old_field), Some(value)) => self
                    .convert(w, value, &old_field.ty, &field.ty, depth + 1, true)
                    .map_err(|e| e.within(&field.name))?,
                _ => put_missing(w, field).map_err(|e| e.within(&field.name))?,
            }
        }
        Ok(())
    }
}

/// Runs a hook under the panic guard: a panic is a failed migration naming the hook.
pub(crate) fn run_hook<T>(
    name: &str,
    hook: impl FnOnce() -> Result<T, MigrateError>,
) -> Result<T, MigrateError> {
    match guard::guarded(hook) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(refused)) => Err(MigrateError::new(format!(
            "the migration hook `{name}` refused the value: {refused}"
        ))),
        Err(report) => Err(MigrateError::new(format!(
            "the migration hook `{name}` panicked: {}",
            report.message
        ))),
    }
}

// ---------------------------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------------------------

/// What a migration hook converts (the arguments of `#[undra::migrate(..)]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationTarget {
    /// `ty = "Todo"`: any persisted value of that record or enum whose structure changed, at any
    /// depth of a snapshot signal, a cached query result or a queued mutation's input.
    Type(&'static str),
    /// `store = "Profile", signal = "age"`: one signal of one store in a snapshot, including a
    /// signal the snapshot lacks (the hook then receives `None`).
    Signal {
        /// The store type's name.
        store: &'static str,
        /// The signal's name.
        signal: &'static str,
    },
    /// `mutation = "add_todo"`: the queued input of one mutation, by parameter name.
    Mutation(&'static str),
}

/// The function a [`Migration`] runs. The macro wraps the app's typed function: `Value` and
/// `Signal` hooks return the current type, which the wrapper encodes with its `Encode`.
#[derive(Clone, Copy)]
pub enum MigrationHook {
    /// `fn(old: &DynValue) -> Result<T, MigrateError>`, encoded.
    Value(fn(&DynValue) -> Result<Vec<u8>, MigrateError>),
    /// `fn(old: Option<&DynValue>) -> Result<T, MigrateError>`, encoded.
    Signal(fn(Option<&DynValue>) -> Result<Vec<u8>, MigrateError>),
    /// `fn(old: &DynRecord) -> Result<DynRecord, MigrateError>`; the result is encoded against
    /// the current parameters with [`encode_params`].
    Mutation(fn(&DynRecord) -> Result<DynRecord, MigrateError>),
}

impl fmt::Debug for MigrationHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MigrationHook::Value(_) => "Value(..)",
            MigrationHook::Signal(_) => "Signal(..)",
            MigrationHook::Mutation(_) => "Mutation(..)",
        })
    }
}

/// One `#[undra::migrate]` hook, submitted through `inventory` (ADR-037 decision 6).
///
/// Hooks run on the core, inside a restore or the query client's hydration, under the panic
/// guard: a hook that panics is a failed migration, logged with its name.
#[derive(Debug)]
pub struct Migration {
    /// The hook function's name, for logs.
    pub name: &'static str,
    /// What it converts.
    pub target: MigrationTarget,
    /// `from = "0x.."`: only for old data with this fingerprint.
    pub from: Option<u64>,
    /// The type the hook returns (`None` for a mutation hook), checked against the schema at
    /// start-up.
    pub returns: Option<TypeRefMeta>,
    /// The function.
    pub hook: MigrationHook,
}

inventory::collect!(Migration);

/// Every registered hook.
pub fn migrations() -> impl Iterator<Item = &'static Migration> {
    inventory::iter::<Migration>.into_iter()
}

/// The signal hook for `store`.`signal` and old data with `fingerprint` (the store's), if any.
pub(crate) fn signal_hook(
    store: &str,
    signal: &str,
    fingerprint: u64,
) -> Option<&'static Migration> {
    find_hook(
        |m| matches!(m.target, MigrationTarget::Signal { store: s, signal: g } if s == store && g == signal),
        fingerprint,
    )
}

/// The mutation hook for `mutation` and old input with `fingerprint`, if any.
pub fn mutation_hook(mutation: &str, fingerprint: u64) -> Option<&'static Migration> {
    find_hook(
        |m| matches!(m.target, MigrationTarget::Mutation(name) if name == mutation),
        fingerprint,
    )
}

/// The `ty` hook of a root value whose current type is `ty`, if it is a named type and a hook is
/// registered for it.
pub fn root_type_hook(ty: &TypeRef, fingerprint: u64) -> Option<&'static Migration> {
    let TypeRef::Named(name) = ty else {
        return None;
    };
    find_hook(
        |m| matches!(m.target, MigrationTarget::Type(t) if t == name),
        fingerprint,
    )
}

/// Runs a `ty` or signal hook and returns its bytes.
///
/// # Errors
///
/// The hook's refusal or panic, as a [`MigrateError`] naming it.
pub fn run_value_hook(hook: &Migration, old: Option<&DynValue>) -> Result<Vec<u8>, MigrateError> {
    match (hook.hook, old) {
        (MigrationHook::Value(run), Some(value)) => run_hook(hook.name, || run(value)),
        (MigrationHook::Signal(run), old) => run_hook(hook.name, || run(old)),
        _ => Err(MigrateError::new(format!(
            "the migration hook `{}` does not take this kind of value",
            hook.name
        ))),
    }
}

/// Runs a mutation hook.
///
/// # Errors
///
/// The hook's refusal or panic, as a [`MigrateError`] naming it.
pub fn run_mutation_hook(hook: &Migration, old: &DynRecord) -> Result<DynRecord, MigrateError> {
    match hook.hook {
        MigrationHook::Mutation(run) => run_hook(hook.name, || run(old)),
        _ => Err(MigrateError::new(format!(
            "the migration hook `{}` is not a mutation hook",
            hook.name
        ))),
    }
}

/// Checks every registered hook's target against `schema`: the E0066 message for each one that
/// names a type, store, signal or mutation the schema does not have, or returns a type that is
/// not the target's (the runtime logs them at ERROR when it starts).
#[must_use]
pub fn check_migrations(schema: &undra_meta::Schema) -> Vec<String> {
    let mut problems = Vec::new();
    for hook in migrations() {
        if let Some(problem) = check_one(schema, hook) {
            problems.push(problem);
        }
    }
    problems
}

fn check_one(schema: &undra_meta::Schema, hook: &Migration) -> Option<String> {
    let returns: Option<TypeRef> = hook.returns.as_ref().map(TypeRef::from);
    let (what, expected): (String, Option<TypeRef>) = match hook.target {
        MigrationTarget::Type(name) => {
            let known = schema.records.iter().any(|r| r.name == name)
                || schema.enums.iter().any(|e| e.name == name);
            if !known {
                return Some(e0066(
                    hook.name,
                    &format!("its target `ty = \"{name}\"` is not a record or enum of this core"),
                ));
            }
            (format!("`{name}`"), Some(TypeRef::named(name)))
        }
        MigrationTarget::Signal { store, signal } => {
            let Some(def) = schema
                .objects
                .iter()
                .find(|o| o.name == store)
                .and_then(|o| o.store.as_ref())
            else {
                return Some(e0066(
                    hook.name,
                    &format!("its target `store = \"{store}\"` is not a store of this core"),
                ));
            };
            let Some(s) = def.signals.iter().find(|s| s.name == signal && !s.computed) else {
                return Some(e0066(
                    hook.name,
                    &format!(
                        "store `{store}` has no plain signal `{signal}` (computed signals are not persisted)"
                    ),
                ));
            };
            (format!("`{store}.{signal}`"), Some(s.ty.clone()))
        }
        MigrationTarget::Mutation(name) => {
            let known = schema
                .queries
                .iter()
                .any(|q| q.name == name && q.kind == undra_meta::QueryKind::Mutation);
            if !known {
                return Some(e0066(
                    hook.name,
                    &format!("its target `mutation = \"{name}\"` is not a mutation of this core"),
                ));
            }
            return None;
        }
    };
    match (returns, expected) {
        (Some(returns), Some(expected)) if returns != expected => Some(e0066(
            hook.name,
            &format!("it returns {returns} but {what} is {expected}"),
        )),
        _ => None,
    }
}

/// The four-part E0066 message of a hook the compiler could not check.
fn e0066(hook: &str, what: &str) -> String {
    format!(
        "error[undra::E0066]: the migration hook `{hook}` cannot run: {what}\n  = note: a hook converts persisted data of one type, store signal or mutation of this core; a target the core does not have never matches, and the data it was written for would be refused or dead-lettered\n  = help: name an existing target in `#[undra::migrate(..)]` (`ty = \"Record\"`, `store = \"Store\", signal = \"field\"` or `mutation = \"name\"`) and return its current type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0066"
    )
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests;
