//! The ring of snapshots time travel restores from (ADR-054).
//!
//! A *step* is one [`Runtime::snapshot`](undra_runtime::Runtime::snapshot) the hub took after a
//! burst of commits. The ring keeps the newest steps within three bounds: how many (`max_steps`),
//! how many bytes in all (`max_bytes`) and how big one may be (`max_step_bytes`: a state over it
//! is listed, with its size, but its bytes are not kept, so it cannot be restored). History is
//! append-only: a restore is a new step, never a rewrite of an old one.

use std::collections::VecDeque;
use std::sync::Arc;

use super::proto::StepInfo;

/// One step: what the page is told, and the snapshot (absent when it was over the per-step bound).
pub(crate) struct Step {
    pub(crate) info: StepInfo,
    pub(crate) bytes: Option<Arc<Vec<u8>>>,
    /// The length and a hash of a snapshot that was not kept, so that the same state is not listed
    /// again and again (a commit that changes nothing a snapshot holds, a query handle's, is common).
    print: Option<(usize, u64)>,
}

/// A hash of `bytes` for telling two snapshots apart without keeping them.
fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// The bounded history of snapshots.
pub(crate) struct Ring {
    max_steps: usize,
    max_bytes: usize,
    max_step_bytes: usize,
    steps: VecDeque<Step>,
    bytes: usize,
    next: u32,
    /// The oldest step number still kept (steps below it were evicted).
    floor: u32,
}

/// What [`Ring::push`] did.
pub(crate) struct Pushed {
    /// The step that was added.
    pub(crate) info: StepInfo,
    /// The new floor when older steps were evicted to make room.
    pub(crate) evicted_below: Option<u32>,
}

impl Ring {
    pub(crate) fn new(max_steps: usize, max_bytes: usize, max_step_bytes: usize) -> Ring {
        Ring {
            max_steps: max_steps.max(1),
            max_bytes,
            max_step_bytes,
            steps: VecDeque::new(),
            bytes: 0,
            next: 1,
            floor: 1,
        }
    }

    /// The number the next step will get.
    #[cfg(test)]
    pub(crate) fn next_step(&self) -> u32 {
        self.next
    }

    /// Steps kept.
    pub(crate) fn len(&self) -> usize {
        self.steps.len()
    }

    /// Bytes of snapshot kept.
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The most recent snapshot that was kept.
    #[cfg(test)]
    pub(crate) fn newest_bytes(&self) -> Option<&Arc<Vec<u8>>> {
        self.steps.back().and_then(|s| s.bytes.as_ref())
    }

    /// Whether `snapshot` is the state the newest step already has (kept or not), so that a capture
    /// that found nothing new adds no step.
    pub(crate) fn unchanged(&self, snapshot: &[u8]) -> bool {
        match self.steps.back() {
            Some(Step { bytes: Some(kept), .. }) => kept.as_slice() == snapshot,
            Some(Step {
                bytes: None,
                print: Some((len, hash)),
                ..
            }) => *len == snapshot.len() && fingerprint(snapshot) == *hash,
            _ => false,
        }
    }

    /// The step `n`, when it is still in the ring.
    pub(crate) fn get(&self, n: u32) -> Option<&Step> {
        let first = self.steps.front()?.info.step;
        let index = usize::try_from(n.checked_sub(first)?).ok()?;
        self.steps.get(index).filter(|s| s.info.step == n)
    }

    /// Every step still kept, oldest first (for the page that attaches late).
    pub(crate) fn infos(&self) -> impl Iterator<Item = &StepInfo> {
        self.steps.iter().map(|s| &s.info)
    }

    /// Adds a step for `snapshot`. `info.step` is assigned here; the rest is the caller's.
    pub(crate) fn push(&mut self, mut info: StepInfo, snapshot: Vec<u8>) -> Pushed {
        info.step = self.next;
        self.next += 1;
        info.bytes = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
        let keep = snapshot.len() <= self.max_step_bytes;
        info.restorable = keep;
        let print = (!keep).then(|| (snapshot.len(), fingerprint(&snapshot)));
        let bytes = keep.then(|| Arc::new(snapshot));
        self.bytes += bytes.as_ref().map_or(0, |b| b.len());
        self.steps.push_back(Step {
            info: info.clone(),
            bytes,
            print,
        });
        let mut evicted = false;
        while self.steps.len() > 1 && (self.steps.len() > self.max_steps || self.bytes > self.max_bytes) {
            if let Some(old) = self.steps.pop_front() {
                self.bytes -= old.bytes.as_ref().map_or(0, |b| b.len());
                evicted = true;
            }
        }
        if evicted {
            self.floor = self.steps.front().map_or(self.next, |s| s.info.step);
        }
        Pushed {
            info,
            evicted_below: evicted.then_some(self.floor),
        }
    }

    /// Forgets everything (the last page left). Step numbers keep counting: a number is never
    /// reused within a core process.
    pub(crate) fn clear(&mut self) {
        self.steps.clear();
        self.bytes = 0;
        self.floor = self.next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> StepInfo {
        StepInfo {
            step: 0,
            through_seq: 0,
            txn: 0,
            at_ms: 0,
            bytes: 0,
            stores: 0,
            restorable: false,
            restored_from: 0,
        }
    }

    #[test]
    fn steps_are_numbered_from_one_and_found_by_number() {
        let mut ring = Ring::new(10, 1 << 20, 1 << 10);
        let a = ring.push(info(), vec![1; 10]).info;
        let b = ring.push(info(), vec![2; 20]).info;
        assert_eq!((a.step, b.step), (1, 2));
        assert_eq!(b.bytes, 20);
        assert!(b.restorable);
        assert_eq!(ring.get(1).unwrap().bytes.as_ref().unwrap().as_slice(), &[1; 10][..]);
        assert!(ring.get(3).is_none());
        assert!(ring.get(0).is_none());
        assert_eq!(ring.bytes(), 30);
    }

    #[test]
    fn the_step_count_bound_evicts_the_oldest() {
        let mut ring = Ring::new(3, 1 << 20, 1 << 10);
        let mut last = None;
        for i in 0..5_u8 {
            last = ring.push(info(), vec![i; 4]).evicted_below;
        }
        assert_eq!(ring.len(), 3);
        assert_eq!(last, Some(3), "steps 1 and 2 are gone");
        assert!(ring.get(2).is_none());
        assert!(ring.get(3).is_some());
        assert_eq!(ring.bytes(), 12);
    }

    #[test]
    fn the_byte_bound_evicts_until_it_fits_but_keeps_the_newest() {
        let mut ring = Ring::new(100, 100, 80);
        ring.push(info(), vec![0; 60]);
        let pushed = ring.push(info(), vec![0; 60]);
        assert_eq!(pushed.evicted_below, Some(2));
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.bytes(), 60);
        // A single step is never evicted by the byte bound, whatever it weighs.
        let mut tiny = Ring::new(100, 10, 80);
        tiny.push(info(), vec![0; 60]);
        assert_eq!(tiny.len(), 1);
    }

    #[test]
    fn a_state_over_the_per_step_bound_is_listed_but_not_kept() {
        let mut ring = Ring::new(10, 1 << 20, 16);
        let big = ring.push(info(), vec![0; 17]).info;
        assert!(!big.restorable);
        assert_eq!(big.bytes, 17);
        assert!(ring.get(big.step).unwrap().bytes.is_none());
        assert_eq!(ring.bytes(), 0);
        assert!(ring.newest_bytes().is_none());
    }

    #[test]
    fn clearing_keeps_the_numbering_going() {
        let mut ring = Ring::new(10, 1 << 20, 1 << 10);
        ring.push(info(), vec![1]);
        ring.push(info(), vec![2]);
        ring.clear();
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.next_step(), 3);
        assert_eq!(ring.push(info(), vec![3]).info.step, 3);
        assert!(ring.get(1).is_none());
    }

    #[test]
    fn the_documented_bounds_hold_at_their_edges() {
        const MIB: usize = 1 << 20;
        // 200 steps: the 200th is kept without evicting, the 201st evicts the oldest and only it.
        let mut ring = Ring::new(200, 32 * MIB, 4 * MIB);
        for i in 0..200_u8 {
            assert!(ring.push(info(), vec![i; 1]).evicted_below.is_none());
        }
        assert_eq!(ring.len(), 200);
        assert_eq!(ring.push(info(), vec![0; 1]).evicted_below, Some(2));
        assert_eq!((ring.len(), ring.get(1).is_none(), ring.get(2).is_some()), (200, true, true));

        // 4 MiB a step: exactly that is kept and restorable, one byte more is listed and not kept.
        let mut ring = Ring::new(200, 32 * MIB, 4 * MIB);
        let at = ring.push(info(), vec![0; 4 * MIB]).info;
        assert!(at.restorable && ring.get(at.step).unwrap().bytes.is_some());
        let over = ring.push(info(), vec![0; 4 * MIB + 1]).info;
        assert!(!over.restorable && ring.get(over.step).unwrap().bytes.is_none());
        assert_eq!(usize::try_from(over.bytes).unwrap(), 4 * MIB + 1, "the size is still told");
        assert_eq!(ring.bytes(), 4 * MIB, "a step that was not kept weighs nothing");

        // 32 MiB in all: eight steps of 4 MiB fill it exactly; the ninth evicts the oldest.
        let mut ring = Ring::new(200, 32 * MIB, 4 * MIB);
        for _ in 0..8 {
            assert!(ring.push(info(), vec![0; 4 * MIB]).evicted_below.is_none());
        }
        assert_eq!(ring.bytes(), 32 * MIB);
        assert_eq!(ring.push(info(), vec![1; 4 * MIB]).evicted_below, Some(2));
        assert_eq!((ring.bytes(), ring.len()), (32 * MIB, 8));
        // One byte over the total with the same step count evicts too.
        let mut ring = Ring::new(200, 100, 100);
        ring.push(info(), vec![0; 50]);
        ring.push(info(), vec![0; 50]);
        assert_eq!(ring.len(), 2, "exactly the bound fits");
        assert_eq!(ring.push(info(), vec![0; 1]).evicted_below, Some(2));
        assert_eq!(ring.bytes(), 51);
    }

    #[test]
    fn a_step_that_was_not_kept_is_evicted_without_unbalancing_the_total() {
        let mut ring = Ring::new(2, 1 << 20, 16);
        ring.push(info(), vec![0; 17]); // listed, not kept
        ring.push(info(), vec![0; 10]);
        ring.push(info(), vec![0; 10]); // evicts the one that was not kept
        assert_eq!((ring.len(), ring.bytes()), (2, 20));
        ring.clear();
        assert_eq!((ring.len(), ring.bytes()), (0, 0), "nothing is kept once the page leaves");
    }

    #[test]
    fn the_same_state_is_not_listed_twice_whether_or_not_it_was_kept() {
        let mut ring = Ring::new(10, 1 << 20, 16);
        assert!(!ring.unchanged(&[1; 4]), "an empty ring has nothing to equal");
        ring.push(info(), vec![1; 4]);
        assert!(ring.unchanged(&[1; 4]) && !ring.unchanged(&[2; 4]) && !ring.unchanged(&[1; 5]));
        // A state over the per-step bound is not kept, and is still recognised by its length and hash.
        ring.push(info(), vec![7; 40]);
        assert!(ring.get(2).unwrap().bytes.is_none());
        assert!(ring.unchanged(&[7; 40]));
        let mut other = vec![7; 40];
        other[39] = 8;
        assert!(!ring.unchanged(&other), "one byte different is a different state");
        assert!(!ring.unchanged(&[7; 41]));
        ring.clear();
        assert!(!ring.unchanged(&[7; 40]), "nothing is compared with once the page has left");
    }

    #[test]
    fn newest_bytes_is_what_the_dedupe_compares_with() {
        let mut ring = Ring::new(10, 1 << 20, 1 << 10);
        assert!(ring.newest_bytes().is_none());
        ring.push(info(), vec![7, 7]);
        assert_eq!(ring.newest_bytes().unwrap().as_slice(), &[7, 7][..]);
        assert_eq!(ring.infos().count(), 1);
    }
}
