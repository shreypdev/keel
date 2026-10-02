//! The React Native build: the iOS and Android builds `@undra/react-native` links, plus the
//! CocoaPods spec that vendors the XCFramework (ADR-038, decision 13; ADR-044).
//!
//! | Result below `build/` | Used by |
//! |---|---|
//! | `ios/<Namespace>Core.xcframework` | the `<Namespace>Core` pod (the iOS build of [`super::ios`]) |
//! | `ios/<Namespace>Core.podspec`, `ios/<Namespace>CoreTable.m` | the app's Podfile: `pod '<Namespace>Core', :path => '<build>/ios'` |
//! | `android/jniLibs/<abi>/lib<namespace>.so` | the app's Gradle `jniLibs`; the module `dlopen`s it |
//!
//! The pod needs no `-force_load` any more: each slice is one prelinked object whose entry pulls
//! every registration (ADR-044). It also compiles one generated Objective-C class,
//! `UndraCoreTable_<namespace>`, whose `+api` returns the core's table: `@undra/react-native` is
//! built once for every core of the app, so on iOS it finds a core by its namespace
//! (`NSClassFromString`) at the moment JavaScript loads it, instead of naming a symbol it cannot
//! know when it is compiled. On Android it `dlopen`s `lib<namespace>.so` and looks up
//! `<namespace>_undra_api`.

use crate::error::Result;
use crate::fsutil::{size_of, write_if_changed};
use crate::session::Session;
use crate::symbols::Symbols;
use crate::sys::Os;
use undra_bindgen::naming::CoreNames;

use super::{Artifact, android, ios};

/// Builds the iOS core (on macOS) and the Android core, and writes `build/ios/<Bundle>.podspec`
/// with the table class it compiles.
///
/// # Errors
///
/// What the iOS and Android builds return, and an I/O error writing the pod's files.
pub fn build(
    session: &Session<'_>,
    release: bool,
    symbols: &Symbols<'_, '_>,
) -> Result<Vec<Artifact>> {
    let mut artifacts = Vec::new();
    if session.sys.os() == Os::Macos {
        artifacts.extend(ios::build(session, release, symbols)?);
        let names = session.core_names()?;
        let dir = session.project.build_dir().join("ios");
        write_if_changed(
            &dir.join(format!("{}Table.m", names.bundle())),
            &table_class(&names),
        )?;
        let path = dir.join(format!("{}.podspec", names.bundle()));
        write_if_changed(
            &path,
            &podspec(
                &session.project.config.name,
                &names,
                &session.project.config.undra_version,
                &session.project.config.ios.deployment_target,
            ),
        )?;
        artifacts.push(Artifact {
            label: "react native pod".to_owned(),
            size: size_of(&path),
            path,
            budget: None,
            note: Some(format!("pod '{}', :path => '<build>/ios'", names.bundle())),
        });
    } else {
        session.ui.warn(
            "skipping the iOS half of the React Native build: it can only be built on macOS (`undra build --platform ios` says more)",
        );
    }
    artifacts.extend(android::build(session, release, symbols)?);
    Ok(artifacts)
}

/// The name of the Objective-C class `@undra/react-native` looks a core up by:
/// `UndraCoreTable_<namespace>`.
#[must_use]
pub fn table_class_name(names: &CoreNames) -> String {
    format!("UndraCoreTable_{}", names.namespace())
}

/// `<Bundle>Table.m`: the class whose `+api` returns the core's table.
#[must_use]
pub fn table_class(names: &CoreNames) -> String {
    let class = table_class_name(names);
    let symbol = names.api_symbol();
    let namespace = names.namespace();
    format!(
        r##"// Written by `undra build --platform rn`; rewritten by every build, do not edit.
//
// The C ABI table of the Undra core `{namespace}` (docs/SPEC.md 6, ADR-044), for @undra/react-native:
// the module is built once for every core of the app, so it finds this one by its namespace,
// `NSClassFromString(@"{class}")`, when JavaScript loads it, and calls `+api`.
#import <Foundation/Foundation.h>

/* The core's one export (its header, {header}, ships in the XCFramework). */
const void *{symbol}(void);

@interface {class} : NSObject
/// The core's `UndraApi` table (undra.h); immutable, valid for the life of the process.
+ (const void *)api;
@end

@implementation {class}
+ (const void *)api {{
  return {symbol}();
}}
@end
"##,
        header = names.header(),
    )
}

/// The `<Bundle>` pod: the XCFramework next to it and the table class.
#[must_use]
pub fn podspec(project: &str, names: &CoreNames, version: &str, deployment_target: &str) -> String {
    let bundle = names.bundle();
    let namespace = names.namespace();
    format!(
        r##"# Written by `undra build --platform rn`; rewritten by every build, do not edit.
#
# The Undra core `{namespace}` of `{project}` as a CocoaPods pod, for React Native apps that use
# @undra/react-native (docs/REACT_NATIVE.md). In the app's Podfile:
#
#   pod '{bundle}', :path => '<this directory>'
#
# Each slice of the XCFramework is one prelinked object whose only global symbol is the core's entry,
# `{namespace}_undra_api` (ADR-044), so the library links like any other and several cores sit in one app.
# `{bundle}Table.m` lets @undra/react-native find the core by its namespace; `-ObjC` keeps that class.
Pod::Spec.new do |s|
  s.name                = "{bundle}"
  s.version             = "{version}"
  s.summary             = "The Undra core {namespace} of {project}, built by undra build."
  s.homepage            = "https://github.com/shreypdev/undra"
  s.license             = {{ :type => "Proprietary", :text => "The app's own core." }}
  s.author              = "undra build"
  s.source              = {{ :http => "file://#{{__dir__}}/{bundle}.xcframework" }}
  s.platforms           = {{ :ios => "{deployment_target}" }}
  s.vendored_frameworks = "{bundle}.xcframework"
  s.source_files        = "{bundle}Table.m"
  s.user_target_xcconfig = {{ "OTHER_LDFLAGS" => "$(inherited) -ObjC" }}
end
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_podspec_vendors_the_xcframework_and_the_table_class_without_force_load() {
        let names = CoreNames::new("playground_core");
        let spec = podspec("playground", &names, "0.1", "17.0");
        assert!(
            spec.contains(r#"s.name                = "PlaygroundCore""#),
            "{spec}"
        );
        assert!(spec.contains(r#"s.vendored_frameworks = "PlaygroundCore.xcframework""#));
        assert!(spec.contains(r#"s.source_files        = "PlaygroundCoreTable.m""#));
        assert!(spec.contains(r#"{ :ios => "17.0" }"#));
        assert!(!spec.contains("-force_load"), "{spec}");
        assert!(spec.contains("`playground`"));
    }

    #[test]
    fn the_table_class_is_named_after_the_namespace_and_returns_the_entry() {
        let names = CoreNames::new("acme_pay");
        assert_eq!(table_class_name(&names), "UndraCoreTable_acme_pay");
        let m = table_class(&names);
        assert!(m.contains("@implementation UndraCoreTable_acme_pay"), "{m}");
        assert!(m.contains("const void *acme_pay_undra_api(void);"), "{m}");
        assert!(m.contains("return acme_pay_undra_api();"), "{m}");
    }
}
