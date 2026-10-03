#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod diag;
pub mod dispatch;
pub mod ids;
#[doc(hidden)]
pub mod keys;

mod canonical;
mod closure;
mod closure_json;
mod def;
mod meta;
mod registry;
mod schema_json;
mod sort;
mod type_ref;
mod validate;

#[cfg(test)]
mod fixtures;

pub use closure::{
    ClosureEnum, ClosureField, ClosureRecord, ClosureRoot, ClosureSignal, ClosureVariant,
    DescribedStore, StoresClosure, TypeClosure,
};
pub use closure_json::ClosureJsonError;
pub use def::{
    EnumDef, FieldDef, FunctionDef, GenericArg, GenericOf, InfiniteDef, MethodDef, ObjectDef,
    ParamDef, PortDef, PortKind, QueryDef, QueryKind, RecordDef, Schema, SignalDef, StoreDef,
    UNDRA_VERSION, VariantDef,
};
pub use dispatch::{DispatchCall, DispatchFn, DispatchOutcome};
pub use meta::{
    EnumMeta, FieldMeta, FunctionMeta, GenericArgMeta, GenericOfMeta, InfiniteMeta, MethodMeta,
    ObjectMeta, ParamMeta, PortMeta, QueryMeta, RecordMeta, SignalMeta, StoreMeta, TypeRefMeta,
    VariantMeta,
};
pub use registry::{Registration, collect_schema, registrations, schema_from_registrations};
pub use type_ref::TypeRef;
pub use validate::{SchemaError, TypeKind};

/// Re-export of the `inventory` crate, so macro output can write
/// `undra_meta::inventory::submit! { .. }` without the user's crate depending
/// on `inventory` directly.
pub use inventory;
