//! Newtypes, named generic instantiations and `Decimal` (ADR-042), compiled against the real facade
//! and run: the generated codecs round-trip, the schema says what the ADR says, a map may be keyed
//! by a newtype of a key, and a keyed list may use one as its key field.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};

use undra::meta::{RecordDef, Schema, TypeRef, collect_schema, ids};
use undra::prelude::{Ctx, Signal};
use undra::wire::payload::ChangeOp;
use undra::wire::{
    Decimal, Decode, Encode, Handle, KeyedPatch, PatchOp, Reader, Uuid, WireError, Writer,
};
use undra_macros as k;

mod support;
use support::Runtime;

fn schema() -> Schema {
    collect_schema("newtypes-generics-test")
}

fn record(name: &str) -> RecordDef {
    schema()
        .records
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("record {name} not registered"))
}

fn named(name: &str) -> TypeRef {
    TypeRef::Named(name.to_owned())
}

// ---------------------------------------------------------------------------------------------
// Newtypes
// ---------------------------------------------------------------------------------------------

/// A user's id.
#[k::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(pub Uuid);

/// A length in metres.
#[k::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meters(f64);

/// A newtype of a newtype: still a key, because the innermost type is.
#[k::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Owner(pub UserId);

/// A newtype of a collection: any value type may be wrapped.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Tags(pub Vec<String>);

/// Money: a newtype of `Decimal`, which is not a map key.
#[k::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price(pub Decimal);

/// A code that identifies a SKU: a newtype of `String`, a key.
#[k::api]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sku(pub String);

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub id: UserId,
    pub tags: Tags,
    pub total: Price,
    pub by_sku: HashMap<Sku, u32>,
    pub owners: BTreeMap<Owner, Vec<UserId>>,
    pub height: Option<Meters>,
}

fn uuid(n: u8) -> Uuid {
    Uuid([n; 16])
}

#[test]
fn a_newtype_has_exactly_the_bytes_of_its_inner_type() {
    let id = UserId(uuid(7));
    assert_eq!(id.encode_to_vec(), uuid(7).encode_to_vec());
    assert_eq!(Meters(1.5).encode_to_vec(), 1.5_f64.encode_to_vec());
    assert_eq!(Owner(id).encode_to_vec(), id.encode_to_vec());
    assert_eq!(
        Tags(vec!["a".into(), "bc".into()]).encode_to_vec(),
        vec!["a".to_owned(), "bc".to_owned()].encode_to_vec()
    );
    assert_eq!(
        Price(Decimal::new(1999, 2)).encode_to_vec(),
        Decimal::new(1999, 2).encode_to_vec()
    );
    assert_eq!(Sku("x-1".into()).encode_to_vec(), "x-1".encode_to_vec());
}

#[test]
fn a_newtype_round_trips_and_its_minimum_length_is_the_inner_types() {
    assert_eq!(<UserId as Decode>::MIN_ENCODED_LEN, 16);
    assert_eq!(<Meters as Decode>::MIN_ENCODED_LEN, 8);
    assert_eq!(<Owner as Decode>::MIN_ENCODED_LEN, 16);
    assert_eq!(<Tags as Decode>::MIN_ENCODED_LEN, 4);
    assert_eq!(<Price as Decode>::MIN_ENCODED_LEN, 17);
    assert_eq!(<Sku as Decode>::MIN_ENCODED_LEN, 4);
    let order = Order {
        id: UserId(uuid(1)),
        tags: Tags(vec!["rush".into()]),
        total: Price(Decimal::new(-50, 1)),
        by_sku: HashMap::from([(Sku("a".into()), 2), (Sku("b".into()), 3)]),
        owners: BTreeMap::from([(
            Owner(UserId(uuid(9))),
            vec![UserId(uuid(2)), UserId(uuid(3))],
        )]),
        height: Some(Meters(1.82)),
    };
    let bytes = order.encode_to_vec();
    assert_eq!(Order::decode_exact(&bytes).unwrap(), order);
    // A hand-written record of the inner types is byte for byte the same.
    let by_hand = {
        let mut w = Writer::new();
        uuid(1).encode(&mut w);
        vec!["rush".to_owned()].encode(&mut w);
        Decimal::new(-50, 1).encode(&mut w);
        HashMap::from([("a".to_owned(), 2_u32), ("b".to_owned(), 3)]).encode(&mut w);
        BTreeMap::from([(uuid(9), vec![uuid(2), uuid(3)])]).encode(&mut w);
        Some(1.82_f64).encode(&mut w);
        w.into_vec()
    };
    assert_eq!(bytes, by_hand);
}

#[test]
fn hostile_input_to_a_newtype_is_a_typed_error() {
    // Truncated.
    assert!(matches!(
        UserId::decode_exact(&[1; 15]),
        Err(WireError::UnexpectedEof { .. })
    ));
    // A count a `Vec` of newtypes cannot hold: rejected before anything is allocated.
    let mut w = Writer::new();
    w.write_u32(u32::MAX);
    assert!(Vec::<UserId>::decode_exact(w.as_slice()).is_err());
    // A decimal scale past the limit is the inner type's error, not a panic.
    let mut bad = Decimal::new(1, 0).encode_to_vec();
    bad[16] = 39;
    assert!(Price::decode_exact(&bad).is_err());
}

#[test]
fn a_newtype_is_a_transparent_record_of_one_field_called_value() {
    let def = record("UserId");
    assert!(def.transparent);
    assert_eq!(def.type_id, ids::type_id("UserId"));
    assert_eq!(def.docs, "A user's id.");
    assert_eq!(def.fields.len(), 1);
    assert_eq!(def.fields[0].name, "value");
    assert_eq!(def.fields[0].ty, TypeRef::Uuid);
    assert!(!def.fields[0].default);
    // Any field visibility, any inner value type.
    assert_eq!(record("Meters").fields[0].ty, TypeRef::F64);
    assert_eq!(record("Owner").fields[0].ty, named("UserId"));
    assert_eq!(
        record("Tags").fields[0].ty,
        TypeRef::Vec(Box::new(TypeRef::String))
    );
    assert_eq!(record("Price").fields[0].ty, TypeRef::Decimal);
    // An ordinary record is not transparent.
    assert!(!record("Order").transparent);
}

#[test]
fn the_schema_with_newtypes_validates_and_resolves_newtype_keys() {
    let schema = schema();
    schema.validate().expect("the schema is valid");
    // `HashMap<Sku, u32>` and `BTreeMap<Owner, _>` are maps keyed by newtypes of keys.
    let order = record("Order");
    let by_sku = &order.fields[3].ty;
    assert_eq!(
        *by_sku,
        TypeRef::Map(Box::new(named("Sku")), Box::new(TypeRef::U32))
    );
    assert!(schema.is_valid_map_key(&named("Sku")));
    assert!(schema.is_valid_map_key(&named("Owner")));
    assert!(schema.is_valid_map_key(&named("UserId")));
    // `Price` wraps a `Decimal`, and a record is no key at all.
    assert!(!schema.is_valid_map_key(&named("Price")));
    assert!(!schema.is_valid_map_key(&named("Order")));
    assert!(!schema.is_valid_map_key(&TypeRef::Decimal));
}

// ---------------------------------------------------------------------------------------------
// A keyed list whose key field is a newtype
// ---------------------------------------------------------------------------------------------

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub id: UserId,
    pub name: String,
}

#[k::store]
pub struct Roster {
    ctx: Ctx,
    #[undra(key = "id")]
    members: Signal<Vec<Member>>,
}

#[k::api(store)]
impl Roster {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            members: Signal::new(Vec::new()),
        }
    }

    /// A newtype crosses as a parameter.
    pub fn add(&self, id: UserId, name: String) {
        self.members.update(|m| m.push(Member { id, name }));
    }

    pub fn rename(&self, id: UserId, name: String) {
        self.members.update(|m| {
            for member in m.iter_mut().filter(|m| m.id == id) {
                member.name.clone_from(&name);
            }
        });
    }

    /// A newtype crosses as a return, and a map keyed by one as a parameter.
    pub fn first(&self) -> Option<UserId> {
        self.members.get().first().map(|m| m.id)
    }

    pub fn count_by(&self, counts: HashMap<UserId, u32>) -> u32 {
        counts.values().sum()
    }

    pub fn has_ctx(&self) -> bool {
        let _ = self.ctx.clone();
        true
    }
}

fn roster_call(rt: &Runtime, handle: u64, method: &str, args: &[u8]) -> Vec<u8> {
    rt.call_object("Roster", method, handle, args).sync_ok()
}

fn member_patch(cs: &undra::wire::payload::ChangeSet) -> KeyedPatch<Member> {
    let entry = &cs.entries[0];
    assert_eq!(entry.op, ChangeOp::KeyedPatch, "{cs:?}");
    let mut r = Reader::new(&entry.value);
    let patch = KeyedPatch::<Member>::decode(&mut r).unwrap();
    r.finish().unwrap();
    patch
}

#[test]
fn a_keyed_list_matches_its_items_by_a_newtype_key_field() {
    let rt = Runtime::new();
    let reply = rt.call_object("Roster", "new", 0, &[]).sync_ok();
    let handle = Handle::decode_exact(&reply).unwrap().0;
    rt.real().observe(handle, 0, true);
    rt.change_sets();

    let args = |id: u8, name: &str| {
        let mut w = Writer::new();
        UserId(uuid(id)).encode(&mut w);
        name.to_owned().encode(&mut w);
        w.into_vec()
    };
    roster_call(&rt, handle, "add", &args(1, "ada"));
    assert_eq!(rt.change_sets()[0].entries[0].op, ChangeOp::Full);
    roster_call(&rt, handle, "add", &args(2, "grace"));
    assert_eq!(
        member_patch(&rt.change_sets()[0]).ops,
        [PatchOp::Insert {
            index: 1,
            item: Member {
                id: UserId(uuid(2)),
                name: "grace".into()
            }
        }]
    );
    // Renaming changes a field that is not the key: an update of the same item.
    roster_call(&rt, handle, "rename", &args(1, "ada l."));
    assert_eq!(
        member_patch(&rt.change_sets()[0]).ops,
        [PatchOp::Update {
            index: 0,
            item: Member {
                id: UserId(uuid(1)),
                name: "ada l.".into()
            }
        }]
    );
    // A newtype comes back as its inner bytes, and a map keyed by one goes in.
    let first = roster_call(&rt, handle, "first", &[]);
    assert_eq!(
        Option::<UserId>::decode_exact(&first).unwrap(),
        Some(UserId(uuid(1)))
    );
    let counts = HashMap::from([(UserId(uuid(1)), 2_u32), (UserId(uuid(2)), 5)]);
    let total = roster_call(&rt, handle, "count_by", &counts.encode_to_vec());
    assert_eq!(u32::decode_exact(&total).unwrap(), 7);
    // The store's signal in the schema is a keyed list of records whose key is a newtype.
    let roster = schema()
        .objects
        .into_iter()
        .find(|o| o.name == "Roster")
        .unwrap();
    let signals = roster.store.unwrap().signals;
    assert_eq!(signals[0].key.as_deref(), Some("id"));
    assert_eq!(record("Member").fields[0].ty, named("UserId"));
}

// ---------------------------------------------------------------------------------------------
// Generic data types
// ---------------------------------------------------------------------------------------------

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: UserId,
    pub title: String,
}

/// A page of rows (the template: it registers nothing).
#[k::api(generic)]
#[derive(Clone, Debug, PartialEq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    #[undra(default)]
    pub next: Option<String>,
    pub by_name: HashMap<String, Vec<Option<T>>>,
}

/// A page of todos.
#[k::api]
pub type TodoPage = Page<Todo>;

#[k::api]
pub type MemberPage = Page<Member>;

/// A page of ids: the argument is a newtype.
#[k::api]
pub type IdPage = Page<UserId>;

/// A page of pages: the argument is another instantiation.
#[k::api]
pub type TodoPages = Page<TodoPage>;

#[k::api(generic)]
#[derive(Clone, Debug, PartialEq)]
pub enum Loadable<T, E> {
    Loading,
    Loaded(T),
    Failed {
        error: E,
        retry_after: Option<std::time::Duration>,
    },
}

#[k::api]
pub type LoadableTodos = Loadable<Vec<Todo>, String>;

/// A generic newtype: an instantiation is a transparent record too.
#[k::api(generic)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tagged<T>(pub T);

#[k::api]
pub type TaggedSku = Tagged<Sku>;

#[k::api]
pub type TaggedPrice = Tagged<Price>;

/// What a hand-written record with the same fields looks like on the wire.
#[k::api]
pub struct HandPage {
    pub items: Vec<Todo>,
    pub next: Option<String>,
    pub by_name: HashMap<String, Vec<Option<Todo>>>,
}

fn todo(n: u8) -> Todo {
    Todo {
        id: UserId(uuid(n)),
        title: format!("todo {n}"),
    }
}

fn page_of_todos() -> TodoPage {
    Page {
        items: vec![todo(1), todo(2)],
        next: Some("cursor-2".into()),
        by_name: HashMap::from([("a".to_owned(), vec![Some(todo(3)), None])]),
    }
}

#[test]
fn an_instantiation_is_byte_for_byte_a_hand_written_record() {
    let page = page_of_todos();
    let by_hand = HandPage {
        items: page.items.clone(),
        next: page.next.clone(),
        by_name: page.by_name.clone(),
    };
    assert_eq!(page.encode_to_vec(), by_hand.encode_to_vec());
    assert_eq!(TodoPage::decode_exact(&page.encode_to_vec()).unwrap(), page);
    // Two instantiations of one template are two types with one wire layout.
    let members = MemberPage {
        items: vec![Member {
            id: UserId(uuid(1)),
            name: "ada".into(),
        }],
        next: None,
        by_name: HashMap::new(),
    };
    assert_eq!(
        MemberPage::decode_exact(&members.encode_to_vec()).unwrap(),
        members
    );
    // The argument may be another instantiation.
    let pages = TodoPages {
        items: vec![page_of_todos()],
        next: None,
        by_name: HashMap::new(),
    };
    assert_eq!(
        TodoPages::decode_exact(&pages.encode_to_vec()).unwrap(),
        pages
    );
    // A generic enum instantiation round-trips every variant.
    for value in [
        LoadableTodos::Loading,
        LoadableTodos::Loaded(vec![todo(1)]),
        LoadableTodos::Failed {
            error: "boom".into(),
            retry_after: Some(std::time::Duration::from_secs(3)),
        },
    ] {
        assert_eq!(
            LoadableTodos::decode_exact(&value.encode_to_vec()).unwrap(),
            value
        );
    }
    // An unknown variant is a typed error.
    assert!(matches!(
        LoadableTodos::decode_exact(&9_u16.to_le_bytes()),
        Err(WireError::InvalidTag { .. })
    ));
    // The minimum length of an instantiation is the template's field walk.
    assert_eq!(<TodoPage as Decode>::MIN_ENCODED_LEN, 4 + 1 + 4);
    assert_eq!(<LoadableTodos as Decode>::MIN_ENCODED_LEN, 2);
}

#[test]
fn an_instantiation_registers_under_the_alias_and_the_template_registers_nothing() {
    let schema = schema();
    assert!(
        schema.records.iter().all(|r| r.name != "Page"),
        "a template has no schema entry"
    );
    assert!(schema.enums.iter().all(|e| e.name != "Loadable"));
    assert!(schema.records.iter().all(|r| r.name != "Tagged"));

    let def = record("TodoPage");
    assert_eq!(def.type_id, ids::type_id("TodoPage"));
    assert_eq!(def.docs, "A page of todos.");
    assert!(!def.transparent);
    let fields: Vec<(&str, &TypeRef, bool, &str)> = def
        .fields
        .iter()
        .map(|f| (f.name.as_str(), &f.ty, f.default, f.docs.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            (
                "items",
                &TypeRef::Vec(Box::new(named("Todo"))),
                false,
                "The rows."
            ),
            (
                "next",
                &TypeRef::Option(Box::new(TypeRef::String)),
                true,
                ""
            ),
            (
                "by_name",
                &TypeRef::Map(
                    Box::new(TypeRef::String),
                    Box::new(TypeRef::Vec(Box::new(TypeRef::Option(Box::new(named(
                        "Todo"
                    ))))))
                ),
                false,
                ""
            ),
        ]
    );
    // The other instantiations substitute their own argument, nested ones included.
    assert_eq!(
        record("IdPage").fields[0].ty,
        TypeRef::Vec(Box::new(named("UserId")))
    );
    assert_eq!(
        record("TodoPages").fields[0].ty,
        TypeRef::Vec(Box::new(named("TodoPage")))
    );
    // The record is what a hand-written one is: same fields, other name and id.
    let hand = record("HandPage");
    assert_eq!(
        hand.fields
            .iter()
            .map(|f| (&f.name, &f.ty))
            .collect::<Vec<_>>(),
        def.fields
            .iter()
            .map(|f| (&f.name, &f.ty))
            .collect::<Vec<_>>()
    );

    let loadable = schema
        .enums
        .iter()
        .find(|e| e.name == "LoadableTodos")
        .unwrap();
    assert!(!loadable.is_error);
    assert_eq!(loadable.type_id, ids::type_id("LoadableTodos"));
    assert_eq!(
        loadable.variants[1].fields[0].ty,
        TypeRef::Vec(Box::new(named("Todo")))
    );
    assert_eq!(loadable.variants[2].fields[0].ty, TypeRef::String);
    assert_eq!(loadable.variants[2].fields[0].name, "error");

    // A generic newtype instantiates as a transparent record, a key when its argument is one.
    let sku = record("TaggedSku");
    assert!(sku.transparent);
    assert_eq!(sku.fields[0].ty, named("Sku"));
    assert!(schema.is_valid_map_key(&named("TaggedSku")));
    assert!(!schema.is_valid_map_key(&named("TaggedPrice")));
    schema
        .validate()
        .expect("the schema with the instantiations is valid");
}

#[test]
fn the_generic_newtype_is_a_map_key_exactly_when_its_argument_is() {
    fn is_key<K: undra::wire::leaf::MapKey>() {}
    is_key::<TaggedSku>();
    is_key::<UserId>();
    is_key::<Owner>();
    is_key::<Sku>();
}

// ---------------------------------------------------------------------------------------------
// Signatures spell the alias
// ---------------------------------------------------------------------------------------------

#[k::api]
pub fn todo_page(count: u32) -> TodoPage {
    Page {
        items: (0..u8::try_from(count).unwrap_or(0)).map(todo).collect(),
        next: None,
        by_name: HashMap::new(),
    }
}

#[k::api]
pub fn sum_prices(prices: HashMap<Sku, Price>) -> Price {
    let sum: i128 = prices.values().map(|p| p.0.mantissa).sum();
    Price(Decimal::new(sum, 2))
}

#[test]
fn a_signature_spelling_an_alias_dispatches_and_its_types_are_in_the_schema() {
    let rt = Runtime::new();
    let reply = rt
        .call_function("todo_page", &3_u32.encode_to_vec())
        .sync_ok();
    assert_eq!(TodoPage::decode_exact(&reply).unwrap().items.len(), 3);
    let prices = HashMap::from([
        (Sku("a".into()), Price(Decimal::new(150, 2))),
        (Sku("b".into()), Price(Decimal::new(250, 2))),
    ]);
    let reply = rt
        .call_function("sum_prices", &prices.encode_to_vec())
        .sync_ok();
    assert_eq!(
        Price::decode_exact(&reply).unwrap(),
        Price(Decimal::new(400, 2))
    );

    let schema = schema();
    let function = schema
        .functions
        .iter()
        .find(|f| f.name == "todo_page")
        .unwrap();
    assert_eq!(function.returns, named("TodoPage"));
    let function = schema
        .functions
        .iter()
        .find(|f| f.name == "sum_prices")
        .unwrap();
    assert_eq!(
        function.params[0].ty,
        TypeRef::Map(Box::new(named("Sku")), Box::new(named("Price")))
    );
    assert_eq!(function.returns, named("Price"));
}

// ---------------------------------------------------------------------------------------------
// `Decimal` fields
// ---------------------------------------------------------------------------------------------

#[k::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invoice {
    pub subtotal: Decimal,
    pub tax: Option<Decimal>,
    pub lines: Vec<Decimal>,
}

#[test]
fn decimal_fields_round_trip_exactly_and_are_a_leaf_in_the_schema() {
    let invoice = Invoice {
        subtotal: Decimal::new(i128::MIN, 38),
        tax: Some(Decimal::new(-1, 0)),
        lines: vec![Decimal::new(100, 2), Decimal::new(10, 1)],
    };
    let bytes = invoice.encode_to_vec();
    let decoded = Invoice::decode_exact(&bytes).unwrap();
    // Not normalised: `1.00` and `1.0` stay distinct.
    assert_eq!(decoded, invoice);
    assert_ne!(decoded.lines[0], decoded.lines[1]);
    assert!(decoded.lines[0].eq_numeric(&decoded.lines[1]));
    let def = record("Invoice");
    assert_eq!(def.fields[0].ty, TypeRef::Decimal);
    assert_eq!(
        def.fields[1].ty,
        TypeRef::Option(Box::new(TypeRef::Decimal))
    );
    assert_eq!(def.fields[2].ty, TypeRef::Vec(Box::new(TypeRef::Decimal)));
}
