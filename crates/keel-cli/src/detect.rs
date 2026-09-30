//! Recognising an existing app repository: which platforms it has and where their project
//! files are. `keel adopt` uses it to fill the real paths into the steps it writes.
//!
//! Detection only reads: it looks for well-known files (`*.xcodeproj`, `settings.gradle(.kts)`,
//! `package.json`), never parses a project deeper than a line-level search for an identifier, and
//! skips dependency and build directories.

use std::fs;
use std::path::{Path, PathBuf};

/// Directories never searched: dependencies, build output, version control, and Keel's own.
const SKIP: &[&str] = &[
    "node_modules",
    ".git",
    "build",
    "Pods",
    "Carthage",
    "target",
    "DerivedData",
    ".gradle",
    ".build",
    "dist",
    "keel",
    ".idea",
    ".next",
];

/// How deep below the repository root the search goes.
const MAX_DEPTH: usize = 4;

/// An iOS app found in the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IosApp {
    /// The directory that holds the project (Xcode's `SRCROOT`).
    pub dir: PathBuf,
    /// The `.xcodeproj` directory, when there is one.
    pub project: Option<PathBuf>,
    /// The `.xcworkspace`, when there is one next to it.
    pub workspace: Option<PathBuf>,
    /// A Swift package manifest (an app defined as a package, or a library).
    pub package: Option<PathBuf>,
    /// The bundle identifier, when one could be read.
    pub bundle_id: Option<String>,
}

/// An Android app found in the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidApp {
    /// The directory of `settings.gradle(.kts)`: the Gradle root.
    pub root: PathBuf,
    /// Whether the build scripts are Kotlin (`.kts`).
    pub kotlin_dsl: bool,
    /// The module that applies `com.android.application`, when one was found.
    pub app_module: Option<PathBuf>,
    /// The application id (or namespace), when one could be read.
    pub app_id: Option<String>,
}

/// A web app found in the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebApp {
    /// The directory of `package.json`.
    pub dir: PathBuf,
    /// The bundler or framework (`vite`, `webpack`, `next`, ...), when one was recognised.
    pub tool: Option<String>,
    /// Whether the project uses TypeScript.
    pub typescript: bool,
}

/// What was found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detected {
    /// An iOS app.
    pub ios: Option<IosApp>,
    /// An Android app.
    pub android: Option<AndroidApp>,
    /// A web app.
    pub web: Option<WebApp>,
}

impl Detected {
    /// A one-line description: `iOS (App.xcodeproj), Android (Gradle, Kotlin DSL)`.
    #[must_use]
    pub fn describe(&self, root: &Path) -> String {
        let shown = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
        let mut parts = Vec::new();
        if let Some(ios) = &self.ios {
            let what = ios
                .project
                .as_deref()
                .or(ios.package.as_deref())
                .map_or_else(|| shown(&ios.dir), shown);
            parts.push(format!("iOS ({what})"));
        }
        if let Some(android) = &self.android {
            parts.push(format!(
                "Android (Gradle at {}, {} DSL)",
                shown(&android.root),
                if android.kotlin_dsl {
                    "Kotlin"
                } else {
                    "Groovy"
                }
            ));
        }
        if let Some(web) = &self.web {
            parts.push(format!(
                "web ({} in {})",
                web.tool.as_deref().unwrap_or("package.json"),
                shown(&web.dir)
            ));
        }
        if parts.is_empty() {
            "nothing".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

/// Looks for apps in `root`.
#[must_use]
pub fn detect(root: &Path) -> Detected {
    let mut found = Detected::default();
    let mut queue = vec![(root.to_path_buf(), 0_usize)];
    while !queue.is_empty() {
        // Breadth first: the shallowest project of each platform wins.
        let level = std::mem::take(&mut queue);
        for (dir, depth) in level {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<(String, PathBuf, bool)> = entries
                .filter_map(|e| e.ok())
                .map(|e| {
                    let path = e.path();
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        path.clone(),
                        path.is_dir(),
                    )
                })
                .collect();
            names.sort();
            inspect(&dir, &names, &mut found);
            if depth < MAX_DEPTH {
                for (name, path, is_dir) in &names {
                    // Bundles like `App.xcodeproj` are opaque; dot directories are tooling.
                    if *is_dir
                        && !SKIP.contains(&name.as_str())
                        && !name.starts_with('.')
                        && !name.ends_with(".xcodeproj")
                        && !name.ends_with(".xcworkspace")
                    {
                        queue.push((path.clone(), depth + 1));
                    }
                }
            }
        }
    }
    found
}

fn inspect(dir: &Path, entries: &[(String, PathBuf, bool)], found: &mut Detected) {
    let has = |name: &str| entries.iter().any(|(n, _, _)| n == name);
    let find_suffix = |suffix: &str| {
        entries
            .iter()
            .find(|(n, _, is_dir)| *is_dir && n.ends_with(suffix))
            .map(|(_, p, _)| p.clone())
    };

    if found.ios.is_none() {
        let project = find_suffix(".xcodeproj");
        let package = has("Package.swift").then(|| dir.join("Package.swift"));
        if project.is_some() || package.is_some() {
            let bundle_id = project
                .as_deref()
                .and_then(|p| read_bundle_id(&p.join("project.pbxproj")));
            found.ios = Some(IosApp {
                dir: dir.to_path_buf(),
                workspace: find_suffix(".xcworkspace"),
                project,
                package,
                bundle_id,
            });
        }
    }
    if found.android.is_none() && (has("settings.gradle") || has("settings.gradle.kts")) {
        let kotlin_dsl = has("settings.gradle.kts");
        let (app_module, app_id) = find_android_app(dir, kotlin_dsl);
        // A Gradle build that is not Android (a plain JVM project) is not an Android app.
        if app_module.is_some() || dir.join("app/src/main/AndroidManifest.xml").is_file() {
            found.android = Some(AndroidApp {
                root: dir.to_path_buf(),
                kotlin_dsl,
                app_module,
                app_id,
            });
        }
    }
    if found.web.is_none() && has("package.json") {
        if let Ok(text) = fs::read_to_string(dir.join("package.json")) {
            if let Some(web) = read_web_app(dir, &text) {
                found.web = Some(web);
            }
        }
    }
}

/// `PRODUCT_BUNDLE_IDENTIFIER = com.acme.app;` from a `project.pbxproj`, skipping variables and
/// test targets.
fn read_bundle_id(pbxproj: &Path) -> Option<String> {
    let text = fs::read_to_string(pbxproj).ok()?;
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("PRODUCT_BUNDLE_IDENTIFIER = "))
        .map(|rest| {
            rest.trim_end_matches(';')
                .trim()
                .trim_matches('"')
                .to_owned()
        })
        .find(|id| !id.contains('$') && !id.to_ascii_lowercase().contains("test"))
}

/// The Gradle module that applies the Android application plugin and its application id.
fn find_android_app(root: &Path, kotlin_dsl: bool) -> (Option<PathBuf>, Option<String>) {
    let script = if kotlin_dsl {
        "build.gradle.kts"
    } else {
        "build.gradle"
    };
    let Ok(entries) = fs::read_dir(root) else {
        return (None, None);
    };
    let mut modules: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    modules.sort();
    modules.insert(0, root.to_path_buf());
    for module in modules {
        let Ok(text) = fs::read_to_string(module.join(script)) else {
            continue;
        };
        if applies_android_application(&text) {
            let id =
                gradle_value(&text, "applicationId").or_else(|| gradle_value(&text, "namespace"));
            return (Some(module), id);
        }
    }
    (None, None)
}

/// Whether a Gradle script applies the Android application plugin (as opposed to declaring its
/// version for the modules, which a root script does with `apply false`).
fn applies_android_application(script: &str) -> bool {
    script
        .lines()
        .filter(|line| line.contains("com.android.application"))
        .any(|line| !line.replace(' ', "").contains("applyfalse"))
}

/// `applicationId = "x"` or `applicationId "x"` from a Gradle script; the key may sit inside a
/// block on the same line (`defaultConfig { applicationId = "x" }`).
fn gradle_value(script: &str, key: &str) -> Option<String> {
    script.lines().find_map(|line| {
        let at = line.find(key)?;
        if line[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            return None;
        }
        let value = line[at + key.len()..]
            .trim_start()
            .trim_start_matches('=')
            .trim();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        value[1..]
            .split(quote)
            .next()
            .map(ToOwned::to_owned)
            .filter(|v| !v.is_empty())
    })
}

/// A `package.json` that looks like a web app (it has a bundler, a framework or React/Vue/...).
fn read_web_app(dir: &Path, text: &str) -> Option<WebApp> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let has_dep = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|section| json[*section].get(name).is_some())
    };
    let tool = [
        "vite",
        "next",
        "webpack",
        "react-scripts",
        "@angular/cli",
        "nuxt",
        "parcel",
        "astro",
        "@sveltejs/kit",
        "esbuild",
        "rollup",
    ]
    .iter()
    .find(|name| has_dep(name))
    .map(|name| (*name).to_owned());
    let is_app = tool.is_some()
        || ["react", "vue", "svelte", "solid-js", "preact"]
            .iter()
            .any(|name| has_dep(name));
    is_app.then(|| WebApp {
        dir: dir.to_path_buf(),
        tool,
        typescript: dir.join("tsconfig.json").is_file() || has_dep("typescript"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::{unique_temp_dir, write_if_changed};

    fn write(root: &Path, path: &str, text: &str) {
        write_if_changed(&root.join(path), text).unwrap();
    }

    #[test]
    fn finds_an_ios_project_and_its_bundle_id() {
        let root = unique_temp_dir("detect-ios");
        write(
            &root,
            "ios/App.xcodeproj/project.pbxproj",
            "PRODUCT_BUNDLE_IDENTIFIER = \"$(X)\";\n PRODUCT_BUNDLE_IDENTIFIER = com.acme.appTests;\n PRODUCT_BUNDLE_IDENTIFIER = com.acme.app;\n",
        );
        write(&root, "ios/App.xcworkspace/contents.xcworkspacedata", "");
        let found = detect(&root);
        let ios = found.ios.expect("ios");
        assert_eq!(ios.bundle_id.as_deref(), Some("com.acme.app"));
        assert!(ios.project.unwrap().ends_with("ios/App.xcodeproj"));
        assert!(ios.workspace.is_some());
        assert_eq!(ios.dir, root.join("ios"));
        assert!(found.android.is_none() && found.web.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn finds_an_android_app_module_and_id() {
        let root = unique_temp_dir("detect-android");
        write(&root, "android/settings.gradle.kts", "include(\":app\")\n");
        write(
            &root,
            "android/app/build.gradle.kts",
            "plugins { id(\"com.android.application\") }\nandroid { namespace = \"com.acme.ns\"\n defaultConfig { applicationId = \"com.acme.app\" } }\n",
        );
        let android = detect(&root).android.expect("android");
        assert!(android.kotlin_dsl);
        assert_eq!(android.app_id.as_deref(), Some("com.acme.app"));
        assert_eq!(android.app_module, Some(root.join("android/app")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_root_script_that_only_declares_the_plugin_is_not_the_app_module() {
        let root = unique_temp_dir("detect-android-root");
        write(&root, "android/settings.gradle.kts", "include(\":app\")\n");
        write(
            &root,
            "android/build.gradle.kts",
            "plugins {\n    id(\"com.android.application\") version \"8.7.3\" apply false\n}\n",
        );
        write(
            &root,
            "android/app/build.gradle.kts",
            "plugins { id(\"com.android.application\") }\nandroid { defaultConfig { applicationId = \"com.acme.app\" } }\n",
        );
        let android = detect(&root).android.expect("android");
        assert_eq!(android.app_module, Some(root.join("android/app")));
        assert_eq!(android.app_id.as_deref(), Some("com.acme.app"));
        assert!(applies_android_application("id 'com.android.application'"));
        assert!(!applies_android_application(
            "id(\"com.android.application\") version \"8.7.3\" apply  false"
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_plain_jvm_gradle_build_is_not_android() {
        let root = unique_temp_dir("detect-jvm");
        write(&root, "settings.gradle", "include 'lib'\n");
        write(&root, "lib/build.gradle", "plugins { id 'java-library' }\n");
        assert!(detect(&root).android.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn groovy_application_ids_are_read() {
        assert_eq!(
            gradle_value("  applicationId \"com.x.y\"\n", "applicationId").as_deref(),
            Some("com.x.y")
        );
        assert_eq!(
            gradle_value("applicationId 'com.x.y'", "applicationId").as_deref(),
            Some("com.x.y")
        );
        assert_eq!(
            gradle_value("applicationId = libs.versions.id", "applicationId"),
            None
        );
    }

    #[test]
    fn finds_a_web_app_by_its_tooling() {
        let root = unique_temp_dir("detect-web");
        write(
            &root,
            "web/package.json",
            "{\"devDependencies\": {\"vite\": \"^6\", \"typescript\": \"^5\"}, \"dependencies\": {\"react\": \"^19\"}}",
        );
        write(
            &root,
            "tools/package.json",
            "{\"name\": \"scripts\", \"dependencies\": {\"chalk\": \"5\"}}",
        );
        write(
            &root,
            "web/node_modules/pkg/package.json",
            "{\"dependencies\": {\"react\": \"1\"}}",
        );
        let web = detect(&root).web.expect("web");
        assert_eq!(web.tool.as_deref(), Some("vite"));
        assert!(web.typescript);
        assert_eq!(web.dir, root.join("web"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_empty_repository_has_nothing() {
        let root = unique_temp_dir("detect-none");
        write(&root, "README.md", "# hi");
        assert_eq!(detect(&root), Detected::default());
        assert_eq!(Detected::default().describe(&root), "nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_keel_directory_itself_is_not_searched() {
        let root = unique_temp_dir("detect-keel");
        write(
            &root,
            "keel/web/package.json",
            "{\"devDependencies\": {\"vite\": \"^6\"}}",
        );
        assert!(detect(&root).web.is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
