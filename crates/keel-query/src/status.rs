//! [`QueryStatus`]: where a query is in its fetch lifecycle.

use keel_wire::{Decode, Encode, Reader, WireError, Writer};

/// Where a query is in its fetch lifecycle: the value of a handle's `status` signal (1).
///
/// On the wire it is a unit enum, a `u16` variant index, exactly what `keel-bindgen`'s
/// synthesized `QueryStatus` decodes in Swift, Kotlin and TypeScript.
///
/// The status is derived from the cache entry, never stored:
///
/// | Status | When |
/// |---|---|
/// | [`Idle`](QueryStatus::Idle) | nothing was fetched, nothing is in flight |
/// | [`Fetching`](QueryStatus::Fetching) | a fetch is in flight and there is no data to show yet |
/// | [`Success`](QueryStatus::Success) | there is data and the latest fetch did not fail |
/// | [`Error`](QueryStatus::Error) | the latest fetch failed (stale data, if any, is still in `data`) |
///
/// A refetch of an entry that already has data keeps `Success` while `fetching` (signal 3)
/// is `true`: a screen shows its content and a subtle refresh indicator instead of a spinner.
///
/// ```
/// use keel_query::QueryStatus;
/// use keel_wire::{Decode, Encode};
///
/// assert_eq!(QueryStatus::Success.encode_to_vec(), [2, 0]);
/// assert_eq!(QueryStatus::decode_exact(&[3, 0]), Ok(QueryStatus::Error));
/// assert!(QueryStatus::decode_exact(&[4, 0]).is_err());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QueryStatus {
    /// No fetch has started and nothing is cached.
    #[default]
    Idle,
    /// A fetch is in flight and there is no data yet.
    Fetching,
    /// The entry has data and the latest fetch succeeded.
    Success,
    /// The latest fetch failed.
    Error,
}

impl Encode for QueryStatus {
    fn encode(&self, w: &mut Writer) {
        w.write_u16(*self as u16);
    }
}

impl Decode for QueryStatus {
    const MIN_ENCODED_LEN: usize = 2;

    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let at = r.position();
        match r.read_u16()? {
            0 => Ok(QueryStatus::Idle),
            1 => Ok(QueryStatus::Fetching),
            2 => Ok(QueryStatus::Success),
            3 => Ok(QueryStatus::Error),
            tag => Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at,
                ty: "QueryStatus",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_are_the_bindgen_enum_order() {
        for (status, index) in [
            (QueryStatus::Idle, 0_u16),
            (QueryStatus::Fetching, 1),
            (QueryStatus::Success, 2),
            (QueryStatus::Error, 3),
        ] {
            assert_eq!(status.encode_to_vec(), index.to_le_bytes());
            assert_eq!(QueryStatus::decode_exact(&index.to_le_bytes()), Ok(status));
        }
    }

    #[test]
    fn bad_tags_and_truncated_input_are_errors_not_panics() {
        assert!(matches!(
            QueryStatus::decode_exact(&[9, 0]),
            Err(WireError::InvalidTag {
                tag: 9,
                ty: "QueryStatus",
                ..
            })
        ));
        assert!(QueryStatus::decode_exact(&[1]).is_err());
        assert!(QueryStatus::decode_exact(&[1, 0, 0]).is_err());
    }
}
