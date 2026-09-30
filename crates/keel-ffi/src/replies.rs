//! Did the host answer *this* port call before its `port_call` import returned? (wasm, SPEC 7)
//!
//! A wasm host answers a synchronous port call by calling `keel_port_reply` from inside the
//! `port_call` import and then returning `0`. The shim has to tell that from a host that returns
//! `0` without having answered, which would leave the call pending forever. Looking at *any*
//! reply is not enough: a host that answers a different, earlier call from inside this import and
//! returns `0` for this one has not answered this one (review L2). So the shim records the ids of
//! the replies that arrive while an import runs and asks about the one id.

use std::sync::{Mutex, MutexGuard, PoisonError};

#[derive(Default)]
struct State {
    /// `port_call` imports running (they nest if the host re-enters the core).
    depth: usize,
    /// The port call ids of the replies received while any import was running.
    replied: Vec<u32>,
}

/// The replies received during `port_call` imports.
pub(crate) struct ReplyWatch {
    state: Mutex<State>,
}

impl ReplyWatch {
    /// An empty watch.
    pub(crate) const fn new() -> ReplyWatch {
        ReplyWatch {
            state: Mutex::new(State {
                depth: 0,
                replied: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `keel_port_reply` received a well-formed reply for `port_call_id`. Only remembered while a
    /// `port_call` import is running: nobody asks about the others.
    pub(crate) fn note(&self, port_call_id: u32) {
        let mut state = self.lock();
        if state.depth > 0 {
            state.replied.push(port_call_id);
        }
    }

    /// Runs `host_call` (the import for `port_call_id`) and returns its result together with
    /// whether a reply for exactly `port_call_id` arrived while it ran.
    pub(crate) fn during<R>(&self, port_call_id: u32, host_call: impl FnOnce() -> R) -> (R, bool) {
        let mark = {
            let mut state = self.lock();
            state.depth += 1;
            state.replied.len()
        };
        let result = host_call();
        let mut state = self.lock();
        state.depth -= 1;
        let replied = state.replied[mark..].contains(&port_call_id);
        state.replied.truncate(mark);
        (result, replied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_reply_for_the_asked_call_counts() {
        let watch = ReplyWatch::new();
        let (answer, replied) = watch.during(7, || {
            watch.note(3); // another call, answered meanwhile
            0
        });
        assert_eq!((answer, replied), (0, false));
        let (_, replied) = watch.during(7, || {
            watch.note(3);
            watch.note(7);
        });
        assert!(replied);
    }

    #[test]
    fn replies_outside_an_import_are_not_remembered() {
        let watch = ReplyWatch::new();
        watch.note(7);
        let (_, replied) = watch.during(7, || {});
        assert!(!replied, "a reply from before the import is not its answer");
        assert!(watch.lock().replied.is_empty());
    }

    #[test]
    fn nested_imports_keep_their_own_answers() {
        let watch = ReplyWatch::new();
        let (inner, outer_replied) = watch.during(1, || {
            let (_, inner_replied) = watch.during(2, || watch.note(2));
            watch.note(9);
            inner_replied
        });
        assert!(inner, "the inner call was answered");
        assert!(
            !outer_replied,
            "the outer call 1 was not, whatever happened inside"
        );
        assert_eq!(watch.lock().depth, 0);
        assert!(watch.lock().replied.is_empty());
    }
}
