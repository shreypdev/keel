//! `persist`: decode/encode round trips for every wire type (proptest), each structural rule,
//! the refusals, and hooks.

use std::collections::{BTreeMap, HashMap};

use proptest::prelude::*;
use undra_meta::{EnumDef, FieldDef, ParamDef, RecordDef, Schema, TypeRef, VariantDef, ids};
use undra_wire::{Bytes, Encode, Timestamp, Uuid, Writer};

use super::*;

// ----- schema helpers ------------------------------------------------------------------------

fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        default: false,
        docs: String::new(),
    }
}

fn defaulted(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        default: true,
        ..field(name, ty)
    }
}

fn record(name: &str, fields: Vec<FieldDef>) -> RecordDef {
    RecordDef {
        name: name.into(),
        type_id: ids::type_id(name),
        fields,
        docs: String::new(),
    }
}

fn variant(name: &str, index: u16, fields: Vec<FieldDef>, tuple: bool) -> VariantDef {
    VariantDef {
        name: name.into(),
        index,
        fields,
        tuple,
        message: None,
        docs: String::new(),
    }
}

fn enumeration(name: &str, variants: Vec<VariantDef>) -> EnumDef {
    EnumDef {
        name: name.into(),
        type_id: ids::type_id(name),
        is_error: false,
        variants,
        docs: String::new(),
    }
}

fn named(n: &str) -> TypeRef {
    TypeRef::named(n.to_owned())
}

/// `Todo { id: Uuid, title: String, done: bool, tags: Vec<String> }`, `Shape { Circle(f64),
/// Rect { w: f64, h: f64 }, Empty }`, `Tier { Free, Pro }`.
fn base_schema() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record(
        "Todo",
        vec![
            field("id", TypeRef::Uuid),
            field("title", TypeRef::String),
            field("done", TypeRef::Bool),
            field("tags", TypeRef::vec(TypeRef::String)),
        ],
    ));
    s.enums.push(enumeration(
        "Shape",
        vec![
            variant("Circle", 0, vec![field("0", TypeRef::F64)], true),
            variant(
                "Rect",
                1,
                vec![field("w", TypeRef::F64), field("h", TypeRef::F64)],
                false,
            ),
            variant("Empty", 2, vec![], false),
        ],
    ));
    s.enums.push(enumeration(
        "Tier",
        vec![
            variant("Free", 0, vec![], false),
            variant("Pro", 1, vec![], false),
        ],
    ));
    s
}

fn todo_bytes(id: [u8; 16], title: &str, done: bool, tags: &[&str]) -> Vec<u8> {
    let tags: Vec<String> = tags.iter().map(|t| (*t).to_owned()).collect();
    (Uuid(id), title.to_owned(), done, tags).encode_to_vec()
}

/// Migrates `bytes` of `ty` from `old` to `new` without hooks.
fn structural(
    bytes: &[u8],
    old: &Schema,
    old_ty: &TypeRef,
    new: &Schema,
    new_ty: &TypeRef,
) -> Result<Vec<u8>, MigrateError> {
    migrate(
        bytes,
        old_ty,
        &old.closure(old_ty),
        new_ty,
        &new.closure(new_ty),
        &NoHooks,
    )
}

// ----- round trips of concrete Rust types --------------------------------------------------

fn round_trips<T: Encode>(value: &T, ty: &TypeRef) -> Result<(), TestCaseError> {
    let schema = base_schema();
    let closure = schema.closure(ty);
    let bytes = value.encode_to_vec();
    let decoded =
        decode_dyn(&bytes, ty, &closure).map_err(|e| TestCaseError::fail(e.to_string()))?;
    let again =
        encode_dyn(&decoded, ty, &closure).map_err(|e| TestCaseError::fail(e.to_string()))?;
    prop_assert_eq!(&again, &bytes, "encode_dyn(decode_dyn(x)) == x for {}", ty);
    // The identity migration (same type, same closure) is the same bytes too.
    let migrated = migrate(&bytes, ty, &closure, ty, &closure, &NoHooks)
        .map_err(|e| TestCaseError::fail(e.to_string()))?;
    prop_assert_eq!(&migrated, &bytes, "identity migration of {}", ty);
    Ok(())
}

proptest! {
    #[test]
    fn every_primitive_round_trips(
        b in any::<bool>(), i8v in any::<i8>(), i16v in any::<i16>(), i32v in any::<i32>(),
        i64v in any::<i64>(), u8v in any::<u8>(), u16v in any::<u16>(), u32v in any::<u32>(),
        u64v in any::<u64>(), f32bits in any::<u32>(), f64bits in any::<u64>(),
        s in ".{0,20}", bytes in proptest::collection::vec(any::<u8>(), 0..20),
        nanos in 0_i64..i64::MAX, ms in any::<i64>(), uuid in any::<[u8; 16]>(),
    ) {
        round_trips(&b, &TypeRef::Bool)?;
        round_trips(&i8v, &TypeRef::I8)?;
        round_trips(&i16v, &TypeRef::I16)?;
        round_trips(&i32v, &TypeRef::I32)?;
        round_trips(&i64v, &TypeRef::I64)?;
        round_trips(&u8v, &TypeRef::U8)?;
        round_trips(&u16v, &TypeRef::U16)?;
        round_trips(&u32v, &TypeRef::U32)?;
        round_trips(&u64v, &TypeRef::U64)?;
        // Every bit pattern, NaN payloads (signalling ones included) too.
        round_trips(&f32::from_bits(f32bits), &TypeRef::F32)?;
        round_trips(&f64::from_bits(f64bits), &TypeRef::F64)?;
        round_trips(&s, &TypeRef::String)?;
        round_trips(&Bytes(bytes.clone()), &TypeRef::Bytes)?;
        round_trips(&bytes, &TypeRef::vec(TypeRef::U8))?;
        round_trips(&core::time::Duration::from_nanos(nanos.unsigned_abs()), &TypeRef::Duration)?;
        round_trips(&Timestamp(ms), &TypeRef::Timestamp)?;
        round_trips(&Uuid(uuid), &TypeRef::Uuid)?;
    }

    #[test]
    fn containers_round_trip(
        opt in proptest::option::of(any::<i32>()),
        nested in proptest::option::of(proptest::collection::vec(proptest::option::of(".{0,4}"), 0..4)),
        map in proptest::collection::hash_map(".{0,6}", any::<i64>(), 0..6),
        map_u in proptest::collection::btree_map(any::<u16>(), proptest::collection::vec(any::<bool>(), 0..3), 0..6),
    ) {
        round_trips(&opt, &TypeRef::option(TypeRef::I32))?;
        round_trips(&nested, &TypeRef::option(TypeRef::vec(TypeRef::option(TypeRef::String))))?;
        let map: HashMap<String, i64> = map;
        round_trips(&map, &TypeRef::map(TypeRef::String, TypeRef::I64))?;
        let map_u: BTreeMap<u16, Vec<bool>> = map_u;
        round_trips(&map_u, &TypeRef::map(TypeRef::U16, TypeRef::vec(TypeRef::Bool)))?;
    }

    #[test]
    fn records_and_enums_round_trip(
        id in any::<[u8; 16]>(), title in ".{0,10}", done in any::<bool>(),
        tags in proptest::collection::vec(".{0,5}", 0..4),
        radius_bits in any::<u64>(), w in any::<f64>(), h in any::<f64>(), which in 0_u16..3,
    ) {
        let todo = (Uuid(id), title, done, tags);
        round_trips(&todo, &named("Todo"))?;
        let mut shape = Writer::new();
        shape.write_u16(which);
        match which {
            0 => shape.write_f64(f64::from_bits(radius_bits)),
            1 => { shape.write_f64(w); shape.write_f64(h); }
            _ => {}
        }
        let shape = Bytes(shape.into_vec());
        // `Bytes` encodes with a length; strip it to get the raw enum bytes.
        let raw = shape.0;
        let schema = base_schema();
        let closure = schema.closure(&named("Shape"));
        let decoded = decode_dyn(&raw, &named("Shape"), &closure).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(encode_dyn(&decoded, &named("Shape"), &closure).unwrap(), raw);
    }

    /// Hostile bytes never panic the decoder, whatever the type.
    #[test]
    fn arbitrary_bytes_never_panic_the_decoder(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let schema = base_schema();
        for ty in [
            named("Todo"), named("Shape"), TypeRef::vec(named("Todo")),
            TypeRef::map(TypeRef::String, TypeRef::option(named("Shape"))), TypeRef::Duration,
        ] {
            let _ = decode_dyn(&bytes, &ty, &schema.closure(&ty));
        }
    }
}

// ----- random types with random values -----------------------------------------------------

/// A type over the base schema, up to `depth` levels of nesting.
fn arb_type(depth: u32) -> BoxedStrategy<TypeRef> {
    let leaf = prop_oneof![
        Just(TypeRef::Bool),
        Just(TypeRef::I8),
        Just(TypeRef::U16),
        Just(TypeRef::I32),
        Just(TypeRef::U64),
        Just(TypeRef::F32),
        Just(TypeRef::F64),
        Just(TypeRef::String),
        Just(TypeRef::Bytes),
        Just(TypeRef::Duration),
        Just(TypeRef::Timestamp),
        Just(TypeRef::Uuid),
        Just(named("Todo")),
        Just(named("Shape")),
        Just(named("Tier")),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    prop_oneof![
        3 => leaf,
        1 => arb_type(depth - 1).prop_map(|t| match t {
            // `Option<Option<T>>` is not a schema type (E0063).
            TypeRef::Option(_) => t,
            other => TypeRef::option(other),
        }),
        1 => arb_type(depth - 1).prop_map(TypeRef::vec),
        1 => (prop_oneof![Just(TypeRef::String), Just(TypeRef::I32), Just(TypeRef::Uuid)], arb_type(depth - 1))
            .prop_map(|(k, v)| TypeRef::map(k, v)),
    ]
    .boxed()
}

/// A valid encoding of one value of `ty`.
fn arb_bytes(ty: &TypeRef) -> BoxedStrategy<Vec<u8>> {
    fn enc<T: Encode + core::fmt::Debug + 'static>(
        s: impl Strategy<Value = T> + 'static,
    ) -> BoxedStrategy<Vec<u8>> {
        s.prop_map(|v| v.encode_to_vec()).boxed()
    }
    match ty {
        TypeRef::Bool => enc(any::<bool>()),
        TypeRef::I8 => enc(any::<i8>()),
        TypeRef::U16 => enc(any::<u16>()),
        TypeRef::I32 => enc(any::<i32>()),
        TypeRef::U64 => enc(any::<u64>()),
        TypeRef::F32 => enc(any::<u32>().prop_map(f32::from_bits)),
        TypeRef::F64 => enc(any::<u64>().prop_map(f64::from_bits)),
        TypeRef::String => enc(".{0,6}"),
        TypeRef::Bytes => enc(proptest::collection::vec(any::<u8>(), 0..6).prop_map(Bytes)),
        TypeRef::Duration => enc((0_u64..1 << 40).prop_map(core::time::Duration::from_nanos)),
        TypeRef::Timestamp => enc(any::<i64>().prop_map(Timestamp)),
        TypeRef::Uuid => enc(any::<[u8; 16]>().prop_map(Uuid)),
        TypeRef::Option(inner) => prop_oneof![
            Just(vec![0_u8]),
            arb_bytes(inner).prop_map(|b| [vec![1_u8], b].concat()),
        ]
        .boxed(),
        TypeRef::Vec(item) => proptest::collection::vec(arb_bytes(item), 0..3)
            .prop_map(|items| {
                let mut w = Writer::new();
                w.write_len(u32::try_from(items.len()).unwrap());
                for i in items {
                    w.write_raw(&i);
                }
                w.into_vec()
            })
            .boxed(),
        TypeRef::Map(k, v) => proptest::collection::btree_map(arb_bytes(k), arb_bytes(v), 0..3)
            .prop_map(|entries| {
                // A BTreeMap of encoded keys is sorted by those bytes and has each key once.
                let mut w = Writer::new();
                w.write_len(u32::try_from(entries.len()).unwrap());
                for (k, v) in entries {
                    w.write_raw(&k);
                    w.write_raw(&v);
                }
                w.into_vec()
            })
            .boxed(),
        TypeRef::Named(n) if n == "Todo" => (
            any::<[u8; 16]>(),
            ".{0,4}",
            any::<bool>(),
            proptest::collection::vec(".{0,3}", 0..3),
        )
            .prop_map(|(id, t, d, tags)| (Uuid(id), t, d, tags).encode_to_vec())
            .boxed(),
        TypeRef::Named(n) if n == "Tier" => (0_u16..2).prop_map(|i| i.encode_to_vec()).boxed(),
        TypeRef::Named(_) => prop_oneof![
            any::<u64>().prop_map(|b| (0_u16, f64::from_bits(b)).encode_to_vec()),
            (any::<f64>(), any::<f64>()).prop_map(|(w, h)| (1_u16, w, h).encode_to_vec()),
            Just(2_u16.encode_to_vec()),
        ]
        .boxed(),
        other => panic!("arb_bytes: {other}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// For random (nested) types and random valid encodings: `encode_dyn(decode_dyn(x)) == x`,
    /// and the identity migration is the identity.
    #[test]
    fn random_types_round_trip((ty, bytes) in arb_type(3).prop_flat_map(|ty| { let b = arb_bytes(&ty); (Just(ty), b) })) {
        let schema = base_schema();
        let closure = schema.closure(&ty);
        let decoded = decode_dyn(&bytes, &ty, &closure).map_err(|e| TestCaseError::fail(format!("{ty}: {e}")))?;
        prop_assert_eq!(&encode_dyn(&decoded, &ty, &closure).unwrap(), &bytes, "{}", ty);
        prop_assert_eq!(&migrate(&bytes, &ty, &closure, &ty, &closure, &NoHooks).unwrap(), &bytes, "{}", ty);
    }

    /// Wrapping any type in `Option` is structural: the value comes back as `Some`.
    #[test]
    fn t_to_option_t_is_structural((ty, bytes) in arb_type(2).prop_filter("not an option", |t| !matches!(t, TypeRef::Option(_))).prop_flat_map(|ty| { let b = arb_bytes(&ty); (Just(ty), b) })) {
        let schema = base_schema();
        let opt = TypeRef::option(ty.clone());
        let out = structural(&bytes, &schema, &ty, &schema, &opt).unwrap();
        prop_assert_eq!(out, [vec![1_u8], bytes].concat());
    }

    /// Every lossless integer widening keeps the value.
    #[test]
    fn integer_widenings_keep_the_value(v in any::<i8>(), u in any::<u8>(), w in any::<u32>()) {
        let s = Schema::new("t");
        prop_assert_eq!(structural(&v.encode_to_vec(), &s, &TypeRef::I8, &s, &TypeRef::I64).unwrap(), i64::from(v).encode_to_vec());
        prop_assert_eq!(structural(&u.encode_to_vec(), &s, &TypeRef::U8, &s, &TypeRef::I16).unwrap(), i16::from(u).encode_to_vec());
        prop_assert_eq!(structural(&u.encode_to_vec(), &s, &TypeRef::U8, &s, &TypeRef::U64).unwrap(), u64::from(u).encode_to_vec());
        prop_assert_eq!(structural(&w.encode_to_vec(), &s, &TypeRef::U32, &s, &TypeRef::I64).unwrap(), i64::from(w).encode_to_vec());
    }
}

// ----- the structural rules, one by one ----------------------------------------------------

#[test]
fn record_fields_match_by_name_whatever_their_order() {
    let old = base_schema();
    let mut new = base_schema();
    new.records[0].fields.reverse();
    let bytes = todo_bytes([1; 16], "milk", true, &["a"]);
    let out = structural(&bytes, &old, &named("Todo"), &new, &named("Todo")).unwrap();
    let expected = (vec!["a".to_owned()], true, "milk".to_owned(), Uuid([1; 16])).encode_to_vec();
    assert_eq!(out, expected);
}

#[test]
fn an_added_field_takes_none_or_its_default_and_otherwise_refuses() {
    let old = base_schema();
    let bytes = todo_bytes([1; 16], "milk", false, &[]);

    let mut new = base_schema();
    new.records[0]
        .fields
        .push(field("note", TypeRef::option(TypeRef::String)));
    new.records[0]
        .fields
        .push(defaulted("priority", TypeRef::U8));
    new.records[0]
        .fields
        .push(defaulted("seen", TypeRef::vec(TypeRef::Uuid)));
    let out = structural(&bytes, &old, &named("Todo"), &new, &named("Todo")).unwrap();
    assert_eq!(
        out,
        [bytes.clone(), vec![0], vec![0], vec![0, 0, 0, 0]].concat()
    );

    let mut new = base_schema();
    new.records[0].fields.push(field("priority", TypeRef::U8));
    let error = structural(&bytes, &old, &named("Todo"), &new, &named("Todo")).unwrap_err();
    assert_eq!(error.path(), "priority");
    assert!(error.message().contains("no default"), "{error}");

    // A default on a named type has no zero value the schema knows.
    let mut new = base_schema();
    new.records[0].fields.push(defaulted("tier", named("Tier")));
    assert!(structural(&bytes, &old, &named("Todo"), &new, &named("Todo")).is_err());
}

#[test]
fn a_removed_field_is_dropped() {
    let old = base_schema();
    let mut new = base_schema();
    new.records[0].fields.retain(|f| f.name != "tags");
    let bytes = todo_bytes([1; 16], "milk", true, &["x", "y"]);
    let out = structural(&bytes, &old, &named("Todo"), &new, &named("Todo")).unwrap();
    assert_eq!(
        out,
        (Uuid([1; 16]), "milk".to_owned(), true).encode_to_vec()
    );
}

#[test]
fn a_renamed_field_without_a_default_is_not_structural() {
    let old = base_schema();
    let mut new = base_schema();
    new.records[0].fields[1].name = "name".into();
    let bytes = todo_bytes([1; 16], "milk", true, &[]);
    let error = structural(&bytes, &old, &named("Todo"), &new, &named("Todo")).unwrap_err();
    assert_eq!(error.path(), "name");
}

#[test]
fn enum_variants_match_by_name_and_a_missing_one_refuses() {
    let old = base_schema();
    let mut new = base_schema();
    // A variant added in front, the others renumbered.
    new.enums[1].variants = vec![
        variant("Trial", 0, vec![], false),
        variant("Free", 1, vec![], false),
        variant("Pro", 2, vec![], false),
    ];
    let pro = 1_u16.encode_to_vec();
    assert_eq!(
        structural(&pro, &old, &named("Tier"), &new, &named("Tier")).unwrap(),
        2_u16.encode_to_vec()
    );
    // A variant the new type lacks.
    new.enums[1].variants.retain(|v| v.name != "Pro");
    let error = structural(&pro, &old, &named("Tier"), &new, &named("Tier")).unwrap_err();
    assert!(
        error.message().contains("no longer has the variant `Pro`"),
        "{error}"
    );
    // A value of a variant that is still there converts.
    let free = 0_u16.encode_to_vec();
    assert_eq!(
        structural(&free, &old, &named("Tier"), &new, &named("Tier")).unwrap(),
        1_u16.encode_to_vec()
    );
}

#[test]
fn variant_fields_are_records_too() {
    let old = base_schema();
    let mut new = base_schema();
    // `Rect { w, h }` becomes `Rect { h, w, label: Option<String> }`.
    new.enums[0].variants[1].fields = vec![
        field("h", TypeRef::F64),
        field("w", TypeRef::F64),
        field("label", TypeRef::option(TypeRef::String)),
    ];
    let rect = (1_u16, 2.0_f64, 3.0_f64).encode_to_vec();
    let out = structural(&rect, &old, &named("Shape"), &new, &named("Shape")).unwrap();
    assert_eq!(
        out,
        (1_u16, 3.0_f64, 2.0_f64, None::<String>).encode_to_vec()
    );
}

#[test]
fn widening_and_wrapping_rules() {
    let s = Schema::new("t");
    // f32 -> f64.
    assert_eq!(
        structural(
            &1.5_f32.encode_to_vec(),
            &s,
            &TypeRef::F32,
            &s,
            &TypeRef::F64
        )
        .unwrap(),
        1.5_f64.encode_to_vec()
    );
    // Bytes <-> Vec<u8>, both ways.
    let bytes = Bytes(vec![1, 2, 3]).encode_to_vec();
    assert_eq!(
        structural(&bytes, &s, &TypeRef::Bytes, &s, &TypeRef::vec(TypeRef::U8)).unwrap(),
        bytes
    );
    assert_eq!(
        structural(&bytes, &s, &TypeRef::vec(TypeRef::U8), &s, &TypeRef::Bytes).unwrap(),
        bytes
    );
    // Vec, Option and Map recurse.
    let v = vec![Some(1_i16), None].encode_to_vec();
    assert_eq!(
        structural(
            &v,
            &s,
            &TypeRef::vec(TypeRef::option(TypeRef::I16)),
            &s,
            &TypeRef::vec(TypeRef::option(TypeRef::I64))
        )
        .unwrap(),
        vec![Some(1_i64), None].encode_to_vec()
    );
    let mut m = BTreeMap::new();
    m.insert(-1_i8, 1_u8);
    m.insert(5_i8, 2_u8);
    let out = structural(
        &m.encode_to_vec(),
        &s,
        &TypeRef::map(TypeRef::I8, TypeRef::U8),
        &s,
        &TypeRef::map(TypeRef::I64, TypeRef::U32),
    )
    .unwrap();
    // Keys are re-sorted by their new encoded bytes.
    let mut expected = BTreeMap::new();
    expected.insert(-1_i64, 1_u32);
    expected.insert(5_i64, 2_u32);
    assert_eq!(out, expected.encode_to_vec());
}

#[test]
fn narrowings_and_type_changes_are_refused() {
    let s = Schema::new("t");
    for (from, to) in [
        (TypeRef::I64, TypeRef::I32),
        (TypeRef::I32, TypeRef::U32),
        (TypeRef::U32, TypeRef::I32),
        (TypeRef::F64, TypeRef::F32),
        (TypeRef::I32, TypeRef::F32),
        (TypeRef::I64, TypeRef::Timestamp),
        (TypeRef::String, TypeRef::Bytes),
        (TypeRef::option(TypeRef::I32), TypeRef::I32),
    ] {
        let schema = base_schema();
        let bytes = match &from {
            TypeRef::I64 => 7_i64.encode_to_vec(),
            TypeRef::I32 => 7_i32.encode_to_vec(),
            TypeRef::U32 => 7_u32.encode_to_vec(),
            TypeRef::F64 => 7.0_f64.encode_to_vec(),
            TypeRef::String => "x".to_owned().encode_to_vec(),
            _ => Some(7_i32).encode_to_vec(),
        };
        let _ = &s;
        assert!(
            structural(&bytes, &schema, &from, &schema, &to).is_err(),
            "{from} -> {to} must not be structural"
        );
    }
}

#[test]
fn the_audit_probe_is_refused_not_misread() {
    // ADR-037's context: `age: i32 = -5` restored as `f32` read the bytes as NaN; `tier` with a
    // variant added in front read `Pro` as the new first variant. By name and type, neither
    // happens: the first is refused, the second keeps `Pro`.
    let s = base_schema();
    assert!(
        structural(
            &(-5_i32).encode_to_vec(),
            &s,
            &TypeRef::I32,
            &s,
            &TypeRef::F32
        )
        .is_err()
    );
}

// ----- value-directed encoding ---------------------------------------------------------------

#[test]
fn encode_dyn_fits_integers_and_floats_by_value() {
    let s = Schema::new("t");
    let c = s.closure(&TypeRef::U8);
    assert_eq!(
        encode_dyn(&DynValue::Int(255), &TypeRef::U8, &c).unwrap(),
        [255]
    );
    assert!(encode_dyn(&DynValue::Int(256), &TypeRef::U8, &c).is_err());
    assert!(encode_dyn(&DynValue::Int(-1), &TypeRef::U8, &c).is_err());
    let f = s.closure(&TypeRef::F32);
    assert_eq!(
        encode_dyn(&DynValue::Float(0.5), &TypeRef::F32, &f).unwrap(),
        0.5_f32.encode_to_vec()
    );
    assert!(
        encode_dyn(&DynValue::Float(0.1), &TypeRef::F32, &f).is_err(),
        "0.1 is not an f32"
    );
    assert!(encode_dyn(&DynValue::Duration(-1), &TypeRef::Duration, &f).is_err());
}

#[test]
fn encode_params_matches_by_name() {
    let s = base_schema();
    let params = vec![
        ParamDef {
            name: "title".into(),
            ty: TypeRef::String,
        },
        ParamDef {
            name: "due".into(),
            ty: TypeRef::option(TypeRef::Timestamp),
        },
    ];
    let closure = s.closure_of_params(&params);
    let record = DynRecord::new("add")
        .with("extra", DynValue::Bool(true))
        .with("title", DynValue::String("milk".into()));
    assert_eq!(
        encode_params(&record, &closure).unwrap(),
        ("milk".to_owned(), None::<Timestamp>).encode_to_vec()
    );
    let decoded =
        decode_params(&encode_params(&record, &closure).unwrap(), "add", &closure).unwrap();
    assert_eq!(decoded.get("title"), Some(&DynValue::String("milk".into())));
    assert_eq!(decoded.get("due"), Some(&DynValue::None));
    assert!(
        encode_params(&DynRecord::new("add"), &closure).is_err(),
        "title is required"
    );
}

#[test]
fn migrate_params_by_name() {
    let s = base_schema();
    let old = s.closure_of_params(&[
        ParamDef {
            name: "count".into(),
            ty: TypeRef::I32,
        },
        ParamDef {
            name: "gone".into(),
            ty: TypeRef::String,
        },
    ]);
    let new = s.closure_of_params(&[
        ParamDef {
            name: "note".into(),
            ty: TypeRef::option(TypeRef::String),
        },
        ParamDef {
            name: "count".into(),
            ty: TypeRef::I64,
        },
    ]);
    let input = decode_params(&(7_i32, "x".to_owned()).encode_to_vec(), "m", &old).unwrap();
    assert_eq!(
        migrate_params(&input, &old, &new, &NoHooks).unwrap(),
        (None::<String>, 7_i64).encode_to_vec()
    );
}

// ----- decoding limits -----------------------------------------------------------------------

#[test]
fn decoding_is_bounded() {
    let s = base_schema();
    let ty = TypeRef::vec(TypeRef::U64);
    // A count the bytes cannot hold is refused before anything is allocated.
    let error = decode_dyn(&u32::MAX.to_le_bytes(), &ty, &s.closure(&ty)).unwrap_err();
    assert!(error.message().contains("do not decode"), "{error}");
    // Trailing bytes.
    assert!(decode_dyn(&[1, 0], &TypeRef::Bool, &s.closure(&TypeRef::Bool)).is_err());
    // A type the closure does not describe.
    let unknown = named("Nope");
    assert!(decode_dyn(&[0], &unknown, &s.closure(&unknown)).is_err());
    // A negative duration.
    assert!(
        decode_dyn(
            &(-1_i64).encode_to_vec(),
            &TypeRef::Duration,
            &s.closure(&TypeRef::Duration)
        )
        .is_err()
    );
    // Deep nesting: `Option<Option<..>>` is not a schema type, but a recursive record is.
    let mut deep = Schema::new("t");
    deep.records.push(record(
        "Node",
        vec![field("next", TypeRef::option(named("Node")))],
    ));
    let bytes = vec![1_u8; 4096];
    let error = decode_dyn(&bytes, &named("Node"), &deep.closure(&named("Node"))).unwrap_err();
    assert!(error.to_string().contains("nest"), "{error}");
}

// ----- hooks ---------------------------------------------------------------------------------

/// A test hook source with one `ty` hook per name.
struct Hooks(Vec<&'static Migration>);

impl HookSource for Hooks {
    fn type_hook(&self, name: &str, from: u64) -> Option<&Migration> {
        self.0
            .iter()
            .copied()
            .filter(|m| matches!(m.target, MigrationTarget::Type(t) if t == name))
            .find(|m| m.from.is_none_or(|f| f == from))
    }
}

fn title_from_name(old: &DynValue) -> Result<Vec<u8>, MigrateError> {
    let title = old
        .field("name")
        .and_then(DynValue::as_str)
        .ok_or_else(|| MigrateError::new("no name"))?;
    let id = match old.field("id") {
        Some(DynValue::Uuid(u)) => *u,
        _ => return Err(MigrateError::new("no id")),
    };
    Ok(todo_bytes(id, title, false, &[]))
}

static RENAME_HOOK: Migration = Migration {
    name: "tests::title_from_name",
    target: MigrationTarget::Type("Todo"),
    from: None,
    returns: None,
    hook: MigrationHook::Value(title_from_name),
};

fn panicking(_: &DynValue) -> Result<Vec<u8>, MigrateError> {
    panic!("the hook panicked on purpose")
}

static PANICKING_HOOK: Migration = Migration {
    name: "tests::panicking",
    target: MigrationTarget::Type("Todo"),
    from: None,
    returns: None,
    hook: MigrationHook::Value(panicking),
};

/// The old `Todo` had `name` where the new one has `title`.
fn renamed_schema() -> Schema {
    let mut old = base_schema();
    old.records[0].fields = vec![field("id", TypeRef::Uuid), field("name", TypeRef::String)];
    old
}

#[test]
fn a_type_hook_converts_what_is_not_structural_at_any_depth() {
    let old = renamed_schema();
    let new = base_schema();
    let ty = TypeRef::vec(named("Todo"));
    let bytes = vec![(Uuid([3; 16]), "milk".to_owned())].encode_to_vec();
    // Without a hook: refused, at the item.
    let error = structural(&bytes, &old, &ty, &new, &ty).unwrap_err();
    assert_eq!(error.path(), "[0].title");
    // With one: converted.
    let out = migrate(
        &bytes,
        &ty,
        &old.closure(&ty),
        &ty,
        &new.closure(&ty),
        &Hooks(vec![&RENAME_HOOK]),
    )
    .unwrap();
    let mut expected = Writer::new();
    expected.write_len(1);
    expected.write_raw(&todo_bytes([3; 16], "milk", false, &[]));
    assert_eq!(out, expected.into_vec());
}

#[test]
fn a_hook_restricted_to_another_fingerprint_does_not_run() {
    static PINNED: Migration = Migration {
        name: "tests::pinned",
        target: MigrationTarget::Type("Todo"),
        from: Some(42),
        returns: None,
        hook: MigrationHook::Value(title_from_name),
    };
    let old = renamed_schema();
    let new = base_schema();
    let ty = TypeRef::vec(named("Todo"));
    let bytes = vec![(Uuid([3; 16]), "milk".to_owned())].encode_to_vec();
    assert!(
        migrate(
            &bytes,
            &ty,
            &old.closure(&ty),
            &ty,
            &new.closure(&ty),
            &Hooks(vec![&PINNED])
        )
        .is_err()
    );
    // The right fingerprint is the old `Todo`'s own closure.
    let from = old.closure(&named("Todo")).fingerprint();
    assert_eq!(
        old.closure(&ty).narrowed(&named("Todo")).fingerprint(),
        from
    );
}

#[test]
fn a_panicking_hook_is_a_failed_migration_naming_it() {
    let old = renamed_schema();
    let new = base_schema();
    let ty = TypeRef::vec(named("Todo"));
    let bytes = vec![(Uuid([3; 16]), "milk".to_owned())].encode_to_vec();
    let error = migrate(
        &bytes,
        &ty,
        &old.closure(&ty),
        &ty,
        &new.closure(&ty),
        &Hooks(vec![&PANICKING_HOOK]),
    )
    .unwrap_err();
    assert!(error.to_string().contains("tests::panicking"), "{error}");
    assert!(error.to_string().contains("panicked"), "{error}");
}

#[test]
fn the_root_value_is_not_offered_to_a_type_hook_by_migrate() {
    // `migrate` leaves the root to the caller, which offers it to the item's hook first and to
    // the root type's hook after (decision 5).
    let old = renamed_schema();
    let new = base_schema();
    let ty = named("Todo");
    let bytes = (Uuid([3; 16]), "milk".to_owned()).encode_to_vec();
    assert!(
        migrate(
            &bytes,
            &ty,
            &old.closure(&ty),
            &ty,
            &new.closure(&ty),
            &Hooks(vec![&RENAME_HOOK])
        )
        .is_err()
    );
}

#[test]
fn run_value_hook_and_mutation_hook() {
    fn as_signal(old: Option<&DynValue>) -> Result<Vec<u8>, MigrateError> {
        Ok(match old.and_then(DynValue::as_i64) {
            Some(i) => (i as f32).encode_to_vec(),
            None => 18.0_f32.encode_to_vec(),
        })
    }
    fn add_due(old: &DynRecord) -> Result<DynRecord, MigrateError> {
        let mut new = old.clone();
        new.rename("name", "title");
        Ok(new)
    }
    let signal = Migration {
        name: "s",
        target: MigrationTarget::Signal {
            store: "Profile",
            signal: "age",
        },
        from: None,
        returns: None,
        hook: MigrationHook::Signal(as_signal),
    };
    assert_eq!(
        run_value_hook(&signal, None).unwrap(),
        18.0_f32.encode_to_vec()
    );
    assert_eq!(
        run_value_hook(&signal, Some(&DynValue::Int(7))).unwrap(),
        7.0_f32.encode_to_vec()
    );
    let mutation = Migration {
        name: "m",
        target: MigrationTarget::Mutation("add"),
        from: None,
        returns: None,
        hook: MigrationHook::Mutation(add_due),
    };
    let out = run_mutation_hook(
        &mutation,
        &DynRecord::new("add").with("name", DynValue::String("x".into())),
    )
    .unwrap();
    assert_eq!(out.get("title"), Some(&DynValue::String("x".into())));
    assert!(run_mutation_hook(&signal, &DynRecord::default()).is_err());
}

#[test]
fn dyn_record_editing() {
    let mut r = DynRecord::new("T")
        .with("a", DynValue::Int(1))
        .with("b", DynValue::Bool(true));
    r.set("a", DynValue::Int(2));
    assert_eq!(r.get("a"), Some(&DynValue::Int(2)));
    assert_eq!(r.remove("b"), Some(DynValue::Bool(true)));
    r.rename("a", "c");
    assert_eq!(r.fields, vec![("c".to_owned(), DynValue::Int(2))]);
    assert_eq!(DynValue::Float32(1.5).as_f64(), Some(1.5));
    assert_eq!(DynValue::Int(i128::MAX).as_i64(), None);
    let e = DynValue::Enum {
        name: "Tier".into(),
        variant: "Pro".into(),
        fields: DynRecord::new("Tier"),
    };
    assert_eq!(e.variant(), Some("Pro"));
    assert_eq!(MigrateError::new("x").within("b").within("a").path(), "a.b");
    assert_eq!(
        MigrateError::new("x").within("[2]").within("a").to_string(),
        "a[2]: x"
    );
}
