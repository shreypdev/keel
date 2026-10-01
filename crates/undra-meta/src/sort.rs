//! The one sort the schema code uses: stable, by a key, and small in a shipped core.
//!
//! [`collect_schema`](crate::collect_schema) and the canonical form (SPEC 2.3) sort a dozen lists
//! of definitions by name, and both run inside every core: the runtime collects its schema and
//! hashes it at start-up, and `undra_schema_json` serializes it. `slice::sort_by` emits a full
//! driftsort (quicksort, merge and small-sort paths, about 3.5 KB of wasm) for every element type
//! *and every closure*: sixteen of them came to 58 KB of the 353 KB hello-world web core, its
//! largest item after the query layer (ADR-052).
//!
//! Here a list of up to 16 items (most of them) is insertion-sorted in place, and a longer one is
//! sorted once per key type, on indices: [`stable_order`] is a bottom-up merge sort (O(n log n),
//! stable) over the keys, and [`permute`] moves the items into that order with swaps. Only the
//! insertion loop, the key extraction and the swap loop are generic over the item type, and each
//! is a few instructions. A long list already in order (the canonical form re-sorts what
//! `collect_schema` sorted) costs one pass over it and no allocation.
//!
//! The result is exactly what `slice::sort_by` / `sort_by_key` would produce with the same key
//! (tests compare them), so the canonical form and the schema hash cannot move. Hashing a schema
//! whose 2,000 records arrive in reverse order takes 239 µs on an Apple M5 Pro (205 µs with
//! `sort_by`); an insertion sort for every length, 3 KB smaller still, took 9 ms there, which is why
//! only short lists take one.

/// Sorts `items` by the name `name` returns, stably (equal names keep their order).
pub(crate) fn by_name<T>(items: &mut [T], name: fn(&T) -> &str) {
    if items.len() <= SMALL {
        insertion(items, |a, b| name(a) > name(b));
        return;
    }
    if items.is_sorted_by(|a, b| name(a) <= name(b)) {
        return;
    }
    let keys: Vec<&str> = items.iter().map(name).collect();
    let order = stable_order(&keys);
    drop(keys);
    permute(items, order);
}

/// Sorts `items` by the `u16` key `key` returns, stably (enum variants by wire index).
pub(crate) fn by_index<T>(items: &mut [T], key: fn(&T) -> u16) {
    if items.len() <= SMALL {
        insertion(items, |a, b| key(a) > key(b));
        return;
    }
    if items.is_sorted_by(|a, b| key(a) <= key(b)) {
        return;
    }
    let keys: Vec<u16> = items.iter().map(key).collect();
    permute(items, stable_order(&keys));
}

/// Up to this many items, an in-place insertion sort: no allocation, and fewer steps than the
/// merge sort's bookkeeping (most schema lists are this short).
const SMALL: usize = 16;

/// Stable insertion sort with swaps; `after(a, b)` says `a` belongs strictly after `b`.
fn insertion<T>(items: &mut [T], after: impl Fn(&T, &T) -> bool) {
    for end in 1..items.len() {
        let mut at = end;
        while at > 0 && after(&items[at - 1], &items[at]) {
            items.swap(at - 1, at);
            at -= 1;
        }
    }
}

/// The indices of `keys` in stable sorted order: `order[i]` is the index of the `i`-th smallest
/// key, equal keys in their original order. A bottom-up merge sort over indices.
fn stable_order<K: Ord>(keys: &[K]) -> Vec<usize> {
    let len = keys.len();
    let mut order: Vec<usize> = (0..len).collect();
    let mut merged = vec![0; len];
    let mut width = 1;
    while width < len {
        let mut start = 0;
        while start < len {
            let mid = (start + width).min(len);
            let end = (start + 2 * width).min(len);
            let (mut left, mut right) = (start, mid);
            for slot in &mut merged[start..end] {
                // The left run wins ties, which is what makes the sort stable.
                if right >= end || (left < mid && keys[order[left]] <= keys[order[right]]) {
                    *slot = order[left];
                    left += 1;
                } else {
                    *slot = order[right];
                    right += 1;
                }
            }
            start = end;
        }
        core::mem::swap(&mut order, &mut merged);
        width *= 2;
    }
    order
}

/// Rearranges `items` so that the item at `order[i]` ends up at `i`, following each cycle of
/// the permutation with swaps. `order` must be a permutation of `0..items.len()`.
fn permute<T>(items: &mut [T], mut order: Vec<usize>) {
    for first in 0..items.len() {
        let mut at = first;
        while order[at] != first {
            let from = order[at];
            items.swap(at, from);
            order[at] = at;
            at = from;
        }
        order[at] = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name and where it was before sorting, so stability is visible.
    type Item = (&'static str, usize);

    fn name(item: &Item) -> &str {
        item.0
    }

    /// Every arrangement of `items` (Heap's algorithm).
    fn permutations(items: &mut Vec<Item>, k: usize, out: &mut Vec<Vec<Item>>) {
        if k <= 1 {
            out.push(items.clone());
            return;
        }
        for i in 0..k {
            permutations(items, k - 1, out);
            let j = if k % 2 == 0 { i } else { 0 };
            items.swap(j, k - 1);
        }
    }

    #[test]
    fn equals_the_standard_stable_sort_on_every_arrangement() {
        // Duplicates included: a stable sort keeps them in their input order.
        let names = ["b", "a", "c", "a", "b", "", "aa"];
        let mut base: Vec<Item> = names.iter().copied().zip(0..).collect();
        let mut all = Vec::new();
        let len = base.len();
        permutations(&mut base, len, &mut all);
        assert_eq!(all.len(), 5040);
        for input in all {
            let mut ours = input.clone();
            by_name(&mut ours, name);
            let mut std = input;
            std.sort_by(|a, b| a.0.cmp(b.0));
            assert_eq!(ours, std);
        }
    }

    #[test]
    fn equals_the_standard_stable_sort_on_long_inputs() {
        // A deterministic pseudo-random sequence (an LCG), with many repeated keys.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let words = [
            "Todo", "Todos", "Filter", "add", "remove", "z", "ü", "A", "a", "_",
        ];
        for len in [0, 1, 2, 3, 15, 16, 17, 31, 64, 65, 200, 1000] {
            let input: Vec<Item> = (0..len)
                .map(|i| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    (words[(state >> 33) as usize % words.len()], i)
                })
                .collect();
            let mut ours = input.clone();
            by_name(&mut ours, name);
            let mut std = input;
            std.sort_by(|a, b| a.0.cmp(b.0));
            assert_eq!(ours, std, "length {len}");
        }
    }

    #[test]
    fn by_index_equals_sort_by_key() {
        let input = [
            (3_u16, 'a'),
            (1, 'b'),
            (3, 'c'),
            (0, 'd'),
            (1, 'e'),
            (u16::MAX, 'f'),
        ];
        let mut ours = input;
        by_index(&mut ours, |i| i.0);
        let mut std = input;
        std.sort_by_key(|i| i.0);
        assert_eq!(ours, std);
        assert_eq!(ours[..3], [(0, 'd'), (1, 'b'), (1, 'e')]);

        // Past the insertion-sort threshold: the merge path.
        let long: Vec<(u16, usize)> = (0..100).map(|i| ((i * 37 % 11) as u16, i)).collect();
        let mut ours = long.clone();
        by_index(&mut ours, |i| i.0);
        let mut std = long;
        std.sort_by_key(|i| i.0);
        assert_eq!(ours, std);
    }

    #[test]
    fn sorted_and_reversed_inputs() {
        let sorted: Vec<Item> = (0..100)
            .map(|i| (["a", "b", "c", "d"][i / 25], i))
            .collect();
        let mut items = sorted.clone();
        by_name(&mut items, name);
        assert_eq!(items, sorted);
        let mut reversed: Vec<Item> = sorted.iter().rev().copied().collect();
        by_name(&mut reversed, name);
        let mut std: Vec<Item> = sorted.iter().rev().copied().collect();
        std.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(reversed, std);
    }

    /// The keys the property tests draw from: few, so most inputs have runs of equal keys, and
    /// ordered the way `str::cmp` orders them (byte-wise: `"B" < "a"`, `"ü"` last).
    const WORDS: [&str; 6] = ["", "B", "a", "aa", "b", "ü"];

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2048))]

        /// `by_name` is `sort_by(|a, b| a.name.cmp(&b.name))`, the comparator `collect_schema`
        /// and the canonical form used before: same order, equal names in input order, on both
        /// sides of the in-place threshold (16) and well past it.
        #[test]
        fn by_name_is_sort_by_on_any_input(keys in proptest::collection::vec(0..WORDS.len(), 0..80)) {
            let input: Vec<Item> = keys.iter().map(|&k| WORDS[k]).zip(0..).collect();
            let mut ours = input.clone();
            by_name(&mut ours, name);
            let mut std = input;
            std.sort_by(|a, b| a.0.cmp(b.0));
            proptest::prop_assert_eq!(ours, std);
        }

        /// `by_index` is `sort_by_key(|v| v.index)`, the variants' comparator, on any `u16`
        /// (a narrow range most of the time, so indexes repeat).
        #[test]
        fn by_index_is_sort_by_key_on_any_input(
            keys in proptest::collection::vec(proptest::prop_oneof![0..4_u16, proptest::num::u16::ANY], 0..80),
        ) {
            let input: Vec<(u16, usize)> = keys.into_iter().zip(0..).collect();
            let mut ours = input.clone();
            by_index(&mut ours, |i| i.0);
            let mut std = input;
            std.sort_by_key(|i| i.0);
            proptest::prop_assert_eq!(ours, std);
        }
    }

    #[test]
    fn the_threshold_between_the_two_paths() {
        // 15 to 18 items: the last in-place lengths and the first merge-sorted ones, for inputs
        // already in order (the merge path's one-pass exit), reversed, all equal (stability is
        // all there is to check) and alternating.
        for len in 15..=18 {
            let shapes: [Vec<&str>; 4] = [
                (0..len).map(|i| WORDS[i * WORDS.len() / len]).collect(),
                (0..len)
                    .rev()
                    .map(|i| WORDS[i * WORDS.len() / len])
                    .collect(),
                vec!["a"; len],
                (0..len).map(|i| WORDS[i % 2 + 1]).collect(),
            ];
            for shape in shapes {
                let input: Vec<Item> = shape.into_iter().zip(0..).collect();
                let mut ours = input.clone();
                by_name(&mut ours, name);
                let mut std = input.clone();
                std.sort_by(|a, b| a.0.cmp(b.0));
                assert_eq!(ours, std, "length {len}: {input:?}");
            }
        }
    }

    #[test]
    fn permute_applies_the_order() {
        let mut items = ['a', 'b', 'c', 'd', 'e'];
        permute(&mut items, vec![2, 0, 1, 4, 3]);
        assert_eq!(items, ['c', 'a', 'b', 'e', 'd']);
        let mut empty: [char; 0] = [];
        permute(&mut empty, Vec::new());
    }
}
