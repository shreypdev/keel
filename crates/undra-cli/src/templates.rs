//! The template sets `undra init` and `undra adopt` write.
//!
//! The files live in `templates/` and are compiled into the binary (`include_str!`), so creating
//! a project needs no network and no files next to the executable. Paths may contain
//! placeholders (`ios/@@APP@@.xcodeproj/...`); see [`crate::render`].
//!
//! The templates of Cargo manifests are called `Cargo.toml.tmpl`: a `Cargo.toml` with `@@...@@`
//! placeholders in it is not TOML, and Cargo, which searches a whole git repository for the package
//! a git dependency names, prints a parse error for each one it meets, on every project's first
//! fetch of Undra.
//!
//! The tests render every template with realistic values and fail on a placeholder nothing sets,
//! and the integration tests build the result with the real toolchains.

use crate::render::{TemplateFile, template};

/// The project's own files: the Cargo workspace and `.gitignore`.
pub const PROJECT: &[TemplateFile] = &[
    template!("project/Cargo.toml.tmpl" => "Cargo.toml"),
    template!("project/gitignore" => ".gitignore"),
];

/// The LLDB startup file of a project made by `undra init`: the Rust formatters of the toolchain
/// (ADR-046). Not written by `undra adopt`, whose directory is not where LLDB starts.
pub const LLDBINIT: TemplateFile = template!("project/lldbinit" => ".lldbinit");

/// The core crate.
pub const CORE: &[TemplateFile] = &[
    template!("core/Cargo.toml.tmpl" => "core/Cargo.toml"),
    template!("core/src/lib.rs" => "core/src/lib.rs"),
];

/// The template core's schema in canonical form (`Schema::canonical_json`: no docs, no labels),
/// which `init` generates the first bindings from. A test builds the template and checks that
/// `undra bindgen` (which reads the built library's `undra_schema_json`) writes the same files,
/// so this cannot go stale unnoticed.
pub const CORE_SCHEMA: &str = include_str!("../templates/core/schema.json");

/// The iOS app: an Xcode project (synchronized folder, no file lists to maintain) and a SwiftUI
/// app using the generated Swift package.
pub const IOS: &[TemplateFile] = &[
    template!("ios/project.pbxproj" => "ios/@@APP@@.xcodeproj/project.pbxproj"),
    template!("ios/Info.plist" => "ios/Config/Info.plist"),
    template!("ios/undra-core-outputs.xcfilelist" => "ios/Config/undra-core-outputs.xcfilelist"),
    template!("ios/App.swift" => "ios/@@APP@@/MainApp.swift"),
    template!("ios/UndraBootstrap.swift" => "ios/@@APP@@/UndraBootstrap.swift"),
    template!("ios/ContentView.swift" => "ios/@@APP@@/ContentView.swift"),
    template!("ios/DevStatusBar.swift" => "ios/@@APP@@/DevStatusBar.swift"),
];

/// The output list of the iOS Run Script phase (a template of its own so the tests can render it
/// for other simulator architectures).
#[cfg(test)]
pub const IOS_OUTPUT_LIST: &str = include_str!("../templates/ios/undra-core-outputs.xcfilelist");

/// The Android app: a Gradle project with a Compose screen.
pub const ANDROID: &[TemplateFile] = &[
    template!("android/settings.gradle.kts" => "android/settings.gradle.kts"),
    template!("android/build.gradle.kts" => "android/build.gradle.kts"),
    template!("android/gradle.properties" => "android/gradle.properties"),
    template!("android/app/build.gradle.kts" => "android/app/build.gradle.kts"),
    template!("android/app/proguard-rules.pro" => "android/app/proguard-rules.pro"),
    template!("android/app/src/main/AndroidManifest.xml" => "android/app/src/main/AndroidManifest.xml"),
    template!("android/app/src/debug/AndroidManifest.xml" => "android/app/src/debug/AndroidManifest.xml"),
    template!("android/app/src/main/res/values/strings.xml" => "android/app/src/main/res/values/strings.xml"),
    template!("android/UndraApp.kt" => "android/app/src/main/kotlin/@@APP_ID_PATH@@/UndraApp.kt"),
    template!("android/MainActivity.kt" => "android/app/src/main/kotlin/@@APP_ID_PATH@@/MainActivity.kt"),
    template!("android/DevServer.kt" => "android/app/src/main/kotlin/@@APP_ID_PATH@@/DevServer.kt"),
    template!("android/DevStatus.kt" => "android/app/src/main/kotlin/@@APP_ID_PATH@@/DevStatus.kt"),
];

/// The web app: Vite, React and TypeScript.
pub const WEB: &[TemplateFile] = &[
    template!("web/package.json" => "web/package.json"),
    template!("web/index.html" => "web/index.html"),
    template!("web/vite.config.ts" => "web/vite.config.ts"),
    template!("web/tsconfig.json" => "web/tsconfig.json"),
    template!("web/src/undra.ts" => "web/src/undra.ts"),
    template!("web/src/dev-banner.ts" => "web/src/dev-banner.ts"),
    template!("web/src/useSignal.ts" => "web/src/useSignal.ts"),
    template!("web/src/main.tsx" => "web/src/main.tsx"),
    template!("web/src/App.tsx" => "web/src/App.tsx"),
    template!("web/src/index.css" => "web/src/index.css"),
];

/// The README of a new project.
pub const README: TemplateFile = template!("project/README.md" => "README.md");

/// The wiring steps `undra adopt` writes.
pub const ADOPT_GUIDE: TemplateFile = template!("adopt/UNDRA_ADOPT.md" => "UNDRA_ADOPT.md");
