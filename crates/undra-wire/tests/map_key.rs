//! The hidden `MapKey` marker (ADR-042): which types may be map keys, checked by the compiler
//! at every key position of a `#[undra::api]` type. A negative case (a type that is not a key
//! is E0006) is a compile-fail test of `undra-macros`.

use undra_wire::Uuid;
use undra_wire::leaf::{MapKey, Newtype};

fn is_key<K: MapKey + ?Sized>() {}

struct UserId(#[allow(dead_code)] Uuid);
impl Newtype for UserId {
    type Inner = Uuid;
}

struct Wrapped(#[allow(dead_code)] UserId);
impl Newtype for Wrapped {
    type Inner = UserId;
}

#[test]
fn the_scalars_the_schema_allows_are_keys() {
    is_key::<String>();
    is_key::<bool>();
    is_key::<i8>();
    is_key::<i16>();
    is_key::<i32>();
    is_key::<i64>();
    is_key::<u8>();
    is_key::<u16>();
    is_key::<u32>();
    is_key::<u64>();
    is_key::<Uuid>();
}

#[test]
fn a_newtype_of_a_key_is_a_key_at_any_depth() {
    is_key::<UserId>();
    is_key::<Wrapped>();
}

#[cfg(feature = "uuid")]
#[test]
fn the_uuid_crate_type_is_a_key_and_a_newtype_of_it_too() {
    struct Id(#[allow(dead_code)] uuid::Uuid);
    impl Newtype for Id {
        type Inner = uuid::Uuid;
    }
    is_key::<uuid::Uuid>();
    is_key::<Id>();
}
