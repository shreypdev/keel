//! What a doctor check reports: a [`Finding`], and the [`Check`] that makes it.
//!
//! Every finding carries the same five things, so a person and a tool read it the same way:
//!
//! | Field | Meaning |
//! |---|---|
//! | `state` | what was found: `ok`, `missing`, `wrong-version` or `not-applicable` |
//! | `status` | how much it matters: [`Status::Ok`], [`Status::Warn`] (builds still work), [`Status::Fail`] (a build in scope cannot work), [`Status::Skip`] |
//! | `observed` | the value that was seen (a version line, a path, a device list), when there is one |
//! | `fix` | the exact commands that close the gap, one per line, ready to paste |
//! | `anchor` | the heading of `docs/ONBOARDING.md` that explains the prerequisite |

use std::fmt::Write as _;

use serde_json::{Value, json};

/// Where the anchors point: the onboarding guide in the repository (the only place a binary
/// installed from a release can send a reader to).
pub const DOCS_BASE: &str = "https://github.com/shreypdev/undra/blob/main/docs/ONBOARDING.md";

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Present and fine.
    Ok,
    /// Something is off but builds still work.
    Warn,
    /// A build for a platform in scope cannot work.
    Fail,
    /// Not applicable on this machine.
    Skip,
}

impl Status {
    /// The word used in the JSON report.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Skip => "skip",
        }
    }
}

/// What a check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Present, at a version that works.
    Ok,
    /// Not there.
    Missing,
    /// There, at a version that does not work.
    WrongVersion,
    /// The check does not apply to this machine or project.
    NotApplicable,
}

impl State {
    /// The word used in the JSON report.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            State::Ok => "ok",
            State::Missing => "missing",
            State::WrongVersion => "wrong-version",
            State::NotApplicable => "not-applicable",
        }
    }
}

/// Who a check is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    /// Anyone who builds an Undra app.
    Everyone,
    /// People who work on Undra itself (running the Kotlin runtime tests, the `undra` emulator).
    Contributors,
}

impl Audience {
    /// The word used in the JSON report.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Audience::Everyone => "everyone",
            Audience::Contributors => "contributors",
        }
    }
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The stable name of the check (`rust.rustc`, `android.ndk`).
    pub id: &'static str,
    /// How much it matters.
    pub status: Status,
    /// What was found.
    pub state: State,
    /// Who the check is for.
    pub audience: Audience,
    /// Whether the thing is optional (a missing optional tool is a warning, never a failure).
    pub optional: bool,
    /// What was checked and what was found, in a sentence.
    pub message: String,
    /// The value that was observed, when there is one.
    pub observed: Option<String>,
    /// The commands that close the gap, one per entry; empty when nothing needs doing.
    pub fix: Vec<String>,
    /// The heading of `docs/ONBOARDING.md` that explains this prerequisite (a GitHub anchor).
    pub anchor: &'static str,
}

impl Finding {
    /// The URL of the documentation of this finding.
    #[must_use]
    pub fn docs_url(&self) -> String {
        format!("{DOCS_BASE}#{}", self.anchor)
    }

    /// Whether this finding asks the reader to do something.
    #[must_use]
    pub fn needs_fix(&self) -> bool {
        matches!(self.status, Status::Warn | Status::Fail)
    }

    /// The finding as a JSON object (see [`Report::to_json`]).
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "status": self.state.word(),
            "severity": self.status.word(),
            "audience": self.audience.word(),
            "optional": self.optional,
            "message": self.message,
            "observed": self.observed,
            "fix": self.fix,
            "docs": format!("docs/ONBOARDING.md#{}", self.anchor),
            "docs_url": self.docs_url(),
        })
    }
}

/// A check: its name, where it is documented, and who it is for. It makes the [`Finding`]s.
#[derive(Clone, Copy, Debug)]
pub struct Check {
    /// The stable name (`rust.rustc`).
    pub id: &'static str,
    /// The heading of `docs/ONBOARDING.md` that documents it.
    pub anchor: &'static str,
    /// Who it is for.
    pub audience: Audience,
    /// Whether the thing is optional.
    pub optional: bool,
}

impl Check {
    /// A check for everyone.
    #[must_use]
    pub const fn new(id: &'static str, anchor: &'static str) -> Check {
        Check {
            id,
            anchor,
            audience: Audience::Everyone,
            optional: false,
        }
    }

    /// Marks the check as one for contributors to Undra.
    #[must_use]
    pub const fn for_contributors(mut self) -> Check {
        self.audience = Audience::Contributors;
        self.optional = true;
        self
    }

    /// Marks the thing as optional.
    #[must_use]
    pub const fn optional(mut self) -> Check {
        self.optional = true;
        self
    }

    fn make(
        &self,
        status: Status,
        state: State,
        message: String,
        observed: Option<String>,
        fix: &[&str],
    ) -> Finding {
        Finding {
            id: self.id,
            status,
            state,
            audience: self.audience,
            optional: self.optional,
            message,
            observed,
            fix: fix.iter().map(|s| (*s).to_owned()).collect(),
            anchor: self.anchor,
        }
    }

    /// Found, and fine: the observed value is the message.
    #[must_use]
    pub fn ok(&self, observed: impl Into<String>) -> Finding {
        let observed = observed.into();
        self.make(Status::Ok, State::Ok, observed.clone(), Some(observed), &[])
    }

    /// Found, and fine, with a sentence that says more than the observed value.
    #[must_use]
    pub fn ok_with(&self, message: impl Into<String>, observed: impl Into<String>) -> Finding {
        self.make(
            Status::Ok,
            State::Ok,
            message.into(),
            Some(observed.into()),
            &[],
        )
    }

    /// Not there. A failure, unless the thing is optional.
    #[must_use]
    pub fn missing(&self, message: impl Into<String>, fix: &[&str]) -> Finding {
        let status = if self.optional {
            Status::Warn
        } else {
            Status::Fail
        };
        self.make(status, State::Missing, message.into(), None, fix)
    }

    /// Not there, and it only warrants a warning.
    #[must_use]
    pub fn warn_missing(&self, message: impl Into<String>, fix: &[&str]) -> Finding {
        self.make(Status::Warn, State::Missing, message.into(), None, fix)
    }

    /// There, at a version that does not work. A failure, unless the thing is optional.
    #[must_use]
    pub fn wrong_version(
        &self,
        observed: impl Into<String>,
        message: impl Into<String>,
        fix: &[&str],
    ) -> Finding {
        let status = if self.optional {
            Status::Warn
        } else {
            Status::Fail
        };
        self.make(
            status,
            State::WrongVersion,
            message.into(),
            Some(observed.into()),
            fix,
        )
    }

    /// There, at a version that is worth a warning.
    #[must_use]
    pub fn warn_version(
        &self,
        observed: impl Into<String>,
        message: impl Into<String>,
        fix: &[&str],
    ) -> Finding {
        self.make(
            Status::Warn,
            State::WrongVersion,
            message.into(),
            Some(observed.into()),
            fix,
        )
    }

    /// Present but not in the state the build wants (a variable that is not set, a tool that is
    /// not on `PATH`): a warning with the observed value.
    #[must_use]
    pub fn warn(
        &self,
        state: State,
        observed: Option<String>,
        message: impl Into<String>,
        fix: &[&str],
    ) -> Finding {
        self.make(Status::Warn, state, message.into(), observed, fix)
    }

    /// A failure with the observed value.
    #[must_use]
    pub fn fail(
        &self,
        state: State,
        observed: Option<String>,
        message: impl Into<String>,
        fix: &[&str],
    ) -> Finding {
        self.make(Status::Fail, state, message.into(), observed, fix)
    }

    /// Does not apply here.
    #[must_use]
    pub fn skip(&self, message: impl Into<String>) -> Finding {
        self.make(
            Status::Skip,
            State::NotApplicable,
            message.into(),
            None,
            &[],
        )
    }
}

/// A titled group of findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// `Rust`, `iOS`, ...
    pub title: &'static str,
    /// The findings.
    pub findings: Vec<Finding>,
}

/// Everything `undra doctor` found, and what it was looking at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The groups, in the order they are printed.
    pub sections: Vec<Section>,
}

impl Report {
    /// Every finding, in order.
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.sections.iter().flat_map(|s| &s.findings)
    }

    /// Whether nothing the project needs is missing.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.findings().all(|f| f.status != Status::Fail)
    }

    /// How many findings have each status, in the order ok, warn, fail, skip.
    #[must_use]
    pub fn counts(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for f in self.findings() {
            counts[f.status as usize] += 1;
        }
        counts
    }

    /// The commands that close every gap, as one block that can be pasted into a shell: a
    /// comment says which finding each group of commands is for, repeated commands appear once,
    /// and `rustup target add` lines are folded into one. Nothing here is ever run by `undra`.
    #[must_use]
    pub fn fix_script(&self) -> String {
        let mut out = String::from(
            "# undra doctor --fix: commands that close the gaps below. Nothing has been run;\n# read them, then paste the block into a shell and run `undra doctor` again.\n",
        );
        let mut seen: Vec<String> = Vec::new();
        let mut targets: Vec<String> = Vec::new();
        let mut body = String::new();
        for f in self
            .findings()
            .filter(|f| f.needs_fix() && !f.fix.is_empty())
        {
            let mut commands = String::new();
            for line in &f.fix {
                if let Some(triple) = line.strip_prefix("rustup target add ") {
                    for t in triple.split_whitespace() {
                        if !targets.iter().any(|known| known == t) {
                            targets.push(t.to_owned());
                        }
                    }
                    continue;
                }
                if seen.iter().any(|known| known == line) {
                    continue;
                }
                seen.push(line.clone());
                commands.push_str(line);
                commands.push('\n');
            }
            if commands.is_empty() {
                continue;
            }
            let _ = writeln!(body, "\n# {}", f.message.lines().next().unwrap_or(""));
            body.push_str(&commands);
        }
        if !targets.is_empty() {
            let _ = write!(
                out,
                "\n# Rust targets the platforms in scope need\nrustup target add {}\n",
                targets.join(" ")
            );
        }
        out.push_str(&body);
        if targets.is_empty() && body.is_empty() {
            out.push_str("\n# nothing to fix\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECK: Check = Check::new("demo.tool", "demo-tool");

    #[test]
    fn a_missing_required_thing_fails_and_a_missing_optional_one_warns() {
        let required = CHECK.missing("tool is missing", &["install tool"]);
        assert_eq!(
            (required.status, required.state),
            (Status::Fail, State::Missing)
        );
        let optional = CHECK
            .optional()
            .missing("tool is missing", &["install tool"]);
        assert_eq!(optional.status, Status::Warn);
        let contributor = CHECK.for_contributors().missing("tool is missing", &[]);
        assert_eq!(contributor.status, Status::Warn);
        assert_eq!(contributor.audience, Audience::Contributors);
    }

    #[test]
    fn a_finding_names_its_documentation() {
        let f = CHECK.ok("tool 1.2");
        assert_eq!(f.docs_url(), format!("{DOCS_BASE}#demo-tool"));
        let json = f.to_json();
        assert_eq!(json["docs"], "docs/ONBOARDING.md#demo-tool");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["observed"], "tool 1.2");
        assert_eq!(json["fix"], json!([]));
    }

    #[test]
    fn the_fix_script_folds_targets_and_drops_repeats() {
        let report = Report {
            sections: vec![Section {
                title: "T",
                findings: vec![
                    CHECK.missing("first gap", &["rustup target add a", "install x"]),
                    CHECK.missing(
                        "second gap",
                        &["rustup target add b", "install x", "install y"],
                    ),
                    CHECK.ok("fine"),
                ],
            }],
        };
        let script = report.fix_script();
        assert!(script.contains("rustup target add a b\n"), "{script}");
        assert_eq!(script.matches("install x").count(), 1, "{script}");
        assert!(script.contains("# first gap\ninstall x\n"), "{script}");
        assert!(script.contains("# second gap\ninstall y\n"), "{script}");
        assert!(script.starts_with("# undra doctor --fix"), "{script}");
    }

    #[test]
    fn nothing_to_fix_says_so() {
        let report = Report {
            sections: vec![Section {
                title: "T",
                findings: vec![CHECK.ok("fine")],
            }],
        };
        assert!(report.fix_script().contains("# nothing to fix"));
        assert!(report.passed());
        assert_eq!(report.counts(), [1, 0, 0, 0]);
    }
}
