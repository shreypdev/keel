//! The ledger: newtypes, generic instantiations and leaf types (ADR-042).
//!
//! A small bookkeeping core written with the shapes ADR-042 adds, so every platform meets each of
//! them once:
//!
//! * **Newtypes**: [`AccountId`] (of a `Uuid`), [`Cents`] (of an `i64`) and [`Price`] (of a `Decimal`)
//!   cross as their inner type, with the idiomatic wrapper of each language (a Swift struct, a Kotlin
//!   value class, a branded TypeScript type). [`balances`] is a `HashMap<AccountId, Price>`, and the
//!   [`Ledger`] store keeps its accounts in a list keyed by an `AccountId`.
//! * **Generic instantiations**: [`Slice`] and [`Loadable`] are templates, [`EntrySlice`] and
//!   [`LoadableEntries`] the named instantiations the signatures spell.
//! * **`Decimal`** is exact money: [`deposit`] adds prices through `rust_decimal`, so `0.1 + 0.2`
//!   is `0.3`, and [`echo_decimal`] returns what it is given, scale and all.
//! * **Leaf types of other crates** (`uuid::Uuid`, `chrono::DateTime<Utc>`, `chrono::TimeDelta`,
//!   `rust_decimal::Decimal`, `bytes::Bytes`) in the [`Receipt`] record, which [`echo_receipt`] returns.
//!
//! The ledger reads no clock and no random source (R12): identities come from a counter and the time
//! of an entry is whatever the caller says it is.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use undra::prelude::*;

/// The identity of an account: a `Uuid` that is not just any `Uuid`.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccountId(pub Uuid);

/// An amount in cents: a whole number that is not just any number.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cents(pub i64);

/// A price: an exact decimal number of the currency's major unit.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price(pub Decimal);

/// One line of an account's statement.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The account the money went into.
    pub account: AccountId,
    /// How much.
    pub amount: Price,
    /// What it was for.
    pub memo: String,
    /// When the caller says it happened.
    pub at: Timestamp,
}

/// A window of rows and where the next one starts: a template, which registers nothing by itself.
#[undra::api(generic)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice<T> {
    /// The rows of this window.
    pub items: Vec<T>,
    /// The offset of the next window, if there is one.
    pub next: Option<u32>,
}

/// A value that may still be on its way: a template.
#[undra::api(generic)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loadable<T> {
    /// Nothing yet.
    Loading,
    /// Here it is.
    Loaded(T),
    /// It could not be had.
    Failed(String),
}

/// A window of [`Entry`] rows.
#[undra::api]
pub type EntrySlice = Slice<Entry>;

/// A window of entries that may still be loading.
#[undra::api]
pub type LoadableEntries = Loadable<EntrySlice>;

/// An account of the [`Ledger`] store.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Its identity; the store's list is patched by it.
    pub id: AccountId,
    /// Whose it is.
    pub owner: String,
}

/// A record of types from other crates: every field is a leaf type that crosses as a wire type.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// A `uuid::Uuid`, which crosses as `Uuid`.
    pub id: uuid::Uuid,
    /// A `chrono::DateTime<Utc>`, which crosses as `Timestamp` (milliseconds).
    pub issued: chrono::DateTime<chrono::Utc>,
    /// A `chrono::TimeDelta`, which crosses as `Duration` (nanoseconds).
    pub valid_for: chrono::TimeDelta,
    /// A `rust_decimal::Decimal`, which crosses as `Decimal`.
    pub total: rust_decimal::Decimal,
    /// A `bytes::Bytes`, which crosses as `Bytes`.
    pub signature: bytes::Bytes,
}

/// Why a ledger operation failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// No account has this identity.
    #[error("no such account")]
    NoSuchAccount,
    /// The amount cannot be added (it is not a number `rust_decimal` holds).
    #[error("the amount is out of range")]
    OutOfRange,
}

/// What the free functions of this module share in one runtime.
#[derive(Default)]
struct LedgerState {
    next: AtomicU64,
    books: Mutex<HashMap<[u8; 16], Books>>,
}

/// The money and the lines of one account.
#[derive(Default)]
struct Books {
    owner: String,
    balance: rust_decimal::Decimal,
    entries: Vec<Entry>,
}

fn state(ctx: &Ctx) -> &LedgerState {
    ctx.runtime().extension::<LedgerState>()
}

/// The identity of the `n`th account: a counter, not a random source.
fn id_of(n: u64) -> AccountId {
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&0x1ed9_e400_0000_0000_u64.to_be_bytes());
    bytes[8..].copy_from_slice(&n.to_be_bytes());
    AccountId(Uuid(bytes))
}

/// `rust_decimal`'s view of a wire decimal, through its text: no float, no rounding.
fn exact(price: Decimal) -> Result<rust_decimal::Decimal, LedgerError> {
    rust_decimal::Decimal::from_str(&price.to_string()).map_err(|_| LedgerError::OutOfRange)
}

/// The wire decimal of one of `rust_decimal`'s.
fn wire(value: rust_decimal::Decimal) -> Price {
    // A `rust_decimal::Decimal` always has a scale of at most 28, which the wire holds.
    Price(Decimal::new(value.mantissa(), value.scale() as u8))
}

/// A new account for `owner`, with an identity from the ledger's counter.
#[undra::api]
pub fn open_account(ctx: &Ctx, owner: String) -> AccountId {
    let state = state(ctx);
    let id = id_of(state.next.fetch_add(1, Ordering::Relaxed) + 1);
    state
        .books
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            id.0.0,
            Books {
                owner,
                ..Books::default()
            },
        );
    id
}

/// Adds `amount` to the balance of `account` and returns the new balance. Exact: `0.1` and `0.2`
/// make `0.3`.
#[undra::api]
pub fn deposit(
    ctx: &Ctx,
    account: AccountId,
    amount: Price,
    memo: String,
    at: Timestamp,
) -> Result<Price, LedgerError> {
    let amount_exact = exact(amount.0)?;
    let mut books = state(ctx)
        .books
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let book = books
        .get_mut(&account.0.0)
        .ok_or(LedgerError::NoSuchAccount)?;
    book.balance = book
        .balance
        .checked_add(amount_exact)
        .ok_or(LedgerError::OutOfRange)?;
    book.entries.push(Entry {
        account,
        amount,
        memo,
        at,
    });
    Ok(wire(book.balance))
}

/// Who owns `account`, or an empty string for an unknown one.
#[undra::api]
pub fn owner_of(ctx: &Ctx, account: AccountId) -> String {
    state(ctx)
        .books
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&account.0.0)
        .map(|book| book.owner.clone())
        .unwrap_or_default()
}

/// The balance of every account: a map keyed by a newtype.
#[undra::api]
pub fn balances(ctx: &Ctx) -> HashMap<AccountId, Price> {
    state(ctx)
        .books
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|(id, book)| (AccountId(Uuid(*id)), wire(book.balance)))
        .collect()
}

/// `limit` entries of the statement of `account` from `offset`: a named instantiation of [`Slice`].
#[undra::api]
pub fn statement(ctx: &Ctx, account: AccountId, offset: u32, limit: u32) -> EntrySlice {
    let books = state(ctx)
        .books
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entries = books
        .get(&account.0.0)
        .map_or(&[][..], |book| book.entries.as_slice());
    let start = (offset as usize).min(entries.len());
    let end = start.saturating_add(limit as usize).min(entries.len());
    Slice {
        items: entries[start..end].to_vec(),
        next: (end < entries.len()).then_some(end as u32),
    }
}

/// The statement of `account` as a [`Loadable`]: `Loading` when `ready` is false, the first
/// `limit` entries when it is true, `Failed` for an unknown account.
#[undra::api]
pub fn loadable_statement(
    ctx: &Ctx,
    account: AccountId,
    ready: bool,
    limit: u32,
) -> LoadableEntries {
    if !ready {
        return Loadable::Loading;
    }
    let known = state(ctx)
        .books
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(&account.0.0);
    if known {
        Loadable::Loaded(statement(ctx, account, 0, limit))
    } else {
        Loadable::Failed("no such account".to_owned())
    }
}

/// Returns `id`: a newtype of `Uuid` comes back as itself.
#[undra::api]
pub fn echo_account(id: AccountId) -> AccountId {
    id
}

/// Returns `cents`: a newtype of `i64`.
#[undra::api]
pub fn echo_cents(cents: Cents) -> Cents {
    cents
}

/// Returns `price`: a newtype of `Decimal`.
#[undra::api]
pub fn echo_price(price: Price) -> Price {
    price
}

/// Returns `value` exactly: mantissa, scale and all (`1.10` stays `1.10`).
#[undra::api]
pub fn echo_decimal(value: Decimal) -> Decimal {
    value
}

/// Returns `receipt`: every leaf type of another crate, through the core and back.
#[undra::api]
pub fn echo_receipt(receipt: Receipt) -> Receipt {
    receipt
}

/// A receipt made of fixed values (what a platform checks it decodes), so a platform can read
/// what the leaf types look like on its side without building one.
#[undra::api]
pub fn sample_receipt() -> Receipt {
    Receipt {
        id: uuid::Uuid::from_bytes(id_of(7).0.0),
        // 2026-10-01T12:00:00.123Z
        issued: chrono::DateTime::from_timestamp_millis(1_790_856_000_123)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH),
        valid_for: chrono::TimeDelta::milliseconds(90_000),
        total: rust_decimal::Decimal::from_str("19.990").unwrap_or_default(),
        signature: bytes::Bytes::from_static(&[1, 2, 3, 255]),
    }
}

/// The accounts the app lists: a list keyed by a newtype.
#[undra::store]
pub struct Ledger {
    next: AtomicU64,
    #[undra(key = "id")]
    accounts: Signal<Vec<Account>>,
}

#[undra::api(store)]
impl Ledger {
    /// A ledger with no accounts.
    pub fn new(_ctx: Ctx) -> Self {
        Self {
            next: AtomicU64::new(1_000),
            accounts: Signal::new(vec![]),
        }
    }

    /// Opens an account for `owner`; one keyed `Insert` whose key is a newtype.
    pub fn open(&self, owner: String) -> AccountId {
        let id = id_of(self.next.fetch_add(1, Ordering::Relaxed));
        self.accounts.push(Account { id, owner });
        id
    }

    /// Changes the owner of the account `id`; one keyed `Update`.
    pub fn rename(&self, id: AccountId, owner: String) {
        let at = self
            .accounts
            .with(|list| list.iter().position(|account| account.id == id));
        if let Some(at) = at {
            self.accounts.update_at(at, |account| account.owner = owner);
        }
    }

    /// Closes the account `id`; one keyed `Remove`.
    pub fn close_account(&self, id: AccountId) {
        let at = self
            .accounts
            .with(|list| list.iter().position(|account| account.id == id));
        if let Some(at) = at {
            self.accounts.remove(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra::runtime::testing::TestRuntime;
    use undra::wire::{Decode, Encode};

    fn price(text: &str) -> Price {
        Price(text.parse().unwrap())
    }

    #[test]
    fn money_is_exact_and_the_balance_is_keyed_by_a_newtype() {
        let t = TestRuntime::new();
        let ctx = t.ctx();
        let ada = open_account(&ctx, "Ada".to_owned());
        let bob = open_account(&ctx, "Bob".to_owned());
        assert_ne!(ada, bob);
        assert_eq!(owner_of(&ctx, ada), "Ada");
        let first = deposit(&ctx, ada, price("0.1"), "a".to_owned(), Timestamp(1)).unwrap();
        assert_eq!(first, price("0.1"));
        let second = deposit(&ctx, ada, price("0.2"), "b".to_owned(), Timestamp(2)).unwrap();
        // The point of a decimal type: 0.1 + 0.2 is 0.3, not 0.30000000000000004.
        assert_eq!(second.0.to_string(), "0.3");
        let all = balances(&ctx);
        assert_eq!(all.len(), 2);
        assert_eq!(all[&ada].0.to_string(), "0.3");
        assert_eq!(all[&bob].0.to_string(), "0");
        assert_eq!(
            deposit(
                &ctx,
                AccountId(Uuid([9; 16])),
                price("1"),
                String::new(),
                Timestamp(0)
            ),
            Err(LedgerError::NoSuchAccount)
        );
        // Something `rust_decimal` cannot hold (a mantissa of more than 96 bits).
        assert_eq!(
            deposit(
                &ctx,
                bob,
                Price(Decimal::new(i128::MAX, 0)),
                String::new(),
                Timestamp(0)
            ),
            Err(LedgerError::OutOfRange)
        );
    }

    #[test]
    fn statements_come_in_windows_and_a_loadable_wraps_one() {
        let t = TestRuntime::new();
        let ctx = t.ctx();
        let ada = open_account(&ctx, "Ada".to_owned());
        for n in 0..5 {
            deposit(&ctx, ada, price("1.50"), format!("#{n}"), Timestamp(n)).unwrap();
        }
        let first = statement(&ctx, ada, 0, 2);
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.next, Some(2));
        assert_eq!(first.items[0].amount, price("1.50"));
        let last = statement(&ctx, ada, 4, 10);
        assert_eq!((last.items.len(), last.next), (1, None));
        assert!(statement(&ctx, ada, 99, 1).items.is_empty());
        assert_eq!(loadable_statement(&ctx, ada, false, 3), Loadable::Loading);
        match loadable_statement(&ctx, ada, true, 3) {
            Loadable::Loaded(slice) => assert_eq!(slice.items.len(), 3),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            loadable_statement(&ctx, AccountId(Uuid([9; 16])), true, 3),
            Loadable::Failed("no such account".to_owned())
        );
    }

    #[test]
    fn a_newtype_has_the_bytes_of_its_inner_type() {
        let id = AccountId(Uuid([7; 16]));
        assert_eq!(id.encode_to_vec(), Uuid([7; 16]).encode_to_vec());
        assert_eq!(Cents(-5).encode_to_vec(), (-5_i64).encode_to_vec());
        let d = Decimal::new(-150, 2);
        assert_eq!(Price(d).encode_to_vec(), d.encode_to_vec());
        assert_eq!(AccountId::decode_exact(&id.encode_to_vec()), Ok(id));
        // A decimal keeps its scale through the core: `echo` is the identity on bytes.
        let wide = Decimal::new(i128::MAX, 38);
        assert_eq!(echo_decimal(wide).encode_to_vec(), wide.encode_to_vec());
        assert_ne!(Decimal::new(10, 1), Decimal::new(100, 2));
    }

    #[test]
    fn leaf_types_of_other_crates_cross_as_the_wire_types() {
        let receipt = sample_receipt();
        let bytes = receipt.encode_to_vec();
        // uuid (16) + timestamp (8) + duration (8) + decimal (17) + bytes (4 + 4).
        assert_eq!(bytes.len(), 16 + 8 + 8 + 17 + 8);
        assert_eq!(Receipt::decode_exact(&bytes), Ok(receipt.clone()));
        assert_eq!(&bytes[16..24], &1_790_856_000_123_i64.to_le_bytes());
        assert_eq!(&bytes[24..32], &90_000_000_000_i64.to_le_bytes());
        assert_eq!(echo_receipt(receipt.clone()), receipt);
    }

    #[test]
    fn the_schema_has_the_instantiations_and_not_the_templates() {
        let schema = undra::meta::collect_schema("playground-core");
        let record = |name: &str| schema.records.iter().find(|r| r.name == name);
        for newtype in ["AccountId", "Cents", "Price"] {
            let r = record(newtype).unwrap_or_else(|| panic!("{newtype}"));
            assert!(r.transparent, "{newtype}");
            assert_eq!(r.fields.len(), 1);
            assert_eq!(r.fields[0].name, "value");
        }
        let slice = record("EntrySlice").expect("the instantiation is a record");
        assert!(!slice.transparent);
        assert_eq!(
            slice
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["items", "next"]
        );
        assert!(schema.enums.iter().any(|e| e.name == "LoadableEntries"));
        assert!(record("Slice").is_none() && schema.enums.iter().all(|e| e.name != "Loadable"));
        let receipt = record("Receipt").unwrap();
        let kinds: Vec<String> = receipt.fields.iter().map(|f| f.ty.to_string()).collect();
        assert_eq!(kinds, ["uuid", "timestamp", "duration", "decimal", "bytes"]);
        schema.validate().expect("the playground schema is valid");
    }

    #[test]
    fn the_store_keys_its_accounts_by_a_newtype() {
        let t = TestRuntime::new();
        let ledger = Ledger::new(t.ctx());
        let a = ledger.open("Ada".to_owned());
        let b = ledger.open("Bob".to_owned());
        ledger.rename(a, "Ada L.".to_owned());
        ledger.close_account(b);
        let names: Vec<String> = ledger
            .accounts
            .with(|l| l.iter().map(|x| x.owner.clone()).collect());
        assert_eq!(names, ["Ada L."]);
    }
}
