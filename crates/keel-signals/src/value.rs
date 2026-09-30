//! Bounds on what a signal can hold.

use keel_wire::Encode;

/// A type that can live in a [`Signal`](crate::Signal): encodable to the wire, cloneable,
/// shareable across threads and free of borrowed data.
///
/// Implemented for every type that meets the bounds, so there is nothing to implement by hand.
pub trait SignalValue: Encode + Clone + Send + Sync + 'static {}

impl<T: Encode + Clone + Send + Sync + 'static> SignalValue for T {}

/// A signal value that is a list of items, which makes it eligible for keyed patches
/// (SPEC 3.8).
///
/// Implemented for `Vec<I>`.
pub trait ListLike {
    /// The element type.
    type Item: SignalValue;

    /// The elements, in order.
    fn items(&self) -> &[Self::Item];
}

impl<I: SignalValue> ListLike for Vec<I> {
    type Item = I;

    fn items(&self) -> &[I] {
        self
    }
}

/// Maps a list item to the `u64` key that identifies it across updates.
///
/// Generated code hashes the encoded key field (`fnv1a64`), so equal keys have equal `u64`s.
/// Keys must be unique within a list; when they are not, a commit that has to diff the list
/// (one written with `set`, `update` or `replace`) falls back to sending the full value. A list
/// written with the recorded operations (`push`, `insert`, `update_at`, ..) is sent by position
/// and never looks at keys, so it does not notice.
pub type KeyFn<T> = fn(&<T as ListLike>::Item) -> u64;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_value<T: SignalValue>() {}

    #[test]
    fn common_types_are_signal_values() {
        assert_value::<i32>();
        assert_value::<String>();
        assert_value::<Vec<String>>();
        assert_value::<Option<u8>>();
        assert_value::<(u8, bool)>();
    }

    #[test]
    fn vec_is_list_like() {
        let v = vec![1_u32, 2, 3];
        assert_eq!(ListLike::items(&v), &[1, 2, 3]);
    }
}
