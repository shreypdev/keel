//! The reload of `undra dev`: replacing the serving core with the rebuilt one without losing its
//! state (ADR-053).
//!
//! This module is the *decisions and the order*; the process plumbing lives in `commands::dev`
//! behind the [`Ops`] trait, so the order of the steps and every row of the failure matrix are
//! unit tests with a recording fake instead of a build.
//!
//! ```text
//! rebuild ok
//!   1. standby   the new core starts, builds its runtime, does not listen
//!        fails -> the old core keeps serving, nothing was touched
//!   2. snapshot  the old core suspends its server (no new calls, open calls settle, the client is
//!                closed, its session kept) and answers with its state
//!        skipped with `--no-keep-state`; failed -> fresh state. A changed schema is no reason to
//!        skip it: the new core migrates the state by name or refuses it (ADR-037)
//!   3. stop      the old core exits (its address is free)
//!   4. restore   the new core restores the state (or is told why not), before it listens
//!   5. listen    the new core serves; clients reconnect and resume (ADR-051)
//! ```

use std::time::Duration;

use crate::error::CliError;

/// The most state a reload carries, in bytes of snapshot: it crosses a pipe as hex (twice the size)
/// and the dev loop has to stay quick; a core with more state than this is better reset than stalled.
pub const STATE_LIMIT_BYTES: usize = 16 << 20;

/// The variable that lowers [`STATE_LIMIT_BYTES`] (a testing aid: the integration tests prove the
/// over-the-limit path with a small state; it can only lower the limit).
pub const STATE_LIMIT_ENV: &str = "UNDRA_DEV_STATE_LIMIT_BYTES";

/// The limit in force: [`STATE_LIMIT_BYTES`], or a lower one from [`STATE_LIMIT_ENV`].
#[must_use]
pub fn state_limit() -> usize {
    std::env::var(STATE_LIMIT_ENV)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(STATE_LIMIT_BYTES, |lowered| lowered.min(STATE_LIMIT_BYTES))
}

/// How long the serving core gets to finish the calls it is already running before its client is
/// closed for a reload: long enough for a port round trip, short enough that a stuck call cannot
/// stall the swap.
pub const SETTLE: Duration = Duration::from_millis(2_000);

/// How long after a reload a client that attaches is still told what became of its state: a
/// reconnecting client retries for at most five seconds a time, and a notice minutes late would
/// describe something the developer has moved past.
pub const NOTICE_WINDOW: Duration = Duration::from_secs(30);

/// How long the old core has to answer `snapshot`: the settle, the encoding and the pipe, with room.
pub const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(15);

/// The state of a core, as its runner handed it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The `Runtime::snapshot` bytes.
    pub bytes: Vec<u8>,
    /// How many stores they hold.
    pub stores: usize,
    /// The session the client left (token and the handles of the objects it made), if any.
    pub session: Option<(String, Vec<u64>)>,
    /// Whether every call that was open had finished when the client was closed.
    pub settled: bool,
    /// How many were still open, and were cancelled.
    pub cancelled: usize,
    /// How many calls the client sent after the old core stopped running calls: never run, so their
    /// writes are not in [`bytes`](Snapshot::bytes).
    pub dropped: usize,
}

impl Snapshot {
    /// The calls the reload cut off, whose writes are not in the state (or only in part): the
    /// cancelled ones and the ones never run. The notice to the app counts them.
    #[must_use]
    pub fn lost_calls(&self) -> usize {
        self.cancelled + self.dropped
    }
}

/// What the new core said about the state it was handed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    /// Stores restored.
    pub stores: usize,
    /// Objects the client held that are not carried over (plain objects, query handles).
    pub lost: usize,
    /// Bytes of snapshot.
    pub bytes: usize,
    /// How long `Runtime::restore` took, in microseconds.
    pub micros: u128,
}

/// What became of the core's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The new core restored the state.
    Kept {
        /// What was restored.
        restored: Restored,
        /// How many calls were still running when the old core was replaced, and were cancelled.
        cancelled: usize,
        /// How many calls were sent after the old core stopped running calls, and were not run.
        dropped: usize,
    },
    /// The old core had no stores: there was nothing to carry.
    NothingToKeep,
    /// The state was not carried over; the reason reads after "state reset: ".
    Reset(String),
}

impl Outcome {
    /// The terminal sentence for the `Restarted:` line.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Outcome::Kept {
                restored,
                cancelled,
                dropped,
            } => {
                let mut text = format!(
                    "state kept ({}, {}, restored in {})",
                    plural(restored.stores, "store", "stores"),
                    kib(restored.bytes),
                    took(restored.micros)
                );
                if restored.lost > 0 {
                    text.push_str(&format!(
                        "; {} not carried over: their handles are stale, the app creates them again",
                        plural(restored.lost, "object", "objects")
                    ));
                }
                if *cancelled > 0 {
                    text.push_str(&format!(
                        "; {} still running when the core was replaced {} cancelled",
                        plural(*cancelled, "call", "calls"),
                        if *cancelled == 1 { "was" } else { "were" }
                    ));
                }
                if *dropped > 0 {
                    text.push_str(&format!(
                        "; {} sent during the reload {} not run",
                        plural(*dropped, "call", "calls"),
                        if *dropped == 1 { "was" } else { "were" }
                    ));
                }
                text
            }
            Outcome::NothingToKeep => "no state to keep (no store was alive)".to_owned(),
            Outcome::Reset(reason) => format!("state reset: {reason}"),
        }
    }
}

/// `1 store`, `3 stores`.
#[must_use]
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `189 us`, `2.4 ms`.
#[must_use]
pub fn took(micros: u128) -> String {
    if micros < 1000 {
        format!("{micros} \u{b5}s")
    } else {
        format!("{:.1} ms", micros as f64 / 1000.0)
    }
}

/// `204 KiB`, `1.5 MiB`.
#[must_use]
pub fn kib(bytes: usize) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / f64::from(1_u32 << 20))
    } else {
        format!("{} KiB", bytes.div_ceil(1024).max(1))
    }
}

/// The reason for a snapshot over [`state_limit`].
#[must_use]
pub fn over_the_limit() -> String {
    let limit = state_limit();
    if limit % (1 << 20) == 0 {
        format!("snapshot over {} MiB", limit >> 20)
    } else {
        format!("snapshot over {} bytes", limit)
    }
}

/// What the swap needs from the two runners; `commands::dev` implements it over processes.
pub trait Ops {
    /// The serving runner.
    type Old;
    /// The runner that replaces it.
    type New;

    /// Starts the rebuilt core in standby: its runtime exists, nothing listens. Returns it with its
    /// schema hash, or why it did not start (nothing else has been touched yet).
    fn standby(&mut self) -> Result<(Self::New, String), String>;

    /// Suspends the old core's server and takes its state. `Err` is the reason it could not, as
    /// it reads after "state reset: ".
    fn snapshot(&mut self, old: &mut Self::Old) -> Result<Snapshot, String>;

    /// Stops the old runner (its address is free when this returns).
    fn stop(&mut self, old: Self::Old);

    /// Hands the state to the new core, which restores it. `Err` is the reason it did not (the core
    /// has already been told to start fresh).
    fn restore(
        &mut self,
        new: &mut Self::New,
        old_hash: &str,
        snapshot: &Snapshot,
    ) -> Result<Restored, String>;

    /// Tells the new core the state is not coming, and why.
    fn reset(&mut self, new: &mut Self::New, reason: &str);

    /// Tells the new core to listen; returns its URL.
    ///
    /// # Errors
    ///
    /// When it cannot bind, or exits.
    fn listen(&mut self, new: &mut Self::New) -> Result<String, CliError>;
}

/// How a swap ended.
pub enum Swap<O: Ops> {
    /// The rebuilt core did not start: the old one is untouched and still serves, with its state.
    StillServing {
        /// The old runner, handed back.
        old: O::Old,
        /// Why the new one did not start.
        why: String,
    },
    /// The new core serves.
    Swapped {
        /// The new runner.
        new: O::New,
        /// Where it listens.
        url: String,
        /// Its schema hash.
        hash: String,
        /// What became of the state.
        outcome: Outcome,
    },
}

/// Replaces `old` (serving a core with schema hash `old_hash`) with the rebuilt core.
///
/// # Errors
///
/// When the new core cannot listen after the old one is gone (as a failed start has always been).
pub fn swap<O: Ops>(
    ops: &mut O,
    old: O::Old,
    old_hash: &str,
    keep_state: bool,
) -> Result<Swap<O>, CliError> {
    let (mut new, hash) = match ops.standby() {
        Ok(started) => started,
        Err(why) => return Ok(Swap::StillServing { old, why }),
    };
    let mut old = old;
    // Across a schema change too: a restore matches signals by name and migrates what changed
    // structurally, or refuses as a whole and changes nothing (ADR-037), and the new core says which
    // (`restore` gets the old hash). A client built from the old bindings is still refused at its
    // `Hello` (R7): a schema change means `undra bindgen` for the app, not lost state.
    let not_taken = if keep_state {
        None
    } else {
        Some("undra dev --no-keep-state".to_owned())
    };
    let taken = match not_taken {
        Some(reason) => Err(reason),
        None => ops.snapshot(&mut old),
    };
    ops.stop(old);
    let outcome = match taken {
        Ok(snapshot) if snapshot.stores == 0 => Outcome::NothingToKeep,
        Ok(snapshot) => match ops.restore(&mut new, old_hash, &snapshot) {
            Ok(restored) => Outcome::Kept {
                restored,
                cancelled: snapshot.cancelled,
                dropped: snapshot.dropped,
            },
            Err(reason) => Outcome::Reset(reason),
        },
        Err(reason) => {
            ops.reset(&mut new, &reason);
            Outcome::Reset(reason)
        }
    };
    let url = ops.listen(&mut new)?;
    Ok(Swap::Swapped {
        new,
        url,
        hash,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the steps and answers from a script.
    struct Fake {
        steps: Vec<String>,
        standby: Result<String, String>,
        snapshot: Result<Snapshot, String>,
        restore: Result<Restored, String>,
        listen_fails: bool,
    }

    fn snapshot(stores: usize) -> Snapshot {
        Snapshot {
            bytes: vec![0; 8],
            stores,
            session: Some(("tok".into(), vec![1])),
            settled: true,
            cancelled: 0,
            dropped: 0,
        }
    }

    impl Fake {
        fn ok() -> Fake {
            Fake {
                steps: Vec::new(),
                standby: Ok("0xaa".into()),
                snapshot: Ok(snapshot(2)),
                restore: Ok(Restored {
                    stores: 2,
                    lost: 0,
                    bytes: 8,
                    micros: 5,
                }),
                listen_fails: false,
            }
        }
    }

    impl Ops for Fake {
        type Old = &'static str;
        type New = &'static str;

        fn standby(&mut self) -> Result<(&'static str, String), String> {
            self.steps.push("standby".into());
            self.standby.clone().map(|hash| ("new", hash))
        }

        fn snapshot(&mut self, _: &mut &'static str) -> Result<Snapshot, String> {
            self.steps.push("snapshot".into());
            self.snapshot.clone()
        }

        fn stop(&mut self, _: &'static str) {
            self.steps.push("stop-old".into());
        }

        fn restore(
            &mut self,
            _: &mut &'static str,
            old_hash: &str,
            _: &Snapshot,
        ) -> Result<Restored, String> {
            self.steps.push(format!("restore({old_hash})"));
            self.restore.clone()
        }

        fn reset(&mut self, _: &mut &'static str, reason: &str) {
            self.steps.push(format!("reset({reason})"));
        }

        fn listen(&mut self, _: &mut &'static str) -> Result<String, CliError> {
            self.steps.push("listen".into());
            if self.listen_fails {
                Err(CliError::new(
                    crate::error::Code::Dev,
                    "bind",
                    "taken",
                    "free it",
                ))
            } else {
                Ok("ws://127.0.0.1:1".into())
            }
        }
    }

    fn run(fake: &mut Fake, keep: bool) -> Swap<Fake> {
        swap(fake, "old", "0xaa", keep).expect("a swap")
    }

    fn outcome(swap: Swap<Fake>) -> Outcome {
        match swap {
            Swap::Swapped { outcome, .. } => outcome,
            Swap::StillServing { why, .. } => panic!("still serving: {why}"),
        }
    }

    #[test]
    fn the_steps_happen_in_the_order_that_cannot_lose_state() {
        let mut fake = Fake::ok();
        let swapped = run(&mut fake, true);
        // The new core exists before the old one is touched; the old one is quiesced and snapshotted
        // before it stops; the state is restored before the new core listens.
        assert_eq!(
            fake.steps,
            ["standby", "snapshot", "stop-old", "restore(0xaa)", "listen"]
        );
        assert!(matches!(outcome(swapped), Outcome::Kept { .. }));
    }

    #[test]
    fn a_core_that_does_not_start_leaves_the_old_one_serving_untouched() {
        let mut fake = Fake::ok();
        fake.standby = Err("exited before it was ready".into());
        match run(&mut fake, true) {
            Swap::StillServing { old, why } => {
                assert_eq!(old, "old");
                assert_eq!(why, "exited before it was ready");
            }
            Swap::Swapped { .. } => panic!("swapped"),
        }
        assert_eq!(fake.steps, ["standby"], "no snapshot, no stop");
    }

    #[test]
    fn a_changed_schema_is_handed_to_the_new_core_which_migrates_or_refuses_it() {
        // ADR-037: the restore migrates by name or refuses as a whole, so the state is offered.
        let mut fake = Fake::ok();
        fake.standby = Ok("0xbb".into());
        let swapped = run(&mut fake, true);
        assert_eq!(
            fake.steps,
            ["standby", "snapshot", "stop-old", "restore(0xaa)", "listen"]
        );
        assert!(matches!(outcome(swapped), Outcome::Kept { .. }));
        // A refusal (an incompatible change) is fresh state with the core's reason.
        let mut fake = Fake::ok();
        fake.standby = Ok("0xbb".into());
        fake.restore =
            Err("the core refused the snapshot: Counter.count: i32 cannot become String".into());
        assert_eq!(
            outcome(run(&mut fake, true)),
            Outcome::Reset(
                "the core refused the snapshot: Counter.count: i32 cannot become String".into()
            )
        );
    }

    #[test]
    fn no_keep_state_takes_no_snapshot() {
        let mut fake = Fake::ok();
        let swapped = run(&mut fake, false);
        assert_eq!(
            fake.steps,
            [
                "standby",
                "stop-old",
                "reset(undra dev --no-keep-state)",
                "listen"
            ]
        );
        assert_eq!(
            outcome(swapped),
            Outcome::Reset("undra dev --no-keep-state".into())
        );
    }

    #[test]
    fn a_failed_snapshot_still_swaps_with_fresh_state() {
        let mut fake = Fake::ok();
        fake.snapshot = Err(over_the_limit());
        let swapped = run(&mut fake, true);
        assert_eq!(
            fake.steps,
            [
                "standby",
                "snapshot",
                "stop-old",
                "reset(snapshot over 16 MiB)",
                "listen"
            ]
        );
        assert_eq!(
            outcome(swapped),
            Outcome::Reset("snapshot over 16 MiB".into())
        );
    }

    #[test]
    fn a_refused_restore_is_fresh_state_and_the_core_still_listens() {
        let mut fake = Fake::ok();
        fake.restore = Err("the core refused the snapshot: x".into());
        let swapped = run(&mut fake, true);
        assert_eq!(
            fake.steps,
            ["standby", "snapshot", "stop-old", "restore(0xaa)", "listen"],
            "the runner already told itself to start fresh; no reset command follows"
        );
        assert_eq!(
            outcome(swapped),
            Outcome::Reset("the core refused the snapshot: x".into())
        );
    }

    #[test]
    fn an_old_core_with_no_stores_has_nothing_to_carry() {
        let mut fake = Fake::ok();
        fake.snapshot = Ok(snapshot(0));
        let swapped = run(&mut fake, true);
        assert_eq!(fake.steps, ["standby", "snapshot", "stop-old", "listen"]);
        assert_eq!(outcome(swapped), Outcome::NothingToKeep);
    }

    #[test]
    fn a_core_that_cannot_listen_is_the_error_it_always_was() {
        let mut fake = Fake::ok();
        fake.listen_fails = true;
        assert!(swap(&mut fake, "old", "0xaa", true).is_err());
        assert_eq!(fake.steps.last().map(String::as_str), Some("listen"));
    }

    #[test]
    fn the_terminal_says_what_happened_to_the_state() {
        let kept = Outcome::Kept {
            restored: Restored {
                stores: 3,
                lost: 2,
                bytes: 209_008,
                micros: 189,
            },
            cancelled: 1,
            dropped: 2,
        };
        assert_eq!(
            kept.describe(),
            "state kept (3 stores, 205 KiB, restored in 189 \u{b5}s); 2 objects not carried over: their handles are stale, the app creates them again; 1 call still running when the core was replaced was cancelled; 2 calls sent during the reload were not run"
        );
        assert_eq!(
            Outcome::Reset("schema changed (was 0x1, now 0x2)".into()).describe(),
            "state reset: schema changed (was 0x1, now 0x2)"
        );
        assert_eq!(took(2_400), "2.4 ms");
        assert_eq!(kib(1536 * 1024), "1.5 MiB");
        assert_eq!(kib(1), "1 KiB");
    }
}
