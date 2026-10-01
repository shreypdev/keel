//! Keyed list patches (SPEC 3.8): a compact way to update a `Signal<Vec<T>>` on the host
//! without resending the whole list.
//!
//! A [`KeyedPatch`] is a sequence of [`PatchOp`]s applied one after another; each index refers
//! to the list as it is *after the previous op*. The core builds patches with
//! [`KeyedPatch::diff`], the host replays them with [`KeyedPatch::apply`].
//!
//! # Example
//!
//! ```
//! use undra_wire::KeyedPatch;
//!
//! #[derive(Clone, Debug, PartialEq)]
//! struct Todo { id: u32, done: bool }
//! let t = |id, done| Todo { id, done };
//!
//! let old = vec![t(1, false), t(2, false), t(3, false)];
//! let new = vec![t(3, false), t(1, true), t(4, false)];
//!
//! let patch = KeyedPatch::diff(&old, &new, |t| t.id, |a, b| a == b).expect("small change");
//!
//! let mut list = old.clone();
//! patch.apply(&mut list).unwrap();
//! assert_eq!(list, new);
//! ```

use core::fmt;
use std::collections::HashMap;
use std::hash::Hash;

use crate::writer::len_u32;
use crate::{Decode, Encode, Reader, WireError, Writer};

/// Tags of the ops on the wire.
const OP_INSERT: u8 = 0;
const OP_REMOVE: u8 = 1;
const OP_UPDATE: u8 = 2;
const OP_MOVE: u8 = 3;
const OP_CLEAR: u8 = 4;

/// One step of a [`KeyedPatch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchOp<T> {
    /// Insert `item` so that it ends up at `index` (`index <= len`; `len` appends).
    Insert {
        /// Position the new item will occupy.
        index: u32,
        /// The item.
        item: T,
    },
    /// Remove the item at `index` (`index < len`).
    Remove {
        /// Position of the item to remove.
        index: u32,
    },
    /// Replace the item at `index` (`index < len`).
    Update {
        /// Position of the item to replace.
        index: u32,
        /// The new item.
        item: T,
    },
    /// Take the item at `from` out of the list and put it back so that it ends up at `to`
    /// (both `< len`; `to` is an index in the list *after* the removal, which is also the
    /// final position of the item). `from == to` is a no-op.
    Move {
        /// Current position of the item.
        from: u32,
        /// Final position of the item.
        to: u32,
    },
    /// Remove every item.
    Clear,
}

/// A sequence of [`PatchOp`]s.
///
/// Wire layout: `count u32, count x { op u8, ... }` with `0 Insert { index u32, item T }`,
/// `1 Remove { index u32 }`, `2 Update { index u32, item T }`, `3 Move { from u32, to u32 }`
/// and `4 Clear {}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyedPatch<T> {
    /// The operations, applied in order.
    pub ops: Vec<PatchOp<T>>,
}

impl<T> Default for KeyedPatch<T> {
    fn default() -> Self {
        KeyedPatch { ops: Vec::new() }
    }
}

/// A patch op referred to a position outside the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchError {
    /// `index` is out of range for a list of length `len` at the point op number `op` runs.
    OutOfBounds {
        /// Zero-based position of the failing op within the patch.
        op: usize,
        /// The offending index (for `Move`, whichever of `from` and `to` is out of range).
        index: u32,
        /// The list length when the op ran.
        len: usize,
    },
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            PatchError::OutOfBounds { op, index, len } => write!(
                f,
                "patch op #{op} uses index {index} but the list has {len} item(s)"
            ),
        }
    }
}

impl std::error::Error for PatchError {}

impl<T> KeyedPatch<T> {
    /// Returns `true` if the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Number of ops.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Applies the ops in order to `list`.
    ///
    /// The patch is validated against the list length first, so on error `list` is left
    /// untouched. Items are cloned out of the patch.
    ///
    /// # Errors
    ///
    /// [`PatchError::OutOfBounds`] if any op refers to a position outside the list.
    pub fn apply(&self, list: &mut Vec<T>) -> Result<(), PatchError>
    where
        T: Clone,
    {
        // Bounds depend only on the length, which each op changes predictably, so validate
        // everything up front and keep the application pass infallible.
        let mut len = list.len();
        for (op_no, op) in self.ops.iter().enumerate() {
            let out_of_bounds = |index: u32| PatchError::OutOfBounds {
                op: op_no,
                index,
                len,
            };
            match *op {
                PatchOp::Insert { index, .. } => {
                    if index as usize > len {
                        return Err(out_of_bounds(index));
                    }
                    len += 1;
                }
                PatchOp::Remove { index } => {
                    if index as usize >= len {
                        return Err(out_of_bounds(index));
                    }
                    len -= 1;
                }
                PatchOp::Update { index, .. } => {
                    if index as usize >= len {
                        return Err(out_of_bounds(index));
                    }
                }
                PatchOp::Move { from, to } => {
                    if from as usize >= len {
                        return Err(out_of_bounds(from));
                    }
                    if to as usize >= len {
                        return Err(out_of_bounds(to));
                    }
                }
                PatchOp::Clear => len = 0,
            }
        }

        for op in &self.ops {
            match op {
                PatchOp::Insert { index, item } => {
                    let index = *index as usize;
                    if index <= list.len() {
                        list.insert(index, item.clone());
                    }
                }
                PatchOp::Remove { index } => {
                    let index = *index as usize;
                    if index < list.len() {
                        list.remove(index);
                    }
                }
                PatchOp::Update { index, item } => {
                    if let Some(slot) = list.get_mut(*index as usize) {
                        *slot = item.clone();
                    }
                }
                PatchOp::Move { from, to } => {
                    let (from, to) = (*from as usize, *to as usize);
                    if from < list.len() && to < list.len() {
                        let item = list.remove(from);
                        list.insert(to, item);
                    }
                }
                PatchOp::Clear => list.clear(),
            }
        }
        Ok(())
    }

    /// Computes the patch that turns `old` into `new`, or `None` when the caller should send
    /// the full value instead.
    ///
    /// Items are matched by `key`; `eq` decides whether two items with the same key are equal
    /// (pass `PartialEq::eq`, or compare encoded bytes as the core does). The patch consists of:
    ///
    /// 1. `Remove` for every old item whose key is gone, from the highest index down;
    /// 2. `Update` for every surviving item that changed;
    /// 3. `Insert` for every new key and `Move` for every surviving item that has to change
    ///    its relative order. Items that already form the longest run in the right order stay
    ///    where they are, so the number of moves is minimal for the removal/insertion pattern.
    ///
    /// Returns `None` (send the full value) when
    ///
    /// * more than 50% of the old items are removed,
    /// * the lists share no key (including when either is empty but the other is not), or
    /// * a key occurs more than once in either list, since matching is then ambiguous.
    ///
    /// Two identical lists give `Some` of an empty patch, see [`KeyedPatch::is_empty`].
    ///
    /// Cost: O(n) hashing plus O(n log n) for the order analysis; each move or insert in the
    /// middle of a list additionally costs a linear scan of the working list.
    pub fn diff<K, F, E>(old: &[T], new: &[T], key: F, eq: E) -> Option<KeyedPatch<T>>
    where
        T: Clone,
        K: Eq + Hash,
        F: Fn(&T) -> K,
        E: Fn(&T, &T) -> bool,
    {
        let (n_old, n_new) = (old.len(), new.len());
        if u32::try_from(n_old).is_err() || u32::try_from(n_new).is_err() {
            return None;
        }

        let old_keys: Vec<K> = old.iter().map(&key).collect();
        let new_keys: Vec<K> = new.iter().map(&key).collect();

        // Key -> position, rejecting duplicate keys.
        let mut old_pos: HashMap<&K, usize> = HashMap::with_capacity(n_old);
        for (i, k) in old_keys.iter().enumerate() {
            if old_pos.insert(k, i).is_some() {
                return None;
            }
        }
        let mut new_pos: HashMap<&K, usize> = HashMap::with_capacity(n_new);
        for (j, k) in new_keys.iter().enumerate() {
            if new_pos.insert(k, j).is_some() {
                return None;
            }
        }

        // For each old item, where it lands in `new` (None = removed); and the reverse.
        let old_to_new: Vec<Option<usize>> =
            old_keys.iter().map(|k| new_pos.get(k).copied()).collect();
        let new_to_old: Vec<Option<usize>> =
            new_keys.iter().map(|k| old_pos.get(k).copied()).collect();
        let kept = old_to_new.iter().flatten().count();
        let removed = n_old - kept;

        if removed * 2 > n_old || (kept == 0 && n_old + n_new > 0) {
            return None;
        }

        let mut ops: Vec<PatchOp<T>> = Vec::new();

        // 1. Removals, highest index first so that lower indices stay valid.
        for i in (0..n_old).rev() {
            if old_to_new[i].is_none() {
                ops.push(PatchOp::Remove { index: i as u32 });
            }
        }

        // 2. Updates. After the removals the list is the surviving old items in old order, so
        //    an item's index is its rank among the survivors.
        let mut rank = 0_u32;
        for (i, target) in old_to_new.iter().enumerate() {
            if let Some(j) = *target {
                if !eq(&old[i], &new[j]) {
                    ops.push(PatchOp::Update {
                        index: rank,
                        item: new[j].clone(),
                    });
                }
                rank += 1;
            }
        }

        // 3. Reorder and insert. `survivor_targets[r]` is the new position of the r-th
        //    survivor; survivors on a longest increasing run of it keep their place.
        let survivor_targets: Vec<usize> = old_to_new.iter().flatten().copied().collect();
        let mut stays = vec![false; n_new];
        mark_longest_increasing_run(&survivor_targets, &mut stays);

        // Working copy of the list as ids (= position in `new`); only survivors are present.
        let mut cur: Vec<usize> = survivor_targets;
        // Index at which the item after the one just placed belongs.
        let mut next = 0_usize;
        for j in 0..n_new {
            // `next` is one past the index of an item in `cur`, so this always holds; checking
            // it keeps every insert and remove below in range without relying on the proof.
            if next > cur.len() {
                return None;
            }
            if new_to_old[j].is_none() {
                ops.push(PatchOp::Insert {
                    index: next as u32,
                    item: new[j].clone(),
                });
                cur.insert(next, j);
                next += 1;
            } else if stays[j] {
                // Not moved. Everything between `next` and its position is a pending mover
                // that will be taken out when its own turn comes.
                let at = cur.get(next..)?.iter().position(|&id| id == j)? + next;
                next = at + 1;
            } else {
                let at = find(&cur, j, next)?;
                if at == next {
                    next += 1;
                } else if at > next {
                    ops.push(PatchOp::Move {
                        from: at as u32,
                        to: next as u32,
                    });
                    cur.remove(at);
                    cur.insert(next, j);
                    next += 1;
                } else {
                    // The item sits before the insertion point; removing it shifts the point
                    // (and everything placed so far) down by one, so it lands at `next - 1`.
                    ops.push(PatchOp::Move {
                        from: at as u32,
                        to: (next - 1) as u32,
                    });
                    cur.remove(at);
                    cur.insert(next - 1, j);
                }
            }
        }

        // Safety net: if the reasoning above were ever wrong, fall back to the full value
        // rather than emit a patch that produces the wrong list.
        if cur.len() != n_new || cur.iter().enumerate().any(|(pos, &id)| pos != id) {
            return None;
        }
        Some(KeyedPatch { ops })
    }
}

/// Position of `id` in `cur`, searching from `hint` forward first (where it usually is).
fn find(cur: &[usize], id: usize, hint: usize) -> Option<usize> {
    let hint = hint.min(cur.len());
    match cur.get(hint..)?.iter().position(|&x| x == id) {
        Some(p) => Some(p + hint),
        None => cur.get(..hint)?.iter().position(|&x| x == id),
    }
}

/// Marks, in `stays`, the values of one longest strictly increasing subsequence of `seq`.
/// `seq` must hold distinct values, each a valid index into `stays`.
fn mark_longest_increasing_run(seq: &[usize], stays: &mut [bool]) {
    if seq.windows(2).all(|w| w[0] < w[1]) {
        for &v in seq {
            if let Some(slot) = stays.get_mut(v) {
                *slot = true;
            }
        }
        return;
    }

    const NONE: usize = usize::MAX;
    // tails[k] = index into `seq` of the smallest possible last element of an increasing
    // subsequence of length k + 1; prev[i] = predecessor of seq[i] in the best subsequence
    // ending at i.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<usize> = vec![NONE; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let pos = tails.partition_point(|&t| seq[t] < v);
        if pos > 0 {
            prev[i] = tails[pos - 1];
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut cursor = tails.last().copied().unwrap_or(NONE);
    while cursor != NONE {
        if let Some(slot) = stays.get_mut(seq[cursor]) {
            *slot = true;
        }
        cursor = prev[cursor];
    }
}

impl<T: Encode> KeyedPatch<T> {
    /// Appends the patch to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_len(len_u32(self.ops.len()));
        for op in &self.ops {
            match op {
                PatchOp::Insert { index, item } => {
                    w.write_u8(OP_INSERT);
                    w.write_u32(*index);
                    item.encode(w);
                }
                PatchOp::Remove { index } => {
                    w.write_u8(OP_REMOVE);
                    w.write_u32(*index);
                }
                PatchOp::Update { index, item } => {
                    w.write_u8(OP_UPDATE);
                    w.write_u32(*index);
                    item.encode(w);
                }
                PatchOp::Move { from, to } => {
                    w.write_u8(OP_MOVE);
                    w.write_u32(*from);
                    w.write_u32(*to);
                }
                PatchOp::Clear => w.write_u8(OP_CLEAR),
            }
        }
    }
}

impl<T: Decode> KeyedPatch<T> {
    /// Reads a patch.
    ///
    /// The op count is checked against the remaining input (every op is at least one byte),
    /// and an unknown op tag is [`WireError::InvalidTag`] with `ty: "PatchOp"`. Indices are
    /// *not* validated here, only when the patch is [applied](KeyedPatch::apply).
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_count(1)?;
        let mut ops = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let at = r.position();
            let op = match r.read_u8()? {
                OP_INSERT => PatchOp::Insert {
                    index: r.read_u32()?,
                    item: T::decode(r)?,
                },
                OP_REMOVE => PatchOp::Remove {
                    index: r.read_u32()?,
                },
                OP_UPDATE => PatchOp::Update {
                    index: r.read_u32()?,
                    item: T::decode(r)?,
                },
                OP_MOVE => PatchOp::Move {
                    from: r.read_u32()?,
                    to: r.read_u32()?,
                },
                OP_CLEAR => PatchOp::Clear,
                tag => {
                    return Err(WireError::InvalidTag {
                        tag: u32::from(tag),
                        at,
                        ty: "PatchOp",
                    });
                }
            };
            ops.push(op);
        }
        Ok(KeyedPatch { ops })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type P = KeyedPatch<i32>;

    fn diff(old: &[(u32, i32)], new: &[(u32, i32)]) -> Option<KeyedPatch<(u32, i32)>> {
        KeyedPatch::diff(old, new, |t| t.0, |a, b| a == b)
    }

    fn check(old: &[(u32, i32)], new: &[(u32, i32)]) -> KeyedPatch<(u32, i32)> {
        let patch = diff(old, new).expect("expected a patch");
        let mut list = old.to_vec();
        patch.apply(&mut list).unwrap();
        assert_eq!(list, new, "patch {patch:?}");
        patch
    }

    #[test]
    fn wire_layout_matches_the_vector() {
        let patch = P {
            ops: vec![
                PatchOp::Insert { index: 0, item: 5 },
                PatchOp::Remove { index: 1 },
                PatchOp::Move { from: 0, to: 1 },
                PatchOp::Clear,
            ],
        };
        let mut w = Writer::new();
        patch.encode(&mut w);
        assert_eq!(
            w.as_slice(),
            [
                4, 0, 0, 0, // count
                0, 0, 0, 0, 0, 5, 0, 0, 0, // insert 0, 5
                1, 1, 0, 0, 0, // remove 1
                3, 0, 0, 0, 0, 1, 0, 0, 0, // move 0 -> 1
                4, // clear
            ]
        );
        assert_eq!(P::decode(&mut Reader::new(w.as_slice())), Ok(patch));
    }

    #[test]
    fn update_layout() {
        let patch = P {
            ops: vec![PatchOp::Update { index: 2, item: -1 }],
        };
        let mut w = Writer::new();
        patch.encode(&mut w);
        assert_eq!(
            w.as_slice(),
            [1, 0, 0, 0, 2, 2, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(P::decode(&mut Reader::new(w.as_slice())), Ok(patch));
    }

    #[test]
    fn decode_rejects_bad_tags_and_hostile_counts() {
        assert_eq!(
            P::decode(&mut Reader::new(&[1, 0, 0, 0, 5])),
            Err(WireError::InvalidTag {
                tag: 5,
                at: 4,
                ty: "PatchOp"
            })
        );
        assert!(matches!(
            P::decode(&mut Reader::new(&[0xff, 0xff, 0xff, 0xff, 4])),
            Err(WireError::LengthTooLarge { .. })
        ));
    }

    #[test]
    fn apply_semantics() {
        let mut list = vec![10, 20, 30];
        let patch = P {
            ops: vec![
                PatchOp::Insert { index: 3, item: 40 }, // append
                PatchOp::Insert { index: 0, item: 5 },  // prepend
                PatchOp::Remove { index: 1 },           // drops 10
                PatchOp::Update { index: 0, item: 6 },
                PatchOp::Move { from: 0, to: 3 }, // 6 goes to the end
            ],
        };
        patch.apply(&mut list).unwrap();
        assert_eq!(list, [20, 30, 40, 6]);
        P {
            ops: vec![PatchOp::Clear],
        }
        .apply(&mut list)
        .unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn move_semantics_remove_then_insert() {
        let mut list = vec!['a', 'b', 'c', 'd'];
        let mv = |from, to| KeyedPatch {
            ops: vec![PatchOp::<char>::Move { from, to }],
        };
        mv(0, 2).apply(&mut list).unwrap();
        assert_eq!(list, ['b', 'c', 'a', 'd']);
        mv(3, 0).apply(&mut list).unwrap();
        assert_eq!(list, ['d', 'b', 'c', 'a']);
        mv(1, 1).apply(&mut list).unwrap();
        assert_eq!(list, ['d', 'b', 'c', 'a']);
    }

    #[test]
    fn apply_rejects_out_of_bounds_and_leaves_the_list_untouched() {
        let cases: Vec<(PatchOp<i32>, u32)> = vec![
            (PatchOp::Insert { index: 4, item: 0 }, 4),
            (PatchOp::Remove { index: 3 }, 3),
            (PatchOp::Update { index: 3, item: 0 }, 3),
            (PatchOp::Move { from: 3, to: 0 }, 3),
            (PatchOp::Move { from: 0, to: 3 }, 3),
        ];
        for (bad, index) in cases {
            let mut list = vec![1, 2, 3];
            let patch = P {
                ops: vec![
                    PatchOp::Remove { index: 0 },
                    PatchOp::Insert { index: 0, item: 9 },
                    bad,
                ],
            };
            // After the first two ops the list still has 3 items, so `bad` is checked against 3.
            assert_eq!(
                patch.apply(&mut list),
                Err(PatchError::OutOfBounds {
                    op: 2,
                    index,
                    len: 3
                })
            );
            assert_eq!(list, [1, 2, 3], "list must be untouched on error");
        }
        let mut empty: Vec<i32> = Vec::new();
        assert!(
            P {
                ops: vec![PatchOp::Remove { index: 0 }]
            }
            .apply(&mut empty)
            .is_err()
        );
        assert!(
            P {
                ops: vec![PatchOp::Insert {
                    index: u32::MAX,
                    item: 0
                }]
            }
            .apply(&mut empty)
            .is_err()
        );
    }

    #[test]
    fn identical_lists_give_an_empty_patch() {
        let list = [(1, 1), (2, 2)];
        assert!(check(&list, &list).is_empty());
        assert!(check(&[], &[]).is_empty());
    }

    #[test]
    fn append_is_inserts_only() {
        let patch = check(&[(1, 1), (2, 2)], &[(1, 1), (2, 2), (3, 3), (4, 4)]);
        assert_eq!(
            patch.ops,
            [
                PatchOp::Insert {
                    index: 2,
                    item: (3, 3)
                },
                PatchOp::Insert {
                    index: 3,
                    item: (4, 4)
                },
            ]
        );
    }

    #[test]
    fn prepend_and_insert_in_the_middle() {
        check(&[(2, 0), (3, 0)], &[(1, 0), (2, 0), (3, 0)]);
        check(&[(1, 0), (3, 0)], &[(1, 0), (2, 0), (3, 0)]);
    }

    #[test]
    fn removals_are_descending_and_come_first() {
        let patch = check(
            &[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)],
            &[(1, 0), (3, 0), (5, 0)],
        );
        assert_eq!(
            patch.ops,
            [PatchOp::Remove { index: 3 }, PatchOp::Remove { index: 1 }]
        );
    }

    #[test]
    fn changed_items_become_updates() {
        let patch = check(&[(1, 1), (2, 2), (3, 3)], &[(1, 1), (2, 99), (3, 3)]);
        assert_eq!(
            patch.ops,
            [PatchOp::Update {
                index: 1,
                item: (2, 99)
            }]
        );
    }

    #[test]
    fn rotating_a_list_costs_one_move() {
        let old: Vec<(u32, i32)> = (0..10).map(|i| (i, 0)).collect();
        let mut new = old.clone();
        new.rotate_left(1); // first item goes to the end
        let patch = check(&old, &new);
        assert_eq!(patch.ops, [PatchOp::Move { from: 0, to: 9 }]);
        let mut new = old.clone();
        new.rotate_right(1); // last item goes to the front
        let patch = check(&old, &new);
        assert_eq!(patch.ops, [PatchOp::Move { from: 9, to: 0 }]);
    }

    #[test]
    fn swap_and_reverse() {
        check(&[(1, 0), (2, 0)], &[(2, 0), (1, 0)]);
        let old: Vec<(u32, i32)> = (0..8).map(|i| (i, 0)).collect();
        let new: Vec<(u32, i32)> = old.iter().rev().copied().collect();
        let patch = check(&old, &new);
        assert_eq!(patch.len(), 7, "reversing n items needs n - 1 moves");
    }

    #[test]
    fn everything_at_once() {
        check(
            &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6)],
            &[(6, 6), (7, 7), (3, 30), (1, 1), (8, 8), (5, 5)],
        );
    }

    #[test]
    fn full_value_is_preferred_when_most_items_are_removed() {
        let old: Vec<(u32, i32)> = (0..10).map(|i| (i, 0)).collect();
        // 6 of 10 removed: more than 50%.
        assert!(diff(&old, &old[..4]).is_none());
        // Exactly 5 of 10 removed: allowed.
        assert!(diff(&old, &old[..5]).is_some());
    }

    #[test]
    fn full_value_is_preferred_without_key_overlap() {
        assert!(diff(&[(1, 0), (2, 0)], &[(3, 0), (4, 0)]).is_none());
        assert!(diff(&[], &[(1, 0)]).is_none());
        assert!(diff(&[(1, 0)], &[]).is_none());
    }

    #[test]
    fn duplicate_keys_fall_back_to_the_full_value() {
        assert!(diff(&[(1, 0), (1, 1)], &[(1, 0)]).is_none());
        assert!(diff(&[(1, 0)], &[(1, 0), (1, 1)]).is_none());
    }

    #[test]
    fn a_wrong_eq_function_only_affects_updates() {
        // `eq` says everything is equal: no updates, structure is still right.
        let patch =
            KeyedPatch::diff(&[(1, 1), (2, 2)], &[(2, 9), (1, 9)], |t| t.0, |_, _| true).unwrap();
        assert!(
            patch
                .ops
                .iter()
                .all(|op| !matches!(op, PatchOp::Update { .. }))
        );
    }
}
