//! Shipping an update (ADR-037): data an older build persisted, read by a newer one.
//!
//! The playground is built twice by the contract runners: **build A** (the default) and
//! **build B** (`UNDRA_PLAYGROUND_V2=1`, or the `migration-v2` feature; `build.rs` turns either into
//! `cfg(playground_v2)`). Build B changes the types below the way an app update does:
//!
//! | Item | Build A | Build B | What happens to A's data in B |
//! |---|---|---|---|
//! | store [`Profile`] | `name: String`, `visits: u32` | `visits`, `name` (reordered), `theme: String` with `#[undra(default)]` | a snapshot restores by name; `theme` is `""` |
//! | store [`Legacy`] | `score: i32` | `score: String` | a snapshot holding one is refused: `RestoreError::Incompatible` (`undra_restore` code 7) |
//! | mutation [`save_note`] | `(list, text)` | `(list, text, pinned: Option<bool>)` | a queued call migrates (`pinned` is `None`) and replays |
//! | mutation [`tag_note`] | `(list, id: u32)` | `(list, id: String)` | a queued call is dead-lettered, never lost |
//! | query [`roster`] | `(team: u32)` | `(team: String)` | a snapshot's query handle is refused: its parameters changed (ADR-059) |
//!
//! Everything else is the same in both builds, and so are the names, so a runner drives build B
//! through the raw API with the ids it already knows. [`storage_status`] reports what the query
//! client's persistence did (ADR-037, ADR-049).

use undra::ports::HttpRequest;
use undra::prelude::*;

use crate::remote::{RemoteError, endpoint, send};

/// What the query client's persistence did so far: [`storage_status`].
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageStatus {
    /// Mutations waiting in the offline queue.
    pub pending: u32,
    /// The dead letters, as `"<mutation>: <reason>"`.
    pub dead_letters: Vec<String>,
    /// Whether the stored offline queue has been read (`false` while the store cannot be read:
    /// the queue is then neither replayed nor written).
    pub queue_readable: bool,
    /// Writes to the `Kv` store that failed.
    pub write_failed: u64,
    /// Persisted cache entries dropped because they could not be migrated.
    pub dropped: u64,
    /// Cache entries and queued mutations an older build wrote that were migrated.
    pub migrated: u64,
    /// Queued mutations moved to the dead-letter queue.
    pub dead_lettered: u64,
}

/// What the query client's persistence did: the offline queue, its dead letters and the
/// persistence counters.
#[undra::api]
pub fn storage_status(ctx: &Ctx) -> StorageStatus {
    let query = ctx.query();
    let stats = query.persist_stats();
    StorageStatus {
        pending: u32::try_from(query.pending_mutations()).unwrap_or(u32::MAX),
        dead_letters: query
            .dead_letters()
            .into_iter()
            .map(|d| format!("{}: {}", d.mutation, d.reason))
            .collect(),
        queue_readable: stats.queue_readable,
        write_failed: stats.write_failed,
        dropped: stats.dropped,
        migrated: stats.migrated,
        dead_lettered: stats.dead_lettered,
    }
}

/// Posts a note: `POST {base}/lists/{list}/notes` with the idempotency key. `true` on a 2xx.
async fn post_note(ctx: &Ctx, list: &str, body: String) -> Result<bool, RemoteError> {
    let url = endpoint(ctx, &format!("/lists/{list}/notes"))?;
    let mut request = HttpRequest::post(url, body.into_bytes());
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    send(ctx, request).await.map(|_| true)
}

#[cfg(not(playground_v2))]
mod build {
    use super::*;

    /// A user profile: build A's signals are `name`, then `visits`.
    #[undra::store]
    pub struct Profile {
        name: Signal<String>,
        visits: Signal<u32>,
    }

    #[undra::api(store)]
    impl Profile {
        /// A profile called `name`, never visited.
        pub fn new(name: String) -> Self {
            Profile {
                name: Signal::new(name),
                visits: Signal::new(0),
            }
        }

        /// Counts one visit.
        pub fn visit(&self) {
            self.visits.update(|v| *v += 1);
        }

        /// `name=..;visits=..`: what both builds can be compared by.
        pub fn describe(&self) -> String {
            format!("name={};visits={}", self.name.get(), self.visits.get())
        }
    }

    /// A store whose one signal build B changes incompatibly.
    #[undra::store]
    pub struct Legacy {
        score: Signal<i32>,
    }

    #[undra::api(store)]
    impl Legacy {
        /// A score of `score`.
        pub fn new(score: i32) -> Self {
            Legacy {
                score: Signal::new(score),
            }
        }

        /// `score=..`.
        pub fn describe(&self) -> String {
            format!("score={}", self.score.get())
        }
    }

    /// Saves a note in `list`; idempotent, so it waits in the offline queue while offline.
    #[undra::mutation(key = "notes:{list}", idempotent)]
    pub async fn save_note(ctx: &Ctx, list: String, text: String) -> Result<bool, RemoteError> {
        post_note(ctx, &list, format!("save:{text}")).await
    }

    /// Tags note `id` of `list`; idempotent.
    #[undra::mutation(key = "notes:{list}", idempotent)]
    pub async fn tag_note(ctx: &Ctx, list: String, id: u32) -> Result<bool, RemoteError> {
        post_note(ctx, &list, format!("tag:{id}")).await
    }

    /// The members of team `team`, which build A names by number. A query handle for it is what a
    /// snapshot of build A carries into build B (ADR-059), which changes the parameter's type.
    #[undra::query(key = "roster:{team}", stale = "30s")]
    pub async fn roster(_ctx: &Ctx, team: u32) -> Result<Vec<String>, RemoteError> {
        Ok(vec![format!("team {team}")])
    }
}

#[cfg(playground_v2)]
mod build {
    use super::*;

    /// A user profile: build B reordered the signals and added `theme` with a default.
    #[undra::store]
    pub struct Profile {
        visits: Signal<u32>,
        name: Signal<String>,
        #[undra(default)]
        theme: Signal<String>,
    }

    #[undra::api(store)]
    impl Profile {
        /// A profile called `name`, never visited, with no theme.
        pub fn new(name: String) -> Self {
            Profile {
                visits: Signal::new(0),
                name: Signal::new(name),
                theme: Signal::new(String::new()),
            }
        }

        /// Counts one visit.
        pub fn visit(&self) {
            self.visits.update(|v| *v += 1);
        }

        /// `name=..;visits=..;theme=..`.
        pub fn describe(&self) -> String {
            format!(
                "name={};visits={};theme={}",
                self.name.get(),
                self.visits.get(),
                self.theme.get()
            )
        }
    }

    /// Build B made the score a `String`: not structural, and no hook converts it.
    #[undra::store]
    pub struct Legacy {
        score: Signal<String>,
    }

    #[undra::api(store)]
    impl Legacy {
        /// A score of `score`.
        pub fn new(score: i32) -> Self {
            Legacy {
                score: Signal::new(score.to_string()),
            }
        }

        /// `score=..`.
        pub fn describe(&self) -> String {
            format!("score={}", self.score.get())
        }
    }

    /// Saves a note; build B added `pinned` (an `Option`: structural).
    #[undra::mutation(key = "notes:{list}", idempotent)]
    pub async fn save_note(
        ctx: &Ctx,
        list: String,
        text: String,
        pinned: Option<bool>,
    ) -> Result<bool, RemoteError> {
        let pin = if pinned == Some(true) { ":pinned" } else { "" };
        post_note(ctx, &list, format!("save:{text}{pin}")).await
    }

    /// Tags a note; build B made the id a `String` (not structural: a queued call from build A is
    /// dead-lettered).
    #[undra::mutation(key = "notes:{list}", idempotent)]
    pub async fn tag_note(ctx: &Ctx, list: String, id: String) -> Result<bool, RemoteError> {
        post_note(ctx, &list, format!("tag:{id}")).await
    }

    /// The members of team `team`; build B names a team by a `String`, so build A's query handles
    /// for it are refused when a snapshot is restored (ADR-059).
    #[undra::query(key = "roster:{team}", stale = "30s")]
    pub async fn roster(_ctx: &Ctx, team: String) -> Result<Vec<String>, RemoteError> {
        Ok(vec![format!("team {team}")])
    }
}

pub use build::{
    Legacy, Profile, RosterQuery, SaveNoteMutation, TagNoteMutation, roster, save_note, tag_note,
};

/// The app's own synchronous port (ADR-049 decision 2): how the platform says hello in the
/// user's language. In the TypeScript `wasm-worker` mode a synchronous port is answered in the
/// worker, so the web app registers it in `worker.ports`.
#[undra::port(sync)]
pub trait Locale {
    /// "Hello" in the user's language.
    fn hello(&self) -> String;
}

/// Greets `name` in the user's language, through the [`Locale`] port.
#[undra::api]
pub fn localized_greeting(ctx: &Ctx, name: String) -> String {
    format!("{}, {name}", locale(ctx).hello())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use undra::ports::fakes;
    use undra::runtime::testing::TestRuntime;

    use super::*;

    struct French;

    #[undra::port]
    impl Locale for French {
        fn hello(&self) -> String {
            "Bonjour".to_owned()
        }
    }

    #[test]
    fn the_greeting_asks_the_locale_port() {
        let t = TestRuntime::new();
        t.runtime().bind_dyn_port::<dyn Locale>(
            <dyn Locale as undra::runtime::Port>::PORT_ID,
            Arc::new(French),
        );
        assert_eq!(localized_greeting(&t.ctx(), "Ada".into()), "Bonjour, Ada");
    }

    #[test]
    fn a_fresh_client_reports_an_empty_readable_queue() {
        let t = TestRuntime::new();
        let _fakes = fakes::install(&t);
        t.run_init_hooks();
        t.run_pending();
        let status = storage_status(&t.ctx());
        assert_eq!(status.pending, 0);
        assert!(status.dead_letters.is_empty());
        assert!(status.queue_readable);
        assert_eq!(
            (
                status.write_failed,
                status.dropped,
                status.migrated,
                status.dead_lettered
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn profiles_count_visits() {
        let profile = Profile::new("Ada".into());
        profile.visit();
        assert!(profile.describe().starts_with("name=Ada;visits=1"));
        assert!(Legacy::new(3).describe().starts_with("score=3"));
    }
}
