//! The React Native build: the iOS and Android builds `@undra/react-native` links, plus the
//! CocoaPods spec that vendors the XCFramework (ADR-038, decision 13).
//!
//! | Result below `build/` | Used by |
//! |---|---|
//! | `ios/UndraCore.xcframework` | the `UndraCore` pod (the iOS build of [`super::ios`]) |
//! | `ios/UndraCore.podspec` | the app's Podfile: `pod 'UndraCore', :path => '<build>/ios'` |
//! | `android/jniLibs/<abi>/libundra_core.so` | the app's Gradle `jniLibs`; the module `dlopen`s it |
//!
//! The pod links the static library with `-force_load`, for the reason the Xcode projects do: a
//! debug core is many object files, and the linker would drop the `#[undra::api]` registrations
//! nothing references (see [`super::ios`]).

use crate::error::Result;
use crate::fsutil::{size_of, write_if_changed};
use crate::session::Session;
use crate::sys::Os;

use super::{Artifact, android, ios};

/// Builds the iOS core (on macOS) and the Android core, and writes `build/ios/UndraCore.podspec`.
///
/// # Errors
///
/// What the iOS and Android builds return, and an I/O error writing the podspec.
pub fn build(session: &Session<'_>, release: bool) -> Result<Vec<Artifact>> {
    let mut artifacts = Vec::new();
    if session.sys.os() == Os::Macos {
        artifacts.extend(ios::build(session, release)?);
        let path = session.project.build_dir().join("ios/UndraCore.podspec");
        write_if_changed(
            &path,
            &podspec(
                &session.project.config.name,
                &session.project.config.undra_version,
                &session.project.config.ios.deployment_target,
            ),
        )?;
        artifacts.push(Artifact {
            label: "react native pod".to_owned(),
            size: size_of(&path),
            path,
            budget: None,
            note: Some("pod 'UndraCore', :path => '<build>/ios'".to_owned()),
        });
    } else {
        session.ui.warn(
            "skipping the iOS half of the React Native build: it can only be built on macOS (`undra build --platform ios` says more)",
        );
    }
    artifacts.extend(android::build(session, release)?);
    Ok(artifacts)
}

/// The `UndraCore` pod: the XCFramework next to it, force-loaded into the app.
#[must_use]
pub fn podspec(project: &str, version: &str, deployment_target: &str) -> String {
    format!(
        r#"# Written by `undra build --platform rn`; rewritten by every build, do not edit.
#
# The Undra core of `{project}` as a CocoaPods pod, for React Native apps that use @undra/react-native
# (docs/REACT_NATIVE.md). In the app's Podfile:
#
#   pod 'UndraCore', :path => '<this directory>'
#
# The static library is force-loaded: a debug core is many object files, and without it the linker
# drops the registrations nothing references, which leaves the schema empty.
Pod::Spec.new do |s|
  s.name                = "UndraCore"
  s.version             = "{version}"
  s.summary             = "The Undra core of {project}, built by undra build."
  s.homepage            = "https://github.com/shreypdev/undra"
  s.license             = {{ :type => "Proprietary", :text => "The app's own core." }}
  s.author              = "undra build"
  s.source              = {{ :http => "file://#{{__dir__}}/UndraCore.xcframework" }}
  s.platforms           = {{ :ios => "{deployment_target}" }}
  s.vendored_frameworks = "UndraCore.xcframework"
  s.user_target_xcconfig = {{
    "OTHER_LDFLAGS" => '$(inherited) -force_load "${{PODS_XCFRAMEWORKS_BUILD_DIR}}/UndraCore/libundra_core.a"',
  }}
end
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_podspec_vendors_and_force_loads_the_xcframework() {
        let spec = podspec("playground", "0.1", "17.0");
        assert!(spec.contains(r#"s.name                = "UndraCore""#));
        assert!(spec.contains(r#"s.vendored_frameworks = "UndraCore.xcframework""#));
        assert!(spec.contains(r#"{ :ios => "17.0" }"#));
        assert!(spec.contains(r#"-force_load "${PODS_XCFRAMEWORKS_BUILD_DIR}/UndraCore/libundra_core.a""#));
        assert!(spec.contains("`playground`"));
    }
}
