//! The stress store: high-frequency data, produced by the core itself.
//!
//! A socket, a sensor or a simulation writes state far faster than any screen refreshes. `burst`
//! does that on demand: it commits one transaction per write, in a tight loop, without batching
//! them in [`Ctx::txn`], so the platforms receive one change-set per transaction. Their mirrors
//! merge what arrives between two frames (ADR-031), so a burst of 1,000 writes to `value` is
//! applied to the UI once, with the final value, while `progress`, declared
//! `#[undra(no_coalesce)]`, is applied step by step, as a progress bar wants.
//!
//! Contract scenario S18 (`contract-tests/scenarios.md`) drives it on every platform.

use undra::prelude::*;

/// Which signal [`Stress::burst`] writes.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StressMode {
    /// `value` + 1 per transaction: a firehose the platforms apply once per frame.
    Firehose,
    /// `progress` + 1 per transaction: a `no_coalesce` signal the platforms apply step by step.
    Progress,
}

/// A store that commits many transactions at once, for the coalesced-delivery scenarios.
#[undra::store]
pub struct Stress {
    /// Written by [`StressMode::Firehose`]: one more per transaction.
    value: Signal<u64>,
    /// Written by [`StressMode::Progress`]: one more per transaction. Every value reaches the
    /// platforms' mirrors.
    #[undra(no_coalesce)]
    progress: Signal<u32>,
}

#[undra::api(store)]
impl Stress {
    /// A store with both signals at zero.
    pub fn new(_ctx: Ctx) -> Self {
        Self {
            value: Signal::new(0),
            progress: Signal::new(0),
        }
    }

    /// Commits `transactions` transactions now, each one write of the signal `mode` names: one
    /// change-set per transaction, the way data that arrives in the core on its own does.
    pub fn burst(&self, mode: StressMode, transactions: u32) {
        for _ in 0..transactions {
            match mode {
                StressMode::Firehose => self.value.update(|v| *v = v.wrapping_add(1)),
                StressMode::Progress => self.progress.update(|p| *p = p.wrapping_add(1)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use undra::meta::{collect_schema, ids};
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ReplyStatus};
    use undra::wire::{Decode, Encode, Writer};

    use super::*;

    fn construct(t: &TestRuntime) -> Handle {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("Stress"),
                method_id: ids::method_id("Stress", "new"),
            },
            1,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        Handle::decode_exact(&reply.body).expect("a handle")
    }

    fn burst(t: &TestRuntime, store: Handle, mode: StressMode, transactions: u32) {
        let mut w = Writer::new();
        mode.encode(&mut w);
        transactions.encode(&mut w);
        let reply = t.call_sync(
            CallTarget::Method {
                handle: store,
                method_id: ids::method_id("Stress", "burst"),
            },
            2,
            w.as_slice(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    }

    #[test]
    fn a_firehose_burst_is_one_change_set_per_transaction() {
        let t = TestRuntime::new();
        let store = construct(&t);
        t.runtime().observe(store.0, ALL_SIGNALS, true);
        t.host().take_decoded_change_sets();
        burst(&t, store, StressMode::Firehose, 1000);
        let sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 1000);
        for (i, set) in sets.iter().enumerate() {
            assert_eq!(set.entries.len(), 1);
            let entry = &set.entries[0];
            assert_eq!((entry.signal_id, entry.op), (0, ChangeOp::Full));
            assert_eq!(u64::decode_exact(&entry.value).unwrap(), i as u64 + 1);
        }
        let txns: Vec<u64> = sets.iter().map(|s| s.txn_id).collect();
        assert!(txns.windows(2).all(|w| w[0] < w[1]), "commit order");
    }

    #[test]
    fn progress_is_delivered_even_while_nobody_observes_it() {
        let t = TestRuntime::new();
        let store = construct(&t);
        t.host().take_decoded_change_sets();
        burst(&t, store, StressMode::Progress, 3);
        burst(&t, store, StressMode::Firehose, 3);
        let sets = t.host().take_decoded_change_sets();
        let progress: Vec<u32> = sets
            .iter()
            .flat_map(|s| &s.entries)
            .map(|e| {
                assert_eq!(e.signal_id, 1, "only the no_coalesce signal is delivered");
                u32::decode_exact(&e.value).unwrap()
            })
            .collect();
        assert_eq!(progress, [1, 2, 3]);
    }

    #[test]
    fn the_schema_marks_progress_no_coalesce() {
        let schema = collect_schema("playground-core");
        let stress = schema.objects.iter().find(|o| o.name == "Stress").unwrap();
        let signals: Vec<(&str, bool)> = stress
            .store
            .as_ref()
            .unwrap()
            .signals
            .iter()
            .map(|s| (s.name.as_str(), s.no_coalesce))
            .collect();
        assert_eq!(signals, [("value", false), ("progress", true)]);
    }
}
