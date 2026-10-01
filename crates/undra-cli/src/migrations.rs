//! What changed for app authors between releases: the notes `undra upgrade` prints.
//!
//! One [`Migration`] per release that an app author has to know about, newest last. `undra upgrade`
//! prints the notes of every release it crosses (newer than the version the project was on, up to
//! the version of this `undra`), each note marked as something to do ([`Kind::Action`]), something
//! that behaves differently ([`Kind::Changed`]) or something new ([`Kind::New`]).
//!
//! **When a release is cut**, add its entry here (`docs/RELEASING.md`): an upgrade across a release
//! that has no entry prints nothing for it, which tells the author nothing changed.

use crate::semver::Semver;

/// What kind of thing a note is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The author has to do something (rebuild, add a `try`, edit a file).
    Action,
    /// Something behaves differently from before.
    Changed,
    /// Something new that existing projects do not get by themselves.
    New,
}

impl Kind {
    /// The marker printed in front of a note of this kind.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Kind::Action => "do",
            Kind::Changed => "changed",
            Kind::New => "new",
        }
    }
}

/// One thing an app author should know.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    /// What kind of thing it is.
    pub kind: Kind,
    /// The text: one paragraph, wrapped by the printer.
    pub text: &'static str,
}

/// The notes of one release.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// The release these notes arrive with (`1.1.0`).
    pub version: &'static str,
    /// A one-line summary.
    pub title: &'static str,
    /// The notes, in the order to read them.
    pub notes: &'static [Note],
}

impl Migration {
    /// The release as a [`Semver`].
    ///
    /// # Panics
    ///
    /// When the table has a malformed version (a test makes sure it has none).
    #[must_use]
    pub fn semver(&self) -> Semver {
        Semver::parse(self.version).expect("the migration table has only valid versions")
    }
}

/// Every release with notes, oldest first.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: "0.1.0",
    title: "Since v1.0: the UNDR wire, frame-coalesced delivery, the Swift error channel, reconnecting apps",
    notes: &[
        Note {
            kind: Kind::Action,
            text: "Rebuild the core and every app together. The envelope's four magic bytes are now `UNDR` (55 4E 44 52, ADR-033), where they were the working name's. A core and a runtime from either side of the change refuse each other's frames with a typed bad-magic error. `undra upgrade` moves every pin in step and regenerates the bindings, so one build of each is all it takes.",
        },
        Note {
            kind: Kind::Changed,
            text: "The platform mirrors apply what the core produces once per display frame, merged (ADR-031): a burst of transactions (a firehose of events, a stream, a burst of port completions) shows as one update per frame with its entries folded per signal, and the backlog is bounded (65,536 entries or 16 MiB, after which the store is observed again). A reply, `callSync` and `observe` are never delayed, so after `await store.method()` the mirror shows the change. A screen that needs every intermediate value of a signal sees the latest one per frame. `UndraCore.stats()` counts what was merged, and TypeScript has a drain listener.",
        },
        Note {
            kind: Kind::Action,
            text: "Swift: a generated call that returns a value or has an error type now `throws` (untyped, ADR-032): add `try`, and keep catching the method's own error (`catch let error as TodoError`). Two more things can arrive: `CancellationError` when the calling task was cancelled, and the new `UndraCallError` for the failure of the call itself (a panic in the core, a refusal, a core that was shut down, a lost connection, a reply that does not decode). A method that returns nothing and has no error type stays non-throwing: its failure goes to `LoadOptions.onError` and to the log. Generated Swift no longer traps on the outcome of a call.",
        },
        Note {
            kind: Kind::Changed,
            text: "Kotlin and TypeScript report failures through the exceptions and `onError` they already had (`UndraReplyException`, `UndraTransportError`, `onError` in TypeScript); only Swift has `UndraCallError` so far, and bringing the other two to the same closed set of failures is planned.",
        },
        Note {
            kind: Kind::New,
            text: "Apps reconnect to `undra dev` by themselves on all three platforms (ADR-051): backoff with jitter, every observed store observed again after the handshake, `core.connection` (TypeScript), `connectionState` (Kotlin, Swift) for a status bar, and the server keeps a dropped client's objects for ten minutes. A rebuild of the core still starts the app over on the new core; carrying state across a rebuild is planned. The Kotlin runtime has the WebSocket transport now, so Android runs against `undra dev` too (`./gradlew -PundraDevUrl=ws://10.0.2.2:7443 :app:installDebug`, `undra dev --android`).",
        },
        Note {
            kind: Kind::Changed,
            text: "`undra bindgen --docs` keeps the core's doc comments in the generated code: the schema a built core exports is the whole schema now (ADR-050). A core built before that change exports only the canonical form, and `--docs` on it is error C0006: rebuild the core.",
        },
        Note {
            kind: Kind::New,
            text: "A project made by `undra init` builds its core from the app's own build system (a Gradle `undraBuild` task, an Xcode \"Build the Undra core\" phase, the `undra()` Vite plugin) and has a CI workflow (`.github/workflows/undra.yml`). `undra upgrade` does not edit your app's project files: to get these in an older project copy the pieces from a fresh `undra init` (the Xcode project's build phase and `ios/Config/*.xcfilelist`, `android/app/build.gradle.kts`, `web/vite.config.ts`). `undra doctor --fix` prints the commands that close every gap of this machine as one block.",
        },
    ],
}];

/// The migrations a project crosses going from `from` (the version it is on; `None` when that is
/// not known) to `to`: every release newer than `from` and not newer than `to`, oldest first.
#[must_use]
pub fn between(from: Option<&Semver>, to: &Semver) -> Vec<&'static Migration> {
    MIGRATIONS
        .iter()
        .filter(|m| {
            let v = m.semver();
            v <= *to && from.is_none_or(|from| v > *from)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Semver {
        Semver::parse(text).unwrap()
    }

    #[test]
    fn the_table_is_valid_sorted_and_not_ahead_of_this_release() {
        let mut previous: Option<Semver> = None;
        for m in MIGRATIONS {
            let this = m.semver();
            assert!(
                previous.as_ref().is_none_or(|p| *p < this),
                "{} is out of order",
                m.version
            );
            assert!(!m.notes.is_empty() && !m.title.is_empty(), "{}", m.version);
            assert!(
                this <= v(env!("CARGO_PKG_VERSION")),
                "{} has notes for a release newer than this undra ({})",
                m.version,
                env!("CARGO_PKG_VERSION")
            );
            previous = Some(this);
        }
    }

    #[test]
    fn it_starts_with_an_entry_for_the_current_version() {
        let first = &MIGRATIONS[0];
        assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
        // What changed since v1.0, for app authors: the wire, the mirrors, the error channel, reconnecting.
        let text: String = first
            .notes
            .iter()
            .map(|n| n.text)
            .collect::<Vec<_>>()
            .join("\n");
        for needle in [
            "UNDR",
            "55 4E 44 52",
            "ADR-033",
            "ADR-031",
            "once per display frame",
            "ADR-032",
            "UndraCallError",
            "onError",
            "ADR-051",
            "reconnect",
            "undra upgrade",
        ] {
            assert!(
                text.contains(needle),
                "the first entry does not mention {needle}"
            );
        }
        assert!(
            first.notes.iter().any(|n| n.kind == Kind::Action),
            "it says what to do"
        );
    }

    #[test]
    fn a_project_gets_the_notes_of_the_releases_it_crosses() {
        let versions = |from: Option<&str>, to: &str| -> Vec<&'static str> {
            between(from.map(v).as_ref(), &v(to))
                .iter()
                .map(|m| m.version)
                .collect()
        };
        assert_eq!(versions(Some("0.0.9"), "0.1.0"), ["0.1.0"]);
        assert_eq!(versions(Some("0.0.9"), "1.0.0"), ["0.1.0"]);
        assert_eq!(
            versions(Some("0.1.0"), "1.0.0"),
            Vec::<&str>::new(),
            "already past it"
        );
        assert_eq!(
            versions(Some("0.0.1"), "0.0.9"),
            Vec::<&str>::new(),
            "not there yet"
        );
        assert_eq!(
            versions(None, "0.1.0"),
            ["0.1.0"],
            "a project of unknown version gets everything up to the target"
        );
    }

    #[test]
    fn markers_are_distinct_words() {
        let markers = [Kind::Action, Kind::Changed, Kind::New].map(Kind::marker);
        assert_eq!(markers, ["do", "changed", "new"]);
    }
}
