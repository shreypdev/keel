//! The web checks: the wasm target (see [`super::rust`]), Node, npm and `wasm-opt`.

use crate::sys::Os;

use super::finding::{Check, Finding, State};
use super::rust::target;
use super::{Context, brew, homebrew_off_path, version_line};

/// Node, 20 or newer.
pub const NODE: Check = Check::new("web.node", "node-and-npm");
/// npm, which installs the web app's dependencies.
pub const NPM: Check = Check::new("web.npm", "node-and-npm").optional();
/// `wasm-opt` (binaryen): optional, shrinks the wasm core.
pub const WASM_OPT: Check = Check::new("web.wasm-opt", "wasm-opt").optional();

/// Every check of this module (the wasm target is in [`super::rust::ALL`]).
#[cfg(test)]
pub const ALL: &[Check] = &[NODE, NPM, WASM_OPT];

/// The oldest Node that runs the web app shell and the TypeScript runtime (`engines`).
pub const MIN_NODE: u32 = 20;

/// `v22.3.0` into `22`.
#[must_use]
pub fn node_major(line: &str) -> Option<u32> {
    line.trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn install_node(cx: &Context<'_>) -> Vec<String> {
    if cx.sys.os() == Os::Macos {
        brew(cx, "install node")
    } else {
        // fnm's installer adds fnm to the shell's startup file; this shell gets it from its directory.
        vec![
            "curl -fsSL https://fnm.vercel.app/install | bash".to_owned(),
            "export PATH=\"$HOME/.local/share/fnm:$PATH\" && eval \"$(fnm env)\"".to_owned(),
            "fnm install --lts".to_owned(),
        ]
    }
}

/// The finding of a program that is not on `PATH`: installed by Homebrew in a directory this shell
/// does not reach (then the fix puts it on `PATH`), else not installed (then `install`).
fn not_on_path(
    cx: &Context<'_>,
    check: &Check,
    program: &str,
    message: &str,
    install: &[String],
    warn_only: bool,
) -> Finding {
    if let Some((found, fix)) = homebrew_off_path(cx, program) {
        let message = format!(
            "{program} is installed at {}, but Homebrew's directory is not on this shell's PATH",
            found.display()
        );
        return if warn_only || check.optional {
            check.warn(
                State::Missing,
                Some(found.display().to_string()),
                message,
                &[&fix],
            )
        } else {
            check.fail(
                State::Missing,
                Some(found.display().to_string()),
                message,
                &[&fix],
            )
        };
    }
    let install: Vec<&str> = install.iter().map(String::as_str).collect();
    if warn_only {
        check.warn_missing(message, &install)
    } else {
        check.missing(message, &install)
    }
}

/// Every web finding.
#[must_use]
pub fn check(cx: &Context<'_>) -> Vec<Finding> {
    let mut out = vec![target(cx, "wasm32-unknown-unknown", "the web build", false)];
    let install_list = install_node(cx);
    let install: Vec<&str> = install_list.iter().map(String::as_str).collect();
    match cx.toolchain.which(cx.sys, "node") {
        None => out.push(not_on_path(
            cx,
            &NODE,
            "node",
            &format!("node was not found (the web app shell and the TypeScript runtime need Node {MIN_NODE}+)"),
            &install_list,
            false,
        )),
        Some(node) => match version_line(cx, &node, &["--version"]) {
            Some(line) => match node_major(&line) {
                Some(major) if major >= MIN_NODE => out.push(NODE.ok_with(format!("node {line}"), line)),
                Some(_) => out.push(NODE.wrong_version(
                    line.clone(),
                    format!("node {line} is too old (Undra needs {MIN_NODE}+)"),
                    &install,
                )),
                None => out.push(NODE.ok_with(format!("node {line}"), line)),
            },
            None => out.push(NODE.fail(State::Missing, None, "node is installed but does not run", &install)),
        },
    }
    match cx.toolchain.which(cx.sys, "npm") {
        Some(npm) => {
            let version = version_line(cx, &npm, &["--version"]).unwrap_or_default();
            out.push(NPM.ok_with(format!("npm {version}").trim().to_owned(), version));
        }
        None => out.push(not_on_path(
            cx,
            &NPM,
            "npm",
            "npm was not found (the web app shell installs its dependencies with it)",
            &install_list,
            true,
        )),
    }
    match cx.toolchain.which(cx.sys, "wasm-opt") {
        Some(tool) => {
            let version =
                version_line(cx, &tool, &["--version"]).unwrap_or_else(|| "wasm-opt".to_owned());
            out.push(WASM_OPT.ok(version));
        }
        None => {
            let install = if cx.sys.os() == Os::Macos {
                brew(cx, "install binaryen")
            } else {
                vec!["sudo apt-get install -y binaryen".to_owned()]
            };
            out.push(not_on_path(
                cx,
                &WASM_OPT,
                "wasm-opt",
                "wasm-opt (binaryen) was not found. It is optional: `undra build --platform web` works without it, and with it the wasm core is 10-20% smaller",
                &install,
                true,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;
    use crate::commands::doctor::finding::Status;

    #[test]
    fn node_versions_are_read() {
        assert_eq!(node_major("v22.3.0"), Some(22));
        assert_eq!(node_major("24.0.0\n"), Some(24));
        assert_eq!(node_major("nonsense"), None);
    }

    #[test]
    fn the_wasm_target_is_needed_for_the_web_build() {
        let f = by_id(
            &scan(&good_machine(), &["web"]),
            "rust.target.wasm32-unknown-unknown",
        )
        .clone();
        assert_eq!(f.status, Status::Ok);
        let report = scan(&good_machine_without_targets(), &["web"]);
        let f = by_id(&report, "rust.target.wasm32-unknown-unknown");
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        assert_eq!(f.fix, ["rustup target add wasm32-unknown-unknown"]);
    }

    #[test]
    fn node_must_be_20_or_newer() {
        let f = by_id(&scan(&good_machine(), &["web"]), "web.node").clone();
        assert_eq!(f.status, Status::Ok);
        assert_eq!(f.observed.as_deref(), Some("v22.3.0"));

        let old = good_machine().with_output("node", "--version", "v18.19.0\n");
        let f = by_id(&scan(&old, &["web"]), "web.node").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::WrongVersion));
        assert_eq!(f.fix, ["brew install node"]);

        let f = by_id(&scan(&bare_mac(), &["web"]), "web.node").clone();
        assert_eq!((f.status, f.state), (Status::Fail, State::Missing));
        let f = by_id(&scan(&bare_machine(), &["web"]), "web.node").clone();
        assert_eq!(f.fix[2], "fnm install --lts");
        assert!(
            f.fix[1].contains("fnm env"),
            "fnm is on this shell's PATH first: {f:?}"
        );

        let broken = good_machine().with_failing_output("node", "--version", "dyld error");
        assert_eq!(
            by_id(&scan(&broken, &["web"]), "web.node").status,
            Status::Fail
        );
    }

    #[test]
    fn npm_is_checked() {
        let f = by_id(&scan(&good_machine(), &["web"]), "web.npm").clone();
        assert_eq!(f.status, Status::Ok);
        assert_eq!(f.observed.as_deref(), Some("10.8.2"));
        let f = by_id(&scan(&bare_mac(), &["web"]), "web.npm").clone();
        assert_eq!((f.status, f.state), (Status::Warn, State::Missing));
    }

    #[test]
    fn wasm_opt_is_optional_and_says_what_it_buys() {
        let f = by_id(&scan(&good_machine(), &["web"]), "web.wasm-opt").clone();
        assert_eq!(f.status, Status::Ok);
        assert!(f.message.starts_with("wasm-opt version"), "{f:?}");

        let f = by_id(&scan(&bare_mac_with_brew(), &["web"]), "web.wasm-opt").clone();
        assert_eq!(
            (f.status, f.state, f.optional),
            (Status::Warn, State::Missing, true)
        );
        assert!(
            f.message.contains("optional") && f.message.contains("10-20% smaller"),
            "{f:?}"
        );
        assert_eq!(f.fix, ["brew install binaryen"]);
        let f = by_id(&scan(&bare_machine(), &["web"]), "web.wasm-opt").clone();
        assert_eq!(f.fix, ["sudo apt-get install -y binaryen"]);
    }
}
