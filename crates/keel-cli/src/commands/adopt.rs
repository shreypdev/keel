//! `keel adopt`: add a Keel core to an app that already exists.
//!
//! Conservative by design. The app's own project files (an `.xcodeproj`, Gradle scripts, a
//! `package.json`) are never modified: editing them reliably means parsing formats that change
//! with every IDE release, and a wrong edit costs the team an afternoon. Instead `adopt` writes
//! one new directory, `keel/`, holding a core crate, a `keel.toml`, bindings for the platforms it
//! found, and `KEEL_ADOPT.md`: the exact edits to make, with this repository's paths filled in.

use std::path::{Path, PathBuf};

use crate::bindgen::canonicalize_lenient;
use crate::cli::AdoptArgs;
use crate::config::{Platform, ProjectConfig};
use crate::detect::{AndroidApp, Detected, IosApp, WebApp, detect};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{is_empty_dir, write_if_changed};
use crate::names::{
    Names, portable, relative_path, suggest, validate_app_id, validate_project_name,
};
use crate::runtimes::{RuntimeRef, Runtimes, require_checkout};
use crate::templates;

use super::Env;
use super::init::{Setup, generate_bindings, scaffold_core, variables};

/// Runs `keel adopt`.
///
/// # Errors
///
/// `C0009` when the path is not a directory, names no app, or an argument is invalid; `C0008`
/// when `keel/` already exists with files in it.
pub fn run(env: &Env<'_>, args: &AdoptArgs) -> Result<()> {
    let ui = env.ui;
    let start = match &args.path {
        Some(p) if p.is_absolute() => p.clone(),
        Some(p) => env.start_dir()?.join(p),
        None => env.start_dir()?,
    };
    let repo = start.canonicalize().map_err(|e| {
        CliError::new(
            Code::BadArgument,
            format!("{}: {e}", start.display()),
            "`keel adopt` looks at an existing app repository and the path has to be a directory",
            "check the path; for a brand new app use `keel init <name>`",
        )
    })?;
    if !repo.is_dir() {
        return Err(CliError::bad_argument(
            format!("{} is not a directory", repo.display()),
            "`keel adopt` looks at an existing app repository",
            "pass the repository's directory",
        ));
    }

    let detected = detect(&repo);
    let platforms = choose_platforms(&detected, args.platform.as_deref(), &repo, env)?;
    let keel_dir = repo.join("keel");
    if !is_empty_dir(&keel_dir) {
        return Err(CliError::new(
            Code::WouldOverwrite,
            format!("{} already exists and is not empty", keel_dir.display()),
            "`keel adopt` writes one new directory and will not merge into an existing one",
            "if it is a Keel project already, use `keel -C keel <command>`; otherwise move it away and run `keel adopt` again",
        ));
    }

    let raw_name = args.name.clone().unwrap_or_else(|| {
        suggest(
            &repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    });
    validate_project_name(&raw_name)?;
    let names = Names::derive(&raw_name);
    let id = detect_id(&detected).unwrap_or_else(|| names.default_app_id());
    validate_app_id(&id).map_err(|e| {
        CliError::bad_argument(
            format!(
                "the application id of this app, `{id}`, cannot be used as a Kotlin package base"
            ),
            e.why,
            "pass `--name` so the default id is used, or rename the app's id",
        )
    })?;

    let mut config = ProjectConfig::new(&raw_name, &id, platforms.clone());
    let keel_repo = match &args.keel_path {
        Some(path) => {
            let resolved = path.canonicalize().map_err(|e| {
                CliError::new(
                    Code::BadArgument,
                    format!("--keel-path {}: {e}", path.display()),
                    "the path has to be a checkout of the Keel repository",
                    "check the path, or leave --keel-path out to use released versions",
                )
            })?;
            require_checkout(&resolved)?;
            config.keel_path = Some(portable(
                &relative_path(&keel_dir, &resolved).unwrap_or_else(|| resolved.clone()),
            ));
            Some(resolved)
        }
        None => None,
    };
    let setup = Setup {
        root: canonicalize_lenient(&keel_dir),
        names,
        config,
        repo: keel_repo,
    };

    ui.step(&format!("Adding a Keel core to {}", repo.display()));
    let vars = variables(&setup);
    scaffold_core(&setup, &vars)?;
    let generated = generate_bindings(&setup)?;

    let runtimes = match &setup.repo {
        Some(repo) => Runtimes::in_repo(repo),
        None => Runtimes::from_registries(&setup.config.keel_version),
    };
    let steps = Steps {
        keel_dir: &setup.root,
        names: &setup.names,
        config: &setup.config,
        runtimes: &runtimes,
    };
    let mut text = String::new();
    if let Some(ios) = detected
        .ios
        .as_ref()
        .filter(|_| platforms.contains(&Platform::Ios))
    {
        text.push_str(&steps.ios(ios));
    } else if platforms.contains(&Platform::Ios) {
        text.push_str(&steps.ios(&placeholder_ios(&repo)));
    }
    if let Some(android) = detected
        .android
        .as_ref()
        .filter(|_| platforms.contains(&Platform::Android))
    {
        text.push_str(&steps.android(android));
    } else if platforms.contains(&Platform::Android) {
        text.push_str(&steps.android(&placeholder_android(&repo)));
    }
    if let Some(web) = detected
        .web
        .as_ref()
        .filter(|_| platforms.contains(&Platform::Web))
    {
        text.push_str(&steps.web(web));
    } else if platforms.contains(&Platform::Web) {
        text.push_str(&steps.web(&placeholder_web(&repo)));
    }

    let guide_vars = vars
        .with("DIR", portable(relative_path(&repo, &setup.root).as_deref().unwrap_or(&setup.root)))
        .with("DETECTED", detected.describe(&repo))
        .with("STEPS", text.clone())
        .with(
            "KEEL_SOURCE_NOTE",
            match &setup.repo {
                Some(r) => format!("The core uses the Keel crates, and the apps the Keel runtimes, from the checkout at `{}`.", r.display()),
                None => "The core depends on the released Keel crates; the apps on the matching runtimes (Swift package, Maven artifact, npm package).".to_owned(),
            },
        );
    let guide = guide_vars
        .render(templates::ADOPT_GUIDE.contents)
        .map_err(|name| {
            CliError::new(
                Code::ToolFailed,
                format!("a keel-cli template uses the placeholder @@{name}@@ and nothing sets it"),
                "this is a bug in keel-cli, not in your arguments",
                "report it at https://github.com/shreypdev/keel/issues",
            )
        })?;
    write_if_changed(&setup.root.join(templates::ADOPT_GUIDE.path), &guide)?;

    let rel_keel = portable(
        relative_path(&repo, &setup.root)
            .as_deref()
            .unwrap_or(&setup.root),
    );
    ui.line(&format!(
        "Detected: {}. Created {rel_keel}/ with a core crate and bindings for schema hash {:#018x}; nothing else was changed.",
        detected.describe(&repo),
        generated.hash
    ));
    ui.line("");
    ui.line(&format!(
        "Steps to wire it in are in {rel_keel}/KEEL_ADOPT.md. In short:"
    ));
    for platform in &platforms {
        ui.line(&format!("  {}", summary_line(*platform, &rel_keel)));
    }
    ui.line(&format!(
        "  cd {rel_keel} && keel doctor && keel build --release"
    ));
    Ok(())
}

fn summary_line(platform: Platform, keel: &str) -> String {
    match platform {
        Platform::Ios => format!(
            "iOS      add the package {keel}/generated/swift and link {keel}/build/ios/KeelCore.xcframework"
        ),
        Platform::Android => format!(
            "Android  include {keel}/generated/kotlin as a Gradle module and package {keel}/build/android/jniLibs"
        ),
        Platform::Web => format!(
            "web      make @keel/runtime and the bindings in {keel}/generated/ts resolvable, load {keel}/build/web/keel_core.wasm"
        ),
    }
}

/// The platforms to generate for: `--platform`, else everything detected.
fn choose_platforms(
    detected: &Detected,
    requested: Option<&str>,
    repo: &Path,
    env: &Env<'_>,
) -> Result<Vec<Platform>> {
    if let Some(list) = requested {
        let platforms = Platform::parse_list(list)?;
        for p in &platforms {
            let found = match p {
                Platform::Ios => detected.ios.is_some(),
                Platform::Android => detected.android.is_some(),
                Platform::Web => detected.web.is_some(),
            };
            if !found {
                env.ui.warn(&format!(
                    "no {} app was found in {}; the steps for it use placeholder paths",
                    p.name(),
                    repo.display()
                ));
            }
        }
        return Ok(platforms);
    }
    let mut platforms = Vec::new();
    if detected.ios.is_some() {
        platforms.push(Platform::Ios);
    }
    if detected.android.is_some() {
        platforms.push(Platform::Android);
    }
    if detected.web.is_some() {
        platforms.push(Platform::Web);
    }
    if platforms.is_empty() {
        return Err(CliError::new(
            Code::BadArgument,
            format!("no iOS, Android or web app was found in {}", repo.display()),
            "`keel adopt` looks for an .xcodeproj or Package.swift, a Gradle build with the Android application plugin, or a package.json with a bundler or UI framework, up to four directories deep",
            "point it at the app's repository, say which platform you mean with `--platform ios|android|web`, or start from scratch with `keel init <name>`",
        ));
    }
    Ok(platforms)
}

/// The application id the existing app already has, so the bindings' package matches it.
fn detect_id(detected: &Detected) -> Option<String> {
    detected
        .android
        .as_ref()
        .and_then(|a| a.app_id.clone())
        .or_else(|| detected.ios.as_ref().and_then(|i| i.bundle_id.clone()))
        .map(|id| id.to_ascii_lowercase().replace('-', "_"))
        .filter(|id| validate_app_id(id).is_ok())
}

fn placeholder_ios(repo: &Path) -> IosApp {
    IosApp {
        dir: repo.join("<ios directory>"),
        project: None,
        workspace: None,
        package: None,
        bundle_id: None,
    }
}

fn placeholder_android(repo: &Path) -> AndroidApp {
    AndroidApp {
        root: repo.join("<android directory>"),
        kotlin_dsl: true,
        app_module: Some(repo.join("<android directory>/app")),
        app_id: None,
    }
}

fn placeholder_web(repo: &Path) -> WebApp {
    WebApp {
        dir: repo.join("<web directory>"),
        tool: None,
        typescript: true,
    }
}

/// The wiring text, with paths relative to where each edit is made.
struct Steps<'a> {
    keel_dir: &'a Path,
    names: &'a Names,
    config: &'a ProjectConfig,
    runtimes: &'a Runtimes,
}

impl Steps<'_> {
    fn rel(&self, from: &Path, to: &Path) -> String {
        portable(&relative_path(from, to).unwrap_or_else(|| to.to_path_buf()))
    }

    fn generated(&self) -> PathBuf {
        self.keel_dir.join(&self.config.generated)
    }

    fn build(&self) -> PathBuf {
        self.keel_dir.join(&self.config.build)
    }

    fn ios(&self, app: &IosApp) -> String {
        let module = &self.names.swift_module;
        let package = self.rel(&app.dir, &self.generated().join("swift"));
        let xcframework = self.rel(&app.dir, &self.build().join("ios/KeelCore.xcframework"));
        let mut text = String::from("## iOS\n\n");
        text.push_str(&format!(
            "Xcode project: `{}`\n\n",
            app.project
                .as_deref()
                .or(app.package.as_deref())
                .map_or_else(
                    || app.dir.display().to_string(),
                    |p| p.display().to_string()
                )
        ));
        text.push_str(&format!(
            "1. **Add the generated package.** In Xcode: *File > Add Package Dependencies... > Add Local...* and choose\n   `{}` (relative to the project: `{package}`). Add the product `{module}` to your app target.\n",
            self.generated().join("swift").display()
        ));
        let runtime = match &self.runtimes.swift {
            RuntimeRef::Path(dir) => format!(
                "the `KeelRuntime` package at `{}` (the generated package depends on it)",
                dir.display()
            ),
            RuntimeRef::Registry { version } => {
                format!("`KeelRuntime` {version}.x from https://github.com/shreypdev/keel-swift")
            }
        };
        text.push_str(&format!(
            "   It pulls in {runtime}; add the product `KeelRuntime` to the app target too.\n"
        ));
        text.push_str(&format!(
            "2. **Link the core.** `keel build --platform ios` writes `KeelCore.xcframework`. Add it to the target's\n   *Frameworks, Libraries, and Embedded Content* (Do Not Embed): `{xcframework}`.\n"
        ));
        text.push_str(&format!(
            "3. **Keep the core's registrations.** Under *Build Settings > Other Linker Flags* add, for the simulator SDK and the\n   device SDK respectively (a debug static library would otherwise lose the `#[keel::api]` registrations):\n\n   ```\n   Any iOS Simulator SDK:  -force_load $(SRCROOT)/{xcframework}/ios-arm64-simulator/libkeel_core.a\n   Any iOS SDK:            -force_load $(SRCROOT)/{xcframework}/ios-arm64/libkeel_core.a\n   ```\n"
        ));
        text.push_str("4. **Real core, not the stand-in.** The Swift runtime ships link-time stand-ins for the core so that it builds without one.\n   Switch them off by starting Xcode with `KEEL_LINK_CORE=1` (`open --env KEEL_LINK_CORE=1 -a Xcode <project>`), or build with\n   `KEEL_LINK_CORE=1 xcodebuild ...`.\n");
        text.push_str(&format!(
            "5. **Load the core at startup**, before any Keel object exists:\n\n   ```swift\n   import KeelRuntime\n   import {module}\n\n   @main struct MyApp: App {{\n       init() {{\n           do {{ try KeelCore.load(.inproc(expectedSchemaHash: KeelIds.schemaHash)) }}\n           catch {{ fatalError(\"Keel did not start: \\(error)\") }}\n       }}\n       // ...\n   }}\n   ```\n\n   then use the generated store wherever a screen needs it: `@State private var todos = try! Todos()` (a `@MainActor @Observable`\n   class; read its properties in a SwiftUI view).\n"
        ));
        text.push_str("6. **Optional, for `keel dev`:** add `NSAppTransportSecurity > NSAllowsLocalNetworking = YES` to the Info.plist so the app may connect to\n   `ws://` on your network, and pass `.remote(url: \"ws://<your Mac>:7443\", ...)` to `KeelCore.load` in debug builds.\n\n");
        text
    }

    fn android(&self, app: &AndroidApp) -> String {
        let module = app
            .app_module
            .clone()
            .unwrap_or_else(|| app.root.join("app"));
        let bindings = self.rel(&app.root, &self.generated().join("kotlin"));
        let jni = self.rel(&module, &self.build().join("android/jniLibs"));
        let kts = app.kotlin_dsl;
        let version = match &self.runtimes.kotlin {
            RuntimeRef::Path(_) => "0.1.0-SNAPSHOT".to_owned(),
            RuntimeRef::Registry { version } => format!("{version}.0"),
        };
        let package = self
            .config
            .bindings
            .kotlin_package
            .clone()
            .unwrap_or_else(|| format!("{}.core", self.config.id));
        let mut text = String::from("## Android\n\n");
        text.push_str(&format!(
            "Gradle project: `{}` (app module `{}`)\n\n",
            app.root.display(),
            module.display()
        ));
        let (include, project_dir, include_build) = if kts {
            (
                format!(
                    "include(\":core-bindings\")\nproject(\":core-bindings\").projectDir = file(\"{bindings}\")"
                ),
                String::new(),
                match &self.runtimes.kotlin {
                    RuntimeRef::Path(dir) => {
                        format!("\n   includeBuild(\"{}\")", self.rel(&app.root, dir))
                    }
                    RuntimeRef::Registry { .. } => String::new(),
                },
            )
        } else {
            (
                format!(
                    "include ':core-bindings'\nproject(':core-bindings').projectDir = file('{bindings}')"
                ),
                String::new(),
                match &self.runtimes.kotlin {
                    RuntimeRef::Path(dir) => {
                        format!("\n   includeBuild('{}')", self.rel(&app.root, dir))
                    }
                    RuntimeRef::Registry { .. } => String::new(),
                },
            )
        };
        let _ = project_dir;
        let settings = if kts {
            "settings.gradle.kts"
        } else {
            "settings.gradle"
        };
        let build = if kts {
            "build.gradle.kts"
        } else {
            "build.gradle"
        };
        text.push_str(&format!(
            "1. **Include the bindings module** in `{settings}`:\n\n   ```\n   {include}{include_build}\n   ```\n\n   The generated module (`{}`) is a plain JVM library with the Kotlin plugin; its `build.gradle.kts` uses\n   `id(\"org.jetbrains.kotlin.jvm\")` without a version, so declare that plugin (Kotlin 2.0.x) in your root build script\n   with `apply false`, as you do for the Android and Kotlin Android plugins.\n",
            self.generated().join("kotlin").display()
        ));
        let deps = if kts {
            format!(
                "implementation(project(\":core-bindings\"))\n   implementation(\"dev.keel:runtime:{version}\")"
            )
        } else {
            format!(
                "implementation project(':core-bindings')\n   implementation 'dev.keel:runtime:{version}'"
            )
        };
        let jni_line = if kts {
            format!("sourceSets {{ getByName(\"main\").jniLibs.srcDir(\"{jni}\") }}")
        } else {
            format!("sourceSets {{ main {{ jniLibs.srcDir '{jni}' }} }}")
        };
        text.push_str(&format!(
            "2. **Depend on it and package the native libraries** in the app module's `{build}` (inside `android {{ }}` for the\n   source set):\n\n   ```\n   dependencies {{\n   {deps}\n   }}\n   android {{\n       {jni_line}\n   }}\n   ```\n\n   `keel build --platform android --release` writes `libkeel_core.so` for the ABIs in keel.toml (arm64-v8a, x86_64) below\n   `build/android/jniLibs`; use `--release` when you package (a debug core is tens of megabytes per ABI).\n   The app needs `minSdk` {} or higher and Kotlin/Java 11 bytecode.\n",
            self.config.android.min_sdk
        ));
        text.push_str(&format!(
            "3. **Load the core once per process**, in your `Application` subclass (register it with `android:name` in the manifest):\n\n   ```kotlin\n   import {package}.KeelIds\n   import dev.keel.runtime.KeelCore\n   import dev.keel.runtime.LoadOptions\n\n   class App : Application() {{\n       override fun onCreate() {{\n           super.onCreate()\n           KeelCore.load(LoadOptions(expectedSchemaHash = KeelIds.SCHEMA_HASH))\n       }}\n   }}\n   ```\n\n   then `val todos = Todos()` (a generated store; its `StateFlow` properties work with `collectAsState()`).\n"
        ));
        text.push_str("4. **Shrinking.** If you minify, keep the natives the library registers by name: `-keep class dev.keel.runtime.KeelNative { *; }` and\n   `-keep class dev.keel.runtime.KeelNative$Callbacks { *; }`.\n\n");
        text
    }

    fn web(&self, app: &WebApp) -> String {
        let generated = self.rel(&app.dir, &self.generated().join("ts"));
        let wasm = self.rel(&app.dir, &self.build().join("web/keel_core.wasm"));
        let ts_package = format!("@app/{}", self.names.core_package);
        let mut text = String::from("## Web\n\n");
        text.push_str(&format!(
            "Web app: `{}`{}\n\n",
            app.dir.display(),
            app.tool
                .as_deref()
                .map_or(String::new(), |t| format!(" ({t})"))
        ));
        match &self.runtimes.ts {
            RuntimeRef::Path(dir) => {
                let src = self.rel(&app.dir, &dir.join("src/index.ts"));
                text.push_str(&format!(
                    "1. **Resolve the runtime and the bindings.** Both are used from their TypeScript sources, so there is no build step between a core\n   change and the browser. Alias them in your bundler (Vite shown; webpack `resolve.alias` and a `tsconfig.json` `paths` entry work the same way):\n\n   ```ts\n   resolve: {{ alias: {{\n     \"@keel/runtime\": fileURLToPath(new URL(\"{src}\", import.meta.url)),\n     \"{ts_package}\": fileURLToPath(new URL(\"{generated}/src/index.ts\", import.meta.url)),\n   }} }},\n   server: {{ fs: {{ allow: [\"..\", \"{}\"] }} }},\n   ```\n",
                    self.rel(&app.dir, dir)
                ));
            }
            RuntimeRef::Registry { version } => {
                text.push_str(&format!(
                    "1. **Install the runtime and the bindings.**\n\n   ```sh\n   npm install @keel/runtime@^{version}.0 {generated}\n   ```\n\n   (`{generated}` is the generated package, `{ts_package}`; it imports `@keel/runtime` as a peer dependency.)\n"
                ));
            }
        }
        text.push_str(&format!(
            "2. **Load the core before rendering**, in the app's entry point:\n\n   ```ts\n   import {{ KeelCore }} from \"@keel/runtime\";\n   import {{ KeelIds, Todos }} from \"{ts_package}\";\n   import wasmUrl from \"{wasm}?url\"; // a bundler asset: Vite shown; webpack 5: new URL(\"{wasm}\", import.meta.url)\n\n   await KeelCore.load({{\n     mode: \"wasm-main\",\n     wasm: new URL(wasmUrl, location.href),\n     expectedSchemaHash: KeelIds.schemaHash,\n   }});\n   const todos = await Todos.create();\n   ```\n\n   `keel build --platform web` writes `build/web/keel_core.wasm`. Signals are `todos.visible.get()` / `.subscribe(fn)`; in React read them with\n   `useSyncExternalStore` (see the `web/` app of a `keel init` project).\n"
        ));
        text.push_str("3. **Optional, for `keel dev`:** `await KeelCore.load({ mode: \"remote\", url: \"ws://127.0.0.1:7443\", expectedSchemaHash: KeelIds.schemaHash })`.\n\n");
        text
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::fsutil::{create_dir_all, unique_temp_dir};
    use crate::project::Project;
    use crate::sys::RealSys;
    use crate::ui::Ui;

    fn env(dir: &Path) -> Env<'static> {
        static SYS: RealSys = RealSys;
        Env {
            sys: &SYS,
            ui: Ui::plain(),
            project_dir: Some(dir.to_path_buf()),
        }
    }

    fn args(path: &Path) -> AdoptArgs {
        AdoptArgs {
            path: Some(path.to_path_buf()),
            platform: None,
            name: None,
            keel_path: None,
        }
    }

    fn write(root: &Path, path: &str, text: &str) {
        write_if_changed(&root.join(path), text).unwrap();
    }

    fn sample_repo(tag: &str) -> PathBuf {
        let root = unique_temp_dir(tag);
        create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap().join("MyApp");
        write(
            &root,
            "ios/MyApp.xcodeproj/project.pbxproj",
            "PRODUCT_BUNDLE_IDENTIFIER = com.acme.myapp;\n",
        );
        write(&root, "android/settings.gradle.kts", "include(\":app\")\n");
        write(
            &root,
            "android/app/build.gradle.kts",
            "plugins { id(\"com.android.application\") }\nandroid { defaultConfig { applicationId = \"com.acme.myapp\" } }\n",
        );
        write(
            &root,
            "web/package.json",
            "{\"devDependencies\": {\"vite\": \"^6\"}, \"dependencies\": {\"react\": \"^19\"}}",
        );
        root
    }

    #[test]
    fn adopt_writes_one_directory_and_leaves_the_app_alone() {
        let root = sample_repo("adopt-all");
        let before: Vec<String> = crate::fsutil::list_files(&root);
        run(&env(&root), &args(&root)).unwrap();
        let after = crate::fsutil::list_files(&root);
        for file in &before {
            assert!(after.contains(file), "{file} vanished");
        }
        let added: Vec<&String> = after.iter().filter(|f| !before.contains(f)).collect();
        assert!(
            added.iter().all(|f| f.starts_with("keel/")),
            "only keel/ is added: {added:?}"
        );
        for expected in [
            "keel/keel.toml",
            "keel/core/Cargo.toml",
            "keel/core/src/lib.rs",
            "keel/KEEL_ADOPT.md",
            "keel/generated/swift/Package.swift",
            "keel/generated/ts/src/index.ts",
        ] {
            assert!(
                after.iter().any(|f| f == expected),
                "missing {expected}: {after:#?}"
            );
        }
        // The app's own project file is byte for byte what it was.
        assert_eq!(
            std::fs::read_to_string(root.join("ios/MyApp.xcodeproj/project.pbxproj")).unwrap(),
            "PRODUCT_BUNDLE_IDENTIFIER = com.acme.myapp;\n"
        );
        // The project is a project, with the id the app already had.
        let project = Project::open(&root.join("keel")).unwrap();
        assert_eq!(project.config.id, "com.acme.myapp");
        assert_eq!(project.config.name, "myapp");
        assert_eq!(project.config.platforms, Platform::ALL.to_vec());
        let guide = std::fs::read_to_string(root.join("keel/KEEL_ADOPT.md")).unwrap();
        assert!(!guide.contains("@@"), "{guide}");
        assert!(
            guide.contains("## iOS") && guide.contains("## Android") && guide.contains("## Web"),
            "{guide}"
        );
        assert!(guide.contains("MyApp.xcodeproj") && guide.contains("-force_load $(SRCROOT)/../keel/build/ios/KeelCore.xcframework/ios-arm64/libkeel_core.a"), "{guide}");
        assert!(
            guide.contains(
                "project(\":core-bindings\").projectDir = file(\"../keel/generated/kotlin\")"
            ),
            "{guide}"
        );
        assert!(
            guide.contains("jniLibs.srcDir(\"../../keel/build/android/jniLibs\")"),
            "{guide}"
        );
        assert!(
            guide.contains("../keel/build/web/keel_core.wasm"),
            "{guide}"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn only_detected_or_requested_platforms_are_covered() {
        let root = sample_repo("adopt-one");
        let mut a = args(&root);
        a.platform = Some("web".into());
        run(&env(&root), &a).unwrap();
        let guide = std::fs::read_to_string(root.join("keel/KEEL_ADOPT.md")).unwrap();
        assert!(
            guide.contains("## Web") && !guide.contains("## iOS"),
            "{guide}"
        );
        assert!(!root.join("keel/generated/swift").exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_repository_with_no_app_is_explained() {
        let root = unique_temp_dir("adopt-none");
        write(&root, "README.md", "# nothing");
        let e = run(&env(&root), &args(&root)).unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(
            e.fix.contains("--platform") && e.fix.contains("keel init"),
            "{e}"
        );
        assert!(!root.join("keel").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_existing_keel_directory_is_never_merged_into() {
        let root = sample_repo("adopt-twice");
        run(&env(&root), &args(&root)).unwrap();
        let e = run(&env(&root), &args(&root)).unwrap_err();
        assert_eq!(e.code, Code::WouldOverwrite);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_groovy_gradle_build_gets_groovy_steps() {
        let root = unique_temp_dir("adopt-groovy");
        write(&root, "settings.gradle", "include ':app'\n");
        write(
            &root,
            "app/build.gradle",
            "plugins { id 'com.android.application' }\nandroid { defaultConfig { applicationId 'com.acme.g' } }\n",
        );
        run(&env(&root), &args(&root)).unwrap();
        let guide = std::fs::read_to_string(root.join("keel/KEEL_ADOPT.md")).unwrap();
        assert!(
            guide.contains("include ':core-bindings'")
                && guide.contains("implementation project(':core-bindings')"),
            "{guide}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ids_are_taken_from_the_existing_app() {
        let detected = Detected {
            ios: Some(IosApp {
                dir: PathBuf::new(),
                project: None,
                workspace: None,
                package: None,
                bundle_id: Some("Com.Acme.My-App".into()),
            }),
            ..Detected::default()
        };
        assert_eq!(detect_id(&detected).as_deref(), Some("com.acme.my_app"));
        assert_eq!(detect_id(&Detected::default()), None);
    }
}
