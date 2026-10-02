//! Registration via `inventory` (SPEC §2.4).
//!
//! Each Undra macro submits one [`Registration`] per public item. At runtime
//! (or when a host extracts the schema) [`collect_schema`] gathers every
//! registration linked into the process and builds the owned [`Schema`].
//!
//! ```
//! use undra_meta::{FieldMeta, Registration, RecordMeta, TypeRefMeta, collect_schema, ids};
//!
//! static POINT: RecordMeta = RecordMeta {
//!     name: "Point",
//!     type_id: ids::type_id("Point"),
//!     fields: &[
//!         FieldMeta { name: "x", ty: TypeRefMeta::F64, default: false, docs: "" },
//!         FieldMeta { name: "y", ty: TypeRefMeta::F64, default: false, docs: "" },
//!     ],
//!     transparent: false, docs: "",
//! };
//! undra_meta::inventory::submit! { Registration::Record(&POINT) }
//!
//! let schema = collect_schema("geometry");
//! assert!(schema.records.iter().any(|r| r.name == "Point"));
//! ```
//!
//! # Caveats
//!
//! * Registrations carry no crate name: [`collect_schema`] returns *everything*
//!   linked into the process. A host that links several Undra cores into one
//!   binary gets one merged schema (and [`Schema::validate`] will flag name
//!   clashes).
//! * `inventory` relies on link-time constructors. On `wasm32-unknown-unknown`
//!   the host must call the module's `_initialize` (or `__wasm_call_ctors`)
//!   once after instantiation before collecting (SPEC §7).

use crate::{
    EnumDef, EnumMeta, FunctionDef, FunctionMeta, ObjectDef, ObjectMeta, PortDef, PortMeta,
    QueryDef, QueryMeta, RecordDef, RecordMeta, Schema,
};

/// One registered schema item, submitted with `inventory::submit!`.
///
/// Errors are registered as [`Registration::Enum`] with `is_error = true`,
/// stores as [`Registration::Object`] with `store = Some(..)`.
#[derive(Clone, Copy, Debug)]
pub enum Registration {
    /// A record.
    Record(&'static RecordMeta),
    /// An enum or error.
    Enum(&'static EnumMeta),
    /// An object or store, with its dispatcher.
    Object(&'static ObjectMeta),
    /// A free function, with its dispatcher.
    Function(&'static FunctionMeta),
    /// A port.
    Port(&'static PortMeta),
    /// A query or mutation.
    Query(&'static QueryMeta),
}

inventory::collect!(Registration);

/// Iterates over every [`Registration`] linked into the process, in an
/// unspecified order.
///
/// Objects and functions expose their `dispatch` pointers here; this is how
/// `undra-runtime` builds its dispatch table.
pub fn registrations() -> impl Iterator<Item = &'static Registration> {
    inventory::iter::<Registration>.into_iter()
}

/// Builds the owned [`Schema`] from every registration in the process.
///
/// The result is deterministic: `inventory` yields items in an unspecified
/// order, so the top-level lists are sorted by name. Everything else keeps its
/// declared order. The schema is stamped with [`UNDRA_VERSION`](crate::UNDRA_VERSION)
/// and `crate_name`.
#[must_use]
pub fn collect_schema(crate_name: &str) -> Schema {
    schema_from_registrations(crate_name, registrations())
}

/// Builds a [`Schema`] from an explicit set of registrations.
///
/// This is what [`collect_schema`] does with the process-wide set; it is
/// exposed so tools and tests can build a schema from registrations they hold
/// themselves. The top-level lists are sorted by name.
#[must_use]
pub fn schema_from_registrations<'a>(
    crate_name: &str,
    registrations: impl IntoIterator<Item = &'a Registration>,
) -> Schema {
    let mut schema = Schema::new(crate_name);
    for registration in registrations {
        match registration {
            Registration::Record(m) => schema.records.push(RecordDef::from(*m)),
            Registration::Enum(m) => schema.enums.push(EnumDef::from(*m)),
            Registration::Object(m) => schema.objects.push(ObjectDef::from(*m)),
            Registration::Function(m) => schema.functions.push(FunctionDef::from(*m)),
            Registration::Port(m) => schema.ports.push(PortDef::from(*m)),
            Registration::Query(m) => schema.queries.push(QueryDef::from(*m)),
        }
    }
    crate::sort::by_name(&mut schema.records, |d| &d.name);
    crate::sort::by_name(&mut schema.enums, |d| &d.name);
    crate::sort::by_name(&mut schema.objects, |d| &d.name);
    crate::sort::by_name(&mut schema.functions, |d| &d.name);
    crate::sort::by_name(&mut schema.ports, |d| &d.name);
    crate::sort::by_name(&mut schema.queries, |d| &d.name);
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldMeta, TypeRefMeta, ids};

    static A: RecordMeta = RecordMeta {
        name: "Alpha",
        type_id: ids::type_id("Alpha"),
        fields: &[],
        transparent: false,
        docs: "",
    };
    static Z: RecordMeta = RecordMeta {
        name: "Zulu",
        type_id: ids::type_id("Zulu"),
        fields: &[FieldMeta {
            name: "n",
            ty: TypeRefMeta::U8,
            default: false,
            docs: "",
        }],
        transparent: false,
        docs: "",
    };

    #[test]
    fn explicit_registrations_are_sorted_by_name() {
        let regs = [Registration::Record(&Z), Registration::Record(&A)];
        let schema = schema_from_registrations("demo", &regs);
        assert_eq!(schema.crate_name, "demo");
        assert_eq!(schema.undra_version, crate::UNDRA_VERSION);
        let names: Vec<_> = schema.records.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Zulu"]);
        assert!(schema.enums.is_empty() && schema.objects.is_empty());
    }

    #[test]
    fn input_order_does_not_change_the_result() {
        let forward = [Registration::Record(&A), Registration::Record(&Z)];
        let backward = [Registration::Record(&Z), Registration::Record(&A)];
        assert_eq!(
            schema_from_registrations("demo", &forward),
            schema_from_registrations("demo", &backward)
        );
    }

    #[test]
    fn empty_registrations_give_an_empty_schema() {
        let schema = schema_from_registrations("empty", &[]);
        assert_eq!(schema, Schema::new("empty"));
    }
}
