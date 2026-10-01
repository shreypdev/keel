#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod dispatch;
pub mod ids;
#[doc(hidden)]
pub mod keys;

mod canonical;
mod def;
mod meta;
mod registry;
mod type_ref;
mod validate;

#[cfg(test)]
mod fixtures;

pub use def::{
    EnumDef, FieldDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind, QueryDef,
    QueryKind, RecordDef, Schema, SignalDef, StoreDef, UNDRA_VERSION, VariantDef,
};
pub use dispatch::{DispatchCall, DispatchFn, DispatchOutcome};
pub use meta::{
    EnumMeta, FieldMeta, FunctionMeta, MethodMeta, ObjectMeta, ParamMeta, PortMeta, QueryMeta,
    RecordMeta, SignalMeta, StoreMeta, TypeRefMeta, VariantMeta,
};
pub use registry::{Registration, collect_schema, registrations, schema_from_registrations};
pub use type_ref::TypeRef;
pub use validate::{SchemaError, TypeKind};

/// Re-export of the `inventory` crate, so macro output can write
/// `undra_meta::inventory::submit! { .. }` without the user's crate depending
/// on `inventory` directly.
pub use inventory;
