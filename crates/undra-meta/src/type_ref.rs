//! [`TypeRef`]: a reference to a type in the Undra type system (SPEC §2.1).

use core::fmt;

use serde::{Deserialize, Serialize};

/// A reference to an Undra wire type.
///
/// Serialized as `{"kind": <snake_case variant>, "of": <content>}`; unit
/// variants have no `of`, and [`TypeRef::Map`] serializes `of` as a two-element
/// array `[key, value]`:
///
/// ```
/// use undra_meta::TypeRef;
///
/// let ty = TypeRef::option(TypeRef::named("Todo"));
/// assert_eq!(
///     serde_json::to_string(&ty).unwrap(),
///     r#"{"kind":"option","of":{"kind":"named","of":"Todo"}}"#
/// );
///
/// let map = TypeRef::map(TypeRef::String, TypeRef::I32);
/// assert_eq!(
///     serde_json::to_string(&map).unwrap(),
///     r#"{"kind":"map","of":[{"kind":"string"},{"kind":"i32"}]}"#
/// );
/// ```
///
/// `Result` and `Stream` are only legal in return position, `Lazy` only as a
/// store signal type, and `Unit` only as a return type or as a variant with no
/// fields (never inside `Option`, `Vec`, map values or `Lazy`, and never as a
/// field, parameter or signal type).
/// [`Schema::validate`](crate::Schema::validate) enforces this, the type
/// itself does not.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "of", rename_all = "snake_case")]
pub enum TypeRef {
    /// `bool`.
    Bool,
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// 32-bit IEEE 754 float.
    F32,
    /// 64-bit IEEE 754 float.
    F64,
    /// UTF-8 string.
    String,
    /// Raw byte string.
    Bytes,
    /// The unit type (no value).
    Unit,
    /// A duration, encoded as `i64` nanoseconds.
    Duration,
    /// A point in time, encoded as `i64` milliseconds since the Unix epoch.
    Timestamp,
    /// A 128-bit UUID.
    Uuid,
    /// A decimal number: a 128-bit two's-complement mantissa and a scale of at most 38, value =
    /// mantissa x 10^-scale (ADR-042). Not a map key.
    Decimal,
    /// `Option<T>`.
    Option(Box<TypeRef>),
    /// `Vec<T>`.
    Vec(Box<TypeRef>),
    /// `Map<K, V>`; the key must satisfy [`TypeRef::is_valid_map_key`].
    Map(Box<TypeRef>, Box<TypeRef>),
    /// A lazily paged list handle; the content is the item type.
    Lazy(Box<TypeRef>),
    /// A record, enum or error, by type name. An object is named by [`TypeRef::Object`], except
    /// as the return type of its own constructors, which keep naming it with `Named` (the schema
    /// of an object that only has constructors did not change, so no hash moved, ADR-040).
    Named(String),
    /// `Result<T, E>`; only as a return type.
    Result(Box<TypeRef>, Box<TypeRef>),
    /// A stream of items; only as a return type.
    Stream(Box<TypeRef>),
    /// An object (an `#[undra::api] impl`, a store included) crossing as a parameter or a
    /// return, by type name: one owned host reference on its way out of the core, a borrowed
    /// reference on its way in (ADR-040). `{"kind":"object","of":"Mailbox"}`.
    Object(String),
    /// A host-implemented callback interface (`#[undra::callback]`) passed in as a parameter, by
    /// trait name: it names a [`PortDef`](crate::PortDef) of kind
    /// [`PortKind::Callback`](crate::PortKind::Callback) (ADR-041).
    /// `{"kind":"callback","of":"UploadListener"}`.
    Callback(String),
}

impl TypeRef {
    /// `Option<inner>`.
    #[must_use]
    pub fn option(inner: TypeRef) -> TypeRef {
        TypeRef::Option(Box::new(inner))
    }

    /// `Vec<item>`.
    #[must_use]
    pub fn vec(item: TypeRef) -> TypeRef {
        TypeRef::Vec(Box::new(item))
    }

    /// `Map<key, value>`.
    #[must_use]
    pub fn map(key: TypeRef, value: TypeRef) -> TypeRef {
        TypeRef::Map(Box::new(key), Box::new(value))
    }

    /// `Lazy<item>`.
    #[must_use]
    pub fn lazy(item: TypeRef) -> TypeRef {
        TypeRef::Lazy(Box::new(item))
    }

    /// A reference to the record, enum, error or object called `name`.
    #[must_use]
    pub fn named(name: impl Into<String>) -> TypeRef {
        TypeRef::Named(name.into())
    }

    /// A reference to the object called `name` (ADR-040).
    #[must_use]
    pub fn object(name: impl Into<String>) -> TypeRef {
        TypeRef::Object(name.into())
    }

    /// A reference to the callback interface called `name` (ADR-041).
    #[must_use]
    pub fn callback(name: impl Into<String>) -> TypeRef {
        TypeRef::Callback(name.into())
    }

    /// `Result<ok, err>`.
    #[must_use]
    pub fn result(ok: TypeRef, err: TypeRef) -> TypeRef {
        TypeRef::Result(Box::new(ok), Box::new(err))
    }

    /// `Stream<item>`.
    #[must_use]
    pub fn stream(item: TypeRef) -> TypeRef {
        TypeRef::Stream(Box::new(item))
    }

    /// Whether this is one of the eight fixed-width integer types.
    #[must_use]
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            TypeRef::I8
                | TypeRef::I16
                | TypeRef::I32
                | TypeRef::I64
                | TypeRef::U8
                | TypeRef::U16
                | TypeRef::U32
                | TypeRef::U64
        )
    }

    /// Whether this type may be used as a map key, **without a schema to resolve names**:
    /// `String`, an integer, `Bool` or `Uuid` (SPEC §2.1). A newtype (ADR-042) of one of those is
    /// a valid key too; [`Schema::is_valid_map_key`](crate::Schema::is_valid_map_key) resolves it.
    #[must_use]
    pub fn is_valid_map_key(&self) -> bool {
        self.is_integer() || matches!(self, TypeRef::String | TypeRef::Bool | TypeRef::Uuid)
    }
}

/// Prints a compact, readable form such as `option<named:Todo>`,
/// `map<string,i32>` or `result<unit,named:TodoError>`.
///
/// ```
/// use undra_meta::TypeRef;
///
/// assert_eq!(TypeRef::option(TypeRef::named("Todo")).to_string(), "option<named:Todo>");
/// assert_eq!(TypeRef::map(TypeRef::String, TypeRef::I32).to_string(), "map<string,i32>");
/// ```
impl fmt::Display for TypeRef {
    // The kind's name (the `kind` tag of the JSON form), then the inner types: written without
    // `write!`, since every core that migrates persisted data links it (ADR-052).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::closure_json::kind_name(self))?;
        match self {
            TypeRef::Option(t) | TypeRef::Vec(t) | TypeRef::Lazy(t) | TypeRef::Stream(t) => {
                f.write_str("<")?;
                t.fmt(f)?;
                f.write_str(">")
            }
            TypeRef::Map(a, b) | TypeRef::Result(a, b) => {
                f.write_str("<")?;
                a.fmt(f)?;
                f.write_str(",")?;
                b.fmt(f)?;
                f.write_str(">")
            }
            TypeRef::Named(n) | TypeRef::Object(n) | TypeRef::Callback(n) => {
                f.write_str(":")?;
                f.write_str(n)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_leaf_types() -> Vec<(TypeRef, &'static str)> {
        vec![
            (TypeRef::Bool, "bool"),
            (TypeRef::I8, "i8"),
            (TypeRef::I16, "i16"),
            (TypeRef::I32, "i32"),
            (TypeRef::I64, "i64"),
            (TypeRef::U8, "u8"),
            (TypeRef::U16, "u16"),
            (TypeRef::U32, "u32"),
            (TypeRef::U64, "u64"),
            (TypeRef::F32, "f32"),
            (TypeRef::F64, "f64"),
            (TypeRef::String, "string"),
            (TypeRef::Bytes, "bytes"),
            (TypeRef::Unit, "unit"),
            (TypeRef::Duration, "duration"),
            (TypeRef::Timestamp, "timestamp"),
            (TypeRef::Uuid, "uuid"),
            (TypeRef::Decimal, "decimal"),
        ]
    }

    #[test]
    fn unit_variants_serialize_as_kind_only() {
        for (ty, name) in all_leaf_types() {
            let json = serde_json::to_string(&ty).unwrap();
            assert_eq!(json, format!(r#"{{"kind":"{name}"}}"#));
            assert_eq!(serde_json::from_str::<TypeRef>(&json).unwrap(), ty);
        }
    }

    #[test]
    fn spec_examples_serialize_exactly() {
        assert_eq!(
            serde_json::to_string(&TypeRef::String).unwrap(),
            r#"{"kind":"string"}"#
        );
        assert_eq!(
            serde_json::to_string(&TypeRef::option(TypeRef::named("Todo"))).unwrap(),
            r#"{"kind":"option","of":{"kind":"named","of":"Todo"}}"#
        );
        assert_eq!(
            serde_json::to_string(&TypeRef::map(TypeRef::String, TypeRef::I32)).unwrap(),
            r#"{"kind":"map","of":[{"kind":"string"},{"kind":"i32"}]}"#
        );
    }

    #[test]
    fn composite_variants_round_trip() {
        let cases = [
            TypeRef::vec(TypeRef::option(TypeRef::vec(TypeRef::Bytes))),
            TypeRef::option(TypeRef::option(TypeRef::U8)),
            TypeRef::map(TypeRef::Uuid, TypeRef::vec(TypeRef::named("Todo"))),
            TypeRef::lazy(TypeRef::named("Todo")),
            TypeRef::option(TypeRef::object("Mailbox")),
            TypeRef::vec(TypeRef::object("Mailbox")),
            TypeRef::callback("UploadListener"),
            TypeRef::result(TypeRef::Unit, TypeRef::named("TodoError")),
            TypeRef::result(
                TypeRef::stream(TypeRef::named("Todo")),
                TypeRef::named("TodoError"),
            ),
            TypeRef::stream(TypeRef::U32),
            TypeRef::named("Ünïcode\u{1F30A}"),
            TypeRef::named(""),
        ];
        for ty in cases {
            let json = serde_json::to_string(&ty).unwrap();
            let back: TypeRef = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ty, "round trip of {json}");
        }
    }

    #[test]
    fn objects_and_callbacks_serialize_by_name() {
        assert_eq!(
            serde_json::to_string(&TypeRef::object("Mailbox")).unwrap(),
            r#"{"kind":"object","of":"Mailbox"}"#
        );
        assert_eq!(
            serde_json::to_string(&TypeRef::callback("UploadListener")).unwrap(),
            r#"{"kind":"callback","of":"UploadListener"}"#
        );
        assert_eq!(TypeRef::object("Mailbox").to_string(), "object:Mailbox");
        assert_eq!(
            TypeRef::option(TypeRef::callback("L")).to_string(),
            "option<callback:L>"
        );
    }

    #[test]
    fn map_and_result_content_is_a_two_element_array() {
        let json = serde_json::to_value(TypeRef::result(TypeRef::Unit, TypeRef::String)).unwrap();
        assert!(json["of"].as_array().is_some_and(|a| a.len() == 2));
    }

    #[test]
    fn rejects_malformed_type_refs() {
        for bad in [
            r#"{"kind":"nope"}"#,
            r#"{"kind":"option"}"#,
            r#"{"kind":"map","of":[{"kind":"string"}]}"#,
            r#"{"kind":"map","of":{"kind":"string"}}"#,
            r#"{"kind":"named","of":7}"#,
            r#"{"of":"Todo"}"#,
            r#""string""#,
        ] {
            assert!(serde_json::from_str::<TypeRef>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn display_is_readable() {
        assert_eq!(
            TypeRef::option(TypeRef::named("Todo")).to_string(),
            "option<named:Todo>"
        );
        assert_eq!(
            TypeRef::map(TypeRef::String, TypeRef::vec(TypeRef::I32)).to_string(),
            "map<string,vec<i32>>"
        );
        assert_eq!(
            TypeRef::result(TypeRef::stream(TypeRef::U8), TypeRef::named("E")).to_string(),
            "result<stream<u8>,named:E>"
        );
        assert_eq!(TypeRef::lazy(TypeRef::Uuid).to_string(), "lazy<uuid>");
        for (ty, name) in all_leaf_types() {
            assert_eq!(ty.to_string(), name);
        }
    }

    #[test]
    fn map_key_rules() {
        for ok in [
            TypeRef::String,
            TypeRef::Bool,
            TypeRef::Uuid,
            TypeRef::I8,
            TypeRef::I16,
            TypeRef::I32,
            TypeRef::I64,
            TypeRef::U8,
            TypeRef::U16,
            TypeRef::U32,
            TypeRef::U64,
        ] {
            assert!(ok.is_valid_map_key(), "{ok}");
        }
        for bad in [
            TypeRef::F32,
            TypeRef::F64,
            TypeRef::Bytes,
            TypeRef::Unit,
            TypeRef::Duration,
            TypeRef::Timestamp,
            TypeRef::Decimal,
            TypeRef::option(TypeRef::String),
            TypeRef::vec(TypeRef::String),
            TypeRef::named("Id"),
            TypeRef::object("Mailbox"),
        ] {
            assert!(!bad.is_valid_map_key(), "{bad}");
        }
    }
}
