//! `sort_ids` (the hand-written Shell's sort that replaced three `slice::sort` instantiations for
//! the web core's size, ADR-052) sorts exactly as the standard library does: any length, duplicates,
//! nearly-sorted and reversed input (types-paging review).

use proptest::prelude::*;

proptest! {
    #[test]
    fn sort_ids_agrees_with_the_standard_sort(mut ids in proptest::collection::vec(any::<u32>(), 0..600)) {
        let mut expected = ids.clone();
        expected.sort_unstable();
        undra_signals::sort_ids(&mut ids);
        prop_assert_eq!(ids, expected);
    }

    #[test]
    fn sort_ids_handles_few_distinct_values(mut ids in proptest::collection::vec(0_u32..4, 0..300)) {
        let mut expected = ids.clone();
        expected.sort_unstable();
        undra_signals::sort_ids(&mut ids);
        prop_assert_eq!(ids, expected);
    }
}

#[test]
fn sort_ids_sorts_reversed_and_nearly_sorted_lists_of_every_small_length() {
    for n in 0_u32..200 {
        let mut reversed: Vec<u32> = (0..n).rev().collect();
        undra_signals::sort_ids(&mut reversed);
        assert_eq!(reversed, (0..n).collect::<Vec<_>>(), "reversed, length {n}");
        let mut nearly: Vec<u32> = (0..n).collect();
        if n > 2 {
            nearly.swap(0, (n - 1) as usize);
        }
        undra_signals::sort_ids(&mut nearly);
        assert_eq!(
            nearly,
            (0..n).collect::<Vec<_>>(),
            "nearly sorted, length {n}"
        );
    }
}
