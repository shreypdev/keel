//! `undra build`.

use crate::builds::{self, Options, Target};
use crate::cli::BuildArgs;
use crate::config::Platform;
use crate::error::Result;
use crate::sys::Os;

use super::Env;

/// Runs `undra build`.
///
/// # Errors
///
/// See [`builds::run`].
pub fn run(env: &Env<'_>, args: &BuildArgs) -> Result<()> {
    let session = env.session()?;
    let targets = match &args.platform {
        Some(list) => parse_targets(list)?,
        None => default_targets(&session.project.config.platforms, session.sys.os(), &env.ui),
    };
    let release = args.release
        || args
            .configuration
            .as_deref()
            .is_some_and(builds::xcode::is_release);
    let options = Options { targets, release };
    let artifacts = builds::run(&session, &options)?;
    if options.targets.contains(&Target::Ios) {
        // What Xcode's build phase watches (see `builds::xcode`): the inputs of the core as they are
        // now, and which configuration the XCFramework was built for.
        builds::xcode::refresh_inputs(&session.project.root, &session.core()?.local_dirs)?;
        if let Some(configuration) = args.configuration.as_deref() {
            builds::xcode::write_stamp(&session.project.build_dir(), configuration)?;
        }
    }
    env.ui.line("");
    env.ui.line(&env.ui.bold_out("Built:"));
    env.ui
        .line(builds::summary(&session.project.root, &artifacts).trim_end());
    for hint in builds::hints(&session, &options, &artifacts) {
        env.ui.hint(&hint);
    }
    Ok(())
}

/// Parses `--platform ios,web,host`.
fn parse_targets(list: &str) -> Result<Vec<Target>> {
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let target = Target::parse(part)?;
        if !out.contains(&target) {
            out.push(target);
        }
    }
    if out.is_empty() {
        return Err(crate::error::CliError::bad_argument(
            "the platform list is empty",
            "there is nothing to build",
            "name at least one of: host, ios, android, web, rn",
        ));
    }
    Ok(out)
}

/// Every platform of the project, except iOS when this is not a Mac (said, not failed: a Linux
/// CI job builds the rest).
fn default_targets(platforms: &[Platform], os: Os, ui: &crate::ui::Ui) -> Vec<Target> {
    let mut out = Vec::new();
    for platform in platforms {
        if *platform == Platform::Ios && os != Os::Macos {
            ui.warn("skipping iOS: it can only be built on macOS (`undra build --platform ios` says more)");
            continue;
        }
        out.push(Target::of(*platform));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_lists() {
        assert_eq!(
            parse_targets("web, ios,web").unwrap(),
            vec![Target::Web, Target::Ios]
        );
        assert!(parse_targets("").is_err());
        assert!(parse_targets("ios,tv").is_err());
    }

    #[test]
    fn ios_is_skipped_off_macos_by_default() {
        let ui = crate::ui::Ui::plain();
        let all = Platform::ALL;
        assert_eq!(
            default_targets(&all, Os::Linux, &ui),
            vec![Target::Android, Target::Web]
        );
        assert_eq!(
            default_targets(&all, Os::Macos, &ui),
            vec![Target::Ios, Target::Android, Target::Web]
        );
    }
}
