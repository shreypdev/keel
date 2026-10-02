//! The streaming form of structural migration: old bytes to new bytes, walking the two types side
//! by side, without building a [`DynValue`] tree. It applies exactly the rules of the tree form
//! ([`Converter`](super::Converter)) and is what [`migrate`](super::migrate) runs; a record or enum
//! that does not convert is decoded into a [`DynValue`] only then, to be offered to its type's hook.
//!
//! A record's old fields are located first (their byte spans, by walking the old type) so the new
//! fields can be read by name in any order; a record or enum whose old and new descriptions are
//! identical is copied as bytes.

use std::cell::RefCell;

use undra_meta::{ClosureField, TypeClosure, TypeRef};
use undra_wire::{MAX_DEPTH, Reader, WireError, Writer};

use super::{
    HookSource, MigrateError, SortedEntries, len_u32, not_structural, put_missing, widens,
};

pub(super) struct Streamer<'a> {
    pub(super) old: &'a TypeClosure,
    pub(super) new: &'a TypeClosure,
    pub(super) hooks: &'a dyn HookSource,
    /// Whether the old and the new description of a named type are identical, so its values are
    /// copied as bytes: `(name, same)`, a short list searched without allocating.
    same: RefCell<Vec<(String, bool)>>,
}

impl<'a> Streamer<'a> {
    pub(super) fn new(
        old: &'a TypeClosure,
        new: &'a TypeClosure,
        hooks: &'a dyn HookSource,
    ) -> Streamer<'a> {
        Streamer {
            old,
            new,
            hooks,
            same: RefCell::new(Vec::new()),
        }
    }

    /// Converts one value of `old_ty` read from `r` into `w` as `new_ty`. `hook_here`: a named
    /// value that does not convert may be offered to its type's hook (not at the root, where the
    /// caller offers the item's own hook first).
    #[allow(clippy::too_many_lines)] // one arm per conversion rule
    pub(super) fn convert(
        &self,
        r: &mut Reader<'_>,
        w: &mut Writer,
        old_ty: &TypeRef,
        new_ty: &TypeRef,
        depth: u32,
        hook_here: bool,
    ) -> Result<(), MigrateError> {
        if depth > MAX_DEPTH {
            return Err(WireError::NestingTooDeep { at: r.position() }.into());
        }
        // A newtype is its inner value on the wire (ADR-042): wrapping a value in one, or unwrapping
        // it, copies the bytes through the inner type's conversion.
        if let Some((old_inner, new_inner)) = self.through_newtype(old_ty, new_ty) {
            return self.convert(r, w, old_inner, new_inner, depth + 1, hook_here);
        }
        let refuse = || not_structural(format!("{old_ty} cannot become {new_ty}"));
        match (old_ty, new_ty) {
            (TypeRef::Option(old_inner), TypeRef::Option(new_inner)) => {
                let at = r.position();
                match r.read_u8()? {
                    0 => w.write_u8(0),
                    1 => {
                        w.write_u8(1);
                        self.convert(r, w, old_inner, new_inner, depth + 1, true)?;
                    }
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
            (_, TypeRef::Option(new_inner)) => {
                w.write_u8(1);
                self.convert(r, w, old_ty, new_inner, depth + 1, hook_here)?;
            }
            // A `Lazy<T>` signal is persisted as the `Vec<T>` of its items (ADR-043 decision 3.4), so
            // `Lazy<T>` and `Vec<T>` convert as lists do.
            (
                TypeRef::Vec(old_item) | TypeRef::Lazy(old_item),
                TypeRef::Vec(new_item) | TypeRef::Lazy(new_item),
            ) => {
                let count = r.read_count(1)?;
                w.write_len(len_u32(count)?);
                for i in 0..count {
                    self.convert(r, w, old_item, new_item, depth + 1, true)
                        .map_err(|e| e.within(&format!("[{i}]")))?;
                }
            }
            (TypeRef::Vec(old_item), TypeRef::Bytes) if **old_item == TypeRef::U8 => {
                w.write_bytes(r.read_bytes()?);
            }
            (TypeRef::Bytes, TypeRef::Vec(new_item)) if **new_item == TypeRef::U8 => {
                w.write_bytes(r.read_bytes()?);
            }
            (TypeRef::Map(old_k, old_v), TypeRef::Map(new_k, new_v)) => {
                let count = r.read_count(2)?;
                let mut sorted = SortedEntries::default();
                for _ in 0..count {
                    let mut kw = Writer::new();
                    self.convert(r, &mut kw, old_k, new_k, depth + 1, true)?;
                    let mut vw = Writer::new();
                    self.convert(r, &mut vw, old_v, new_v, depth + 1, true)?;
                    sorted.insert(kw.into_vec(), vw.into_vec())?;
                }
                sorted.write(w)?;
            }
            (TypeRef::F32, TypeRef::F64) => w.write_f64(f64::from(r.read_f32()?)),
            (TypeRef::Named(old_name), TypeRef::Named(new_name)) => {
                let start = r.position();
                if self.same_named(old_name, new_name) {
                    skip(r, old_ty, self.old, depth)?;
                    w.write_raw(span(r, start));
                    return Ok(());
                }
                // Written in place; a failed attempt is taken back before a hook writes instead.
                let mark = w.len();
                let converted = self.convert_named(r, w, old_name, new_name, depth);
                match converted {
                    Ok(()) => {}
                    Err(error) => {
                        w.truncate(mark);
                        if !hook_here {
                            return Err(error);
                        }
                        let from = self
                            .old
                            .narrowed(&TypeRef::named(old_name.clone()))
                            .fingerprint();
                        let Some(hook) = self.hooks.type_hook(new_name, from) else {
                            return Err(error);
                        };
                        // The value as it was stored, from its start, for the hook.
                        let mut again = r.at(start);
                        skip(&mut again, old_ty, self.old, depth)?;
                        let old = (again.consumed_since(start), old_ty, self.old);
                        w.write_raw(&(hook.support.offer)(hook, Some(old))?);
                        // Leave the reader after the value whatever the failed attempt read.
                        *r = again;
                    }
                }
            }
            (a, b) if a == b => {
                let start = r.position();
                skip(r, a, self.old, depth)?;
                w.write_raw(span(r, start));
            }
            (a, b) if widens(a, b) => write_widened(w, read_int(r, a)?, b),
            _ => return Err(refuse()),
        }
        Ok(())
    }

    /// The pair of types to convert instead when exactly one of `old` and `new` is a newtype: the
    /// inner type stands for it.
    fn through_newtype<'t>(
        &'t self,
        old: &'t TypeRef,
        new: &'t TypeRef,
    ) -> Option<(&'t TypeRef, &'t TypeRef)> {
        match (old, new) {
            (TypeRef::Named(_), TypeRef::Named(_)) => None,
            (TypeRef::Named(name), _) => {
                transparent_inner(self.old, name).map(|inner| (inner, new))
            }
            (_, TypeRef::Named(name)) => {
                transparent_inner(self.new, name).map(|inner| (old, inner))
            }
            _ => None,
        }
    }

    /// Whether the named types `old` and `new` are described identically (so their bytes mean the
    /// same): the same name and the same closure.
    fn same_named(&self, old: &str, new: &str) -> bool {
        if old != new {
            return false;
        }
        if let Some((_, same)) = self.same.borrow().iter().find(|(name, _)| name == old) {
            return *same;
        }
        let ty = TypeRef::named(old.to_owned());
        let same = self.old.narrowed(&ty).fingerprint() == self.new.narrowed(&ty).fingerprint();
        self.same.borrow_mut().push((old.to_owned(), same));
        same
    }

    fn convert_named(
        &self,
        r: &mut Reader<'_>,
        w: &mut Writer,
        old_name: &str,
        new_name: &str,
        depth: u32,
    ) -> Result<(), MigrateError> {
        if let (Some(old), Some(new)) = (self.old.record(old_name), self.new.record(new_name)) {
            return self.convert_fields(r, w, &old.fields, &new.fields, depth);
        }
        if let (Some(old), Some(new)) = (self.old.enum_def(old_name), self.new.enum_def(new_name)) {
            let at = r.position();
            let index = r.read_u16()?;
            let Some(old_variant) = old.variants.iter().find(|v| v.index == index) else {
                return Err(WireError::InvalidTag {
                    tag: u32::from(index),
                    at,
                    ty: "enum variant",
                }
                .into());
            };
            let Some(new_variant) = new.variants.iter().find(|v| v.name == old_variant.name) else {
                return Err(not_structural(format!(
                    "`{}` no longer has the variant `{}`",
                    new.name, old_variant.name
                )));
            };
            w.write_u16(new_variant.index);
            return self
                .convert_fields(r, w, &old_variant.fields, &new_variant.fields, depth)
                .map_err(|e| e.within(&old_variant.name));
        }
        // Rare enough to say generically (the path names the place): a type one of the two
        // closures lacks, or a record that became an enum or the other way round.
        if self.new.record(new_name).is_none() && self.new.enum_def(new_name).is_none() {
            return Err(not_structural(
                "the current schema has no such record or enum",
            ));
        }
        if self.old.record(old_name).is_none() && self.old.enum_def(old_name).is_none() {
            return Err(MigrateError::new(
                "the stored description has no such record or enum",
            ));
        }
        Err(not_structural(
            "a record became an enum, or an enum a record",
        ))
    }

    /// The fields of a record or variant, by name: the old ones are located first, then each new
    /// field converts its namesake (or takes `None` / its default).
    fn convert_fields(
        &self,
        r: &mut Reader<'_>,
        w: &mut Writer,
        old_fields: &[ClosureField],
        new_fields: &[ClosureField],
        depth: u32,
    ) -> Result<(), MigrateError> {
        // Most records are small: their spans live on the stack.
        let mut inline = [(0_usize, 0_usize); 16];
        let mut heap = Vec::new();
        let spans: &mut [(usize, usize)] = if old_fields.len() <= inline.len() {
            &mut inline[..old_fields.len()]
        } else {
            heap.resize(old_fields.len(), (0, 0));
            &mut heap
        };
        for (slot, field) in spans.iter_mut().zip(old_fields) {
            let start = r.position();
            skip(r, &field.ty, self.old, depth + 1).map_err(|e| e.within(&field.name))?;
            *slot = (start, r.position());
        }
        for field in new_fields {
            match old_fields.iter().position(|f| f.name == field.name) {
                Some(at) => {
                    let (start, end) = spans[at];
                    let mut sub = r.at(start);
                    self.convert(&mut sub, w, &old_fields[at].ty, &field.ty, depth + 1, true)
                        .map_err(|e| e.within(&field.name))?;
                    if sub.position() != end {
                        return Err(MigrateError::new("a field did not decode to its end")
                            .within(&field.name));
                    }
                }
                None => put_missing(w, field).map_err(|e| e.within(&field.name))?,
            }
        }
        Ok(())
    }
}

/// The type a newtype named `name` wraps in `closure`, if it is one (ADR-042).
pub(super) fn transparent_inner<'c>(closure: &'c TypeClosure, name: &str) -> Option<&'c TypeRef> {
    closure
        .record(name)
        .filter(|record| record.transparent)
        .and_then(|record| record.fields.first())
        .map(|field| &field.ty)
}

/// Writes `i`, read as a narrower integer type, as `ty`: `widens` holds, so it always fits (the
/// checked `put_int` of the tree form is for values a hook built).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // lossless: `widens` holds
fn write_widened(w: &mut Writer, i: i128, ty: &TypeRef) {
    match ty {
        TypeRef::I8 => w.write_i8(i as i8),
        TypeRef::I16 => w.write_i16(i as i16),
        TypeRef::I32 => w.write_i32(i as i32),
        TypeRef::I64 => w.write_i64(i as i64),
        TypeRef::U8 => w.write_u8(i as u8),
        TypeRef::U16 => w.write_u16(i as u16),
        TypeRef::U32 => w.write_u32(i as u32),
        _ => w.write_u64(i as u64),
    }
}

/// The bytes read since `start`.
fn span<'r>(r: &Reader<'r>, start: usize) -> &'r [u8] {
    r.consumed_since(start)
}

fn read_int(r: &mut Reader<'_>, ty: &TypeRef) -> Result<i128, MigrateError> {
    Ok(match ty {
        TypeRef::I8 => i128::from(r.read_i8()?),
        TypeRef::I16 => i128::from(r.read_i16()?),
        TypeRef::I32 => i128::from(r.read_i32()?),
        TypeRef::I64 => i128::from(r.read_i64()?),
        TypeRef::U8 => i128::from(r.read_u8()?),
        TypeRef::U16 => i128::from(r.read_u16()?),
        TypeRef::U32 => i128::from(r.read_u32()?),
        TypeRef::U64 => i128::from(r.read_u64()?),
        _ => return Err(not_structural("not an integer")),
    })
}

/// Reads past one value of `ty` (its lengths, counts, tags and variants checked; the bytes of a
/// string or a primitive are left to whoever decodes the copy), allocating nothing.
pub(super) fn skip(
    r: &mut Reader<'_>,
    ty: &TypeRef,
    closure: &TypeClosure,
    depth: u32,
) -> Result<(), MigrateError> {
    if depth > MAX_DEPTH {
        return Err(WireError::NestingTooDeep { at: r.position() }.into());
    }
    // Lengths only: whoever decodes the copied bytes (the store's restore, the query client)
    // validates them, so a string's UTF-8 or a duration's sign is not checked twice.
    match ty {
        TypeRef::Bool | TypeRef::I8 | TypeRef::U8 => {
            r.read_u8()?;
        }
        TypeRef::I16 | TypeRef::U16 => {
            r.read_u16()?;
        }
        TypeRef::I32 | TypeRef::U32 | TypeRef::F32 => {
            r.read_u32()?;
        }
        TypeRef::I64 | TypeRef::U64 | TypeRef::F64 | TypeRef::Timestamp | TypeRef::Duration => {
            r.read_u64()?;
        }
        TypeRef::String | TypeRef::Bytes => {
            r.read_bytes()?;
        }
        TypeRef::Uuid => {
            r.read_array::<16>()?;
        }
        TypeRef::Decimal => {
            r.read_array::<17>()?;
        }
        TypeRef::Option(inner) => {
            let at = r.position();
            match r.read_u8()? {
                0 => {}
                1 => skip(r, inner, closure, depth + 1)?,
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
        // A `Lazy<T>` signal is persisted as the `Vec<T>` of its items (ADR-043 decision 3.4).
        TypeRef::Vec(item) | TypeRef::Lazy(item) => {
            let count = r.read_count(1)?;
            for _ in 0..count {
                skip(r, item, closure, depth + 1)?;
            }
        }
        TypeRef::Map(k, v) => {
            let count = r.read_count(2)?;
            for _ in 0..count {
                skip(r, k, closure, depth + 1)?;
                skip(r, v, closure, depth + 1)?;
            }
        }
        TypeRef::Named(name) => {
            if let Some(record) = closure.record(name) {
                for field in &record.fields {
                    skip(r, &field.ty, closure, depth + 1)?;
                }
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
                for field in &variant.fields {
                    skip(r, &field.ty, closure, depth + 1)?;
                }
            } else {
                return Err(MigrateError::new(format!(
                    "the stored description has no record or enum `{name}`"
                )));
            }
        }
        TypeRef::Unit
        | TypeRef::Result(..)
        | TypeRef::Stream(_)
        | TypeRef::Object(_)
        | TypeRef::Callback(_) => {
            return Err(MigrateError::new("not a persisted value type"));
        }
    }
    Ok(())
}
