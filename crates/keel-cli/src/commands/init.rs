//! `keel init`: a new project.
//!
//! The project is written from templates compiled into the binary, then its bindings are
//! generated from the template core's schema (embedded too, and checked against a real build by
//! the tests), so what `init` leaves behind is complete: the core compiles, the three app shells
//! import bindings that exist, and `keel bindgen --check` passes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use keel_bindgen::Generator;

use crate::bindgen::{self, Plan, canonicalize_lenient};
use crate::cli::InitArgs;
use crate::config::{KEEL_VERSION, Platform, ProjectConfig};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{self, create_dir_all, is_empty_dir, make_executable, write_if_changed};
use crate::names::{Names, portable, relative_path, validate_app_id, validate_project_name};
use crate::project::{CONFIG_FILE, Project};
use crate::render::{TemplateFile, Vars};
use crate::runtimes::{KOTLIN_IN_REPO, RuntimeRef, Runtimes, require_checkout};
use crate::schema::parse_schema_json;
use crate::templates;

use super::Env;

/// The Gradle version the wrapper is created with when there is no checkout to copy one from
/// (the Kotlin runtime's own wrapper version).
const GRADLE_VERSION: &str = "8.14.3";

/// What `init` decided before writing anything.
pub(super) struct Setup {
    // (fields below are read by `keel adopt` too)
    pub(super) root: PathBuf,
    pub(super) names: Names,
    pub(super) config: ProjectConfig,
    pub(super) repo: Option<PathBuf>,
}

/// Runs `keel init`.
///
/// # Errors
///
/// `C0009` for a bad name, platform list or id, `C0008` when the directory is not empty, `C0010`
/// when a file cannot be written.
pub fn run(env: &Env<'_>, args: &InitArgs) -> Result<()> {
    let setup = prepare(env, args)?;
    let ui = env.ui;
    ui.step(&format!("Creating {} in {}", setup.names.project, setup.root.display()));
    let files = scaffold(&setup)?;
    let bindings = generate_bindings(&setup)?;
    if setup.config.platforms.contains(&Platform::Android) {
        gradle_wrapper(env, &setup);
    }

    ui.line(&format!(
        "Created {} ({} files, bindings for schema hash {:#018x}).",
        setup.root.display(),
        files + bindings.files,
        bindings.hash
    ));
    ui.line("");
    ui.line(&next_steps(&setup));
    Ok(())
}

/// Validates the arguments and works out where everything goes.
fn prepare(env: &Env<'_>, args: &InitArgs) -> Result<Setup> {
    validate_project_name(&args.name)?;
    let platforms = Platform::parse_list(&args.platforms)?;
    let names = Names::derive(&args.name);
    let id = args.id.clone().unwrap_or_else(|| names.default_app_id());
    validate_app_id(&id)?;

    let parent = match &args.dir {
        Some(dir) => dir.clone(),
        None => env.start_dir()?,
    };
    let root = canonicalize_lenient(&parent.join(&args.name));
    if !is_empty_dir(&root) {
        return Err(CliError::new(
            Code::WouldOverwrite,
            format!("{} already exists and is not empty", root.display()),
            "`keel init` creates a new project and will not write into a directory that has files in it",
            format!("choose another name, or another parent with `--dir`; to add Keel to an existing app use `keel adopt {}`", root.display()),
        ));
    }

    let mut config = ProjectConfig::new(&args.name, &id, platforms);
    let repo = match &args.keel_path {
        Some(path) => {
            let repo = path.canonicalize().map_err(|e| {
                CliError::new(
                    Code::BadArgument,
                    format!("--keel-path {}: {e}", path.display()),
                    "the path has to be a checkout of the Keel repository",
                    "check the path, or leave --keel-path out to use released versions",
                )
            })?;
            require_checkout(&repo)?;
            let shown = relative_path(&root, &repo).map_or_else(|| repo.clone(), |p| p);
            config.keel_path = Some(portable(&shown));
            Some(repo)
        }
        None => None,
    };
    Ok(Setup {
        root,
        names,
        config,
        repo,
    })
}

/// The path from `from` (a directory of the project) to `to`, as text for a config file.
fn rel(from: &Path, to: &Path) -> String {
    portable(&relative_path(from, to).unwrap_or_else(|| to.to_path_buf()))
}

/// Every placeholder the templates use.
pub(super) fn variables(setup: &Setup) -> Vars {
    let Setup {
        root, names, config, repo, ..
    } = setup;
    let generated = root.join(&config.generated);
    let build = root.join(&config.build);
    let kotlin_package = format!("{}.core", config.id);
    let ts_package = format!("@app/{}", names.core_package);
    let id_path = config.id.replace('.', "/");

    // Where Keel comes from, as the core's Cargo.toml and the shells say it.
    let keel_dep = match repo {
        Some(repo) => format!("{{ path = {} }}", crate::toml_lite::quote(&rel(&root.join("core"), &repo.join("crates/keel")))),
        None => format!("\"{KEEL_VERSION}\""),
    };
    let runtimes = match repo {
        Some(repo) => Runtimes::in_repo(repo),
        None => Runtimes::from_registries(KEEL_VERSION),
    };

    let mut vars = Vars::new()
        .with("NAME", names.project.clone())
        .with("KEBAB", names.kebab.clone())
        .with("SNAKE", names.snake.clone())
        .with("PASCAL", names.pascal.clone())
        .with("APP", names.pascal.clone())
        .with("APP_ID", config.id.clone())
        .with("APP_ID_PATH", id_path)
        .with("CORE_PACKAGE", names.core_package.clone())
        .with("CORE_LIB", names.core_lib.clone())
        .with("SWIFT_MODULE", names.swift_module.clone())
        .with("KOTLIN_PACKAGE", kotlin_package)
        .with("TS_PACKAGE", ts_package)
        .with("KEEL_DEP", keel_dep)
        .with("KEEL_VERSION", KEEL_VERSION);

    // iOS
    let ios_dir = root.join("ios");
    let slice_sim = if config.ios.simulator_archs.len() > 1 { "ios-arm64_x86_64-simulator" } else if config.ios.simulator_archs[0] == "x86_64" { "ios-x86_64-simulator" } else { "ios-arm64-simulator" };
    let xcframework = build.join("ios/KeelCore.xcframework");
    let xcframework_rel = rel(&ios_dir, &xcframework);
    let link_flags = format!(
        "\"OTHER_LDFLAGS[sdk=iphoneos*]\" = (\n\t\t\t\t\t\"$(inherited)\",\n\t\t\t\t\t\"-force_load\",\n\t\t\t\t\t\"$(SRCROOT)/{xcframework_rel}/ios-arm64/libkeel_core.a\",\n\t\t\t\t);\n\t\t\t\t\"OTHER_LDFLAGS[sdk=iphonesimulator*]\" = (\n\t\t\t\t\t\"$(inherited)\",\n\t\t\t\t\t\"-force_load\",\n\t\t\t\t\t\"$(SRCROOT)/{xcframework_rel}/{slice_sim}/libkeel_core.a\",\n\t\t\t\t);"
    );
    vars.set("XCFRAMEWORK_PATH", xcframework_rel);
    vars.set("LINK_FLAGS", link_flags);
    vars.set("DEPLOYMENT_TARGET", config.ios.deployment_target.clone());
    vars.set("PACKAGE_REFERENCE_SECTIONS", package_references(&rel(&ios_dir, &generated.join("swift")), &runtimes.swift, &ios_dir));

    // Android
    let android_dir = root.join("android");
    vars.set("GENERATED_KOTLIN_PATH", rel(&android_dir, &generated.join("kotlin")));
    vars.set("JNI_LIBS_PATH", rel(&android_dir.join("app"), &build.join("android/jniLibs")));
    vars.set("MIN_SDK", config.android.min_sdk.to_string());
    vars.set(
        "ABI_FILTERS",
        config.android.abis.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", "),
    );
    match &runtimes.kotlin {
        RuntimeRef::Path(dir) => {
            vars.set(
                "KOTLIN_RUNTIME_BUILD",
                format!(
                    "\n// The Kotlin runtime, built from the Keel checkout (a composite build: `dev.keel:runtime` below\n// resolves to it, so runtime edits show up without publishing).\nincludeBuild(\"{}\")\n",
                    rel(&android_dir, dir)
                ),
            );
            vars.set("KOTLIN_RUNTIME_VERSION", "0.1.0-SNAPSHOT");
        }
        RuntimeRef::Registry { version } => {
            vars.set("KOTLIN_RUNTIME_BUILD", "");
            vars.set("KOTLIN_RUNTIME_VERSION", format!("{version}.0"));
        }
    }

    // Web
    let web_dir = root.join("web");
    vars.set("GENERATED_TS_PATH", rel(&web_dir, &generated.join("ts")));
    vars.set("PROJECT_ROOT_PATH", rel(&web_dir, root));
    vars.set("WASM_IMPORT", rel(&web_dir.join("src"), &build.join("web/keel_core.wasm")));
    match &runtimes.ts {
        RuntimeRef::Path(dir) => {
            let runtime_src = rel(&web_dir, &dir.join("src/index.ts"));
            vars.set("RUNTIME_DEPENDENCY", "");
            vars.set(
                "RUNTIME_ALIAS",
                format!("      // The Keel runtime, from the Keel checkout's sources.\n      \"@keel/runtime\": here(\"{runtime_src}\"),\n"),
            );
            vars.set("RUNTIME_PATHS", format!(",\n      \"@keel/runtime\": [\"{runtime_src}\"]"));
            vars.set("EXTRA_FS_ALLOW", format!(", here(\"{}\")", rel(&web_dir, dir)));
        }
        RuntimeRef::Registry { version } => {
            vars.set("RUNTIME_DEPENDENCY", format!("\"@keel/runtime\": \"^{version}.0\",\n    "));
            vars.set("RUNTIME_ALIAS", "");
            vars.set("RUNTIME_PATHS", "");
            vars.set("EXTRA_FS_ALLOW", "");
        }
    }
    vars
}

/// The `XCLocalSwiftPackageReference` / `XCRemoteSwiftPackageReference` sections of the Xcode
/// project: the generated package is always local, the runtime is local in a checkout and a
/// versioned remote otherwise.
fn package_references(generated: &str, runtime: &RuntimeRef, ios_dir: &Path) -> String {
    let mut out = format!(
        "/* Begin XCLocalSwiftPackageReference section */\n\t\tA0A0A0A0A0A0A0A000000120 /* generated Swift package */ = {{\n\t\t\tisa = XCLocalSwiftPackageReference;\n\t\t\trelativePath = \"{generated}\";\n\t\t}};\n"
    );
    match runtime {
        RuntimeRef::Path(dir) => {
            out.push_str(&format!(
                "\t\tA0A0A0A0A0A0A0A000000121 /* KeelRuntime package */ = {{\n\t\t\tisa = XCLocalSwiftPackageReference;\n\t\t\trelativePath = \"{}\";\n\t\t}};\n/* End XCLocalSwiftPackageReference section */",
                rel(ios_dir, dir)
            ));
        }
        RuntimeRef::Registry { version } => {
            out.push_str("/* End XCLocalSwiftPackageReference section */\n\n/* Begin XCRemoteSwiftPackageReference section */\n");
            out.push_str(&format!(
                "\t\tA0A0A0A0A0A0A0A000000121 /* KeelRuntime package */ = {{\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/shreypdev/keel-swift\";\n\t\t\trequirement = {{\n\t\t\t\tkind = upToNextMajorVersion;\n\t\t\t\tminimumVersion = {version}.0;\n\t\t\t}};\n\t\t}};\n/* End XCRemoteSwiftPackageReference section */"
            ));
        }
    }
    out
}

/// Writes `keel.toml`, the Cargo workspace and the core crate: what a project is without app
/// shells (`keel adopt` stops here). Returns how many files.
pub(super) fn scaffold_core(setup: &Setup, vars: &Vars) -> Result<usize> {
    create_dir_all(&setup.root)?;
    write_if_changed(&setup.root.join(CONFIG_FILE), &setup.config.render())?;
    let mut count = 1;
    count += write_set(&setup.root, templates::PROJECT, vars)?;
    count += write_set(&setup.root, templates::CORE, vars)?;
    Ok(count)
}

/// Writes the project's own files, the core and the shells. Returns how many files.
fn scaffold(setup: &Setup) -> Result<usize> {
    let vars = variables(setup);
    let mut count = scaffold_core(setup, &vars)?;
    for platform in &setup.config.platforms {
        count += match platform {
            Platform::Ios => write_set(&setup.root, templates::IOS, &vars)?,
            Platform::Android => write_set(&setup.root, templates::ANDROID, &vars)?,
            Platform::Web => write_set(&setup.root, templates::WEB, &vars)?,
        };
    }
    count += write_set(&setup.root, std::slice::from_ref(&templates::README), &readme_vars(setup, vars))?;
    Ok(count)
}

/// Renders every file of `set` below `root`.
pub(super) fn write_set(root: &Path, set: &[TemplateFile], vars: &Vars) -> Result<usize> {
    for file in set {
        let path = vars.render(file.path).map_err(template_bug)?;
        let contents = vars.render(file.contents).map_err(template_bug)?;
        let dest = root.join(&path);
        write_if_changed(&dest, &contents)?;
        if file.executable {
            make_executable(&dest)?;
        }
    }
    Ok(set.len())
}

fn template_bug(name: String) -> CliError {
    CliError::new(
        Code::ToolFailed,
        format!("a keel-cli template uses the placeholder @@{name}@@ and nothing sets it"),
        "this is a bug in keel-cli, not in your arguments",
        "report it at https://github.com/shreypdev/keel/issues",
    )
}

/// Variables of the README on top of the shared ones.
fn readme_vars(setup: &Setup, vars: Vars) -> Vars {
    let has = |p: Platform| setup.config.platforms.contains(&p);
    let mut vars = vars;
    vars.set("PLATFORM_LIST", setup.config.platforms.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "));
    // The per-platform sections are templates themselves: render them with the shared values.
    let section = |vars: &Vars, platform: Platform, text: &str| {
        if has(platform) { vars.render(text).unwrap_or_default() } else { String::new() }
    };
    let (ios, android, web) = (
        section(&vars, Platform::Ios, README_IOS),
        section(&vars, Platform::Android, README_ANDROID),
        section(&vars, Platform::Web, README_WEB),
    );
    vars.set("README_IOS", ios);
    vars.set("README_ANDROID", android);
    vars.set("README_WEB", web);
    vars.set("KEEL_SOURCE_NOTE", match &setup.repo {
        Some(repo) => format!(
            "The core uses the Keel crates, and the apps the Keel runtimes, from the checkout at `{}` (`[keel] path` in keel.toml).",
            repo.display()
        ),
        None => format!("The core depends on Keel {KEEL_VERSION} from crates.io; the apps on the matching runtimes (Swift package, Maven artifact, npm package)."),
    });
    vars
}

const README_IOS: &str = "
## iOS

```sh
keel build --platform ios                     # build/ios/KeelCore.xcframework (device + simulator)
KEEL_LINK_CORE=1 xcodebuild -project ios/@@APP@@.xcodeproj -scheme @@APP@@ \\
    -destination 'generic/platform=iOS Simulator' build
```

Open `ios/@@APP@@.xcodeproj` in Xcode 16 or newer to run it. The app target links the XCFramework with
`-force_load` (a debug static library has many object files and the linker would otherwise drop the ones
that register the core's `#[keel::api]` items; release builds do not need it, and it is harmless there).

**`KEEL_LINK_CORE=1`.** The Swift runtime package ships link-time stand-ins for the core so that it builds
and tests without one; that variable switches them off so the real core is linked. Xcode reads it when it
resolves packages, so start Xcode with it set: `KEEL_LINK_CORE=1 open --env KEEL_LINK_CORE=1 -a Xcode ios/@@APP@@.xcodeproj`,
or build from the command line as above.

**Against `keel dev`.** In the scheme's Run environment variables set `KEEL_DEV_URL` to the `ws://` URL
`keel dev` prints (a simulator can use `ws://127.0.0.1:7443`; a device needs your computer's address and
`keel dev --addr 0.0.0.0:7443`). Debug builds then use that core instead of the linked one.
";

const README_ANDROID: &str = "
## Android

```sh
keel build --platform android                 # build/android/jniLibs/<abi>/libkeel_core.so
cd android && ./gradlew :app:assembleDebug    # or open android/ in Android Studio
```

The app module packages `build/android/jniLibs` and depends on the generated Kotlin module
(`generated/kotlin`, included by `android/settings.gradle.kts`) and on the Keel runtime (`dev.keel:runtime`).
The Kotlin runtime has no remote transport on Android yet, so `keel dev` serves the web and iOS shells and
the JVM; an Android build always runs the core in process.
";

const README_WEB: &str = "
## Web

```sh
keel build --platform web                     # build/web/keel_core.wasm
cd web && npm install && npm run dev          # http://localhost:5173
```

The page loads the wasm core and runs it on the main thread. Add `?keel=ws://127.0.0.1:7443` to the URL
(with `keel dev` running) to use the core that `keel dev` serves instead, so a Rust change needs only a
reload. `npm run build` type-checks and bundles.
";

/// What `init` generated from the embedded schema.
pub(super) struct Generated {
    pub(super) files: usize,
    pub(super) hash: u64,
}

/// Generates the bindings of the template core into `generated/`, exactly as `keel bindgen`
/// would.
pub(super) fn generate_bindings(setup: &Setup) -> Result<Generated> {
    let schema = parse_schema_json(templates::CORE_SCHEMA, &setup.names.core_package)?;
    let project = Project {
        root: setup.root.clone(),
        config: setup.config.clone(),
    };
    let mut generator = Generator::for_crate(&setup.names.core_package);
    generator.swift_module = project.swift_module();
    generator.kotlin_package = project.kotlin_package();
    let plan = Plan {
        generator,
        platforms: setup.config.platforms.clone(),
        runtimes: Runtimes::for_project(&project),
        out: canonicalize_lenient(&project.generated_dir()),
        keel_version: setup.config.keel_version.clone(),
    };
    let files = bindgen::plan_files(&schema, &plan)?;
    bindgen::apply(&plan.out, &files)?;
    Ok(Generated {
        files: files.len(),
        hash: schema.hash(),
    })
}

/// Gives the Android project a Gradle wrapper: copied from the Keel checkout's Kotlin runtime
/// when there is one, else created with `gradle wrapper` when Gradle is installed.
fn gradle_wrapper(env: &Env<'_>, setup: &Setup) {
    let android = setup.root.join("android");
    if let Some(repo) = &setup.repo {
        let source = repo.join(KOTLIN_IN_REPO);
        let mut copied = true;
        for file in ["gradlew", "gradlew.bat", "gradle/wrapper/gradle-wrapper.jar", "gradle/wrapper/gradle-wrapper.properties"] {
            let from = source.join(file);
            if from.is_file() {
                if fsutil::copy_file(&from, &android.join(file)).is_err() {
                    copied = false;
                }
            } else {
                copied = false;
            }
        }
        if copied {
            let _ = make_executable(&android.join("gradlew"));
            return;
        }
    }
    let Some(gradle) = env.sys.which("gradle", &[]) else {
        env.ui.warn("Gradle is not installed, so android/ has no ./gradlew; Android Studio creates one when it opens the project (or run `gradle wrapper` in android/)");
        return;
    };
    let status = Command::new(gradle)
        .args(["wrapper", "--gradle-version", GRADLE_VERSION, "--no-validate-url"])
        .current_dir(&android)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !status.is_ok_and(|s| s.success()) {
        env.ui.warn("could not create the Gradle wrapper; Android Studio creates one when it opens android/");
    }
}

/// The text printed after a successful `init`.
fn next_steps(setup: &Setup) -> String {
    let name = &setup.names.project;
    let mut out = format!("Next:\n  cd {name}\n  keel doctor                 check this machine\n  keel dev                    serve the core to a running app, rebuilding on change\n  keel build --release        libraries for the apps to link\n");
    out.push_str("\nThen run an app:\n");
    for platform in &setup.config.platforms {
        match platform {
            Platform::Ios => out.push_str("  iOS      open ios/ in Xcode (README.md: the KEEL_LINK_CORE=1 note)\n"),
            Platform::Android => out.push_str("  Android  open android/ in Android Studio, or ./gradlew :app:installDebug\n"),
            Platform::Web => out.push_str("  web      cd web && npm install && npm run dev\n"),
        }
    }
    out.push_str("\nREADME.md has the details for each platform.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn args(name: &str, platforms: &str) -> InitArgs {
        InitArgs {
            name: name.to_owned(),
            platforms: platforms.to_owned(),
            id: None,
            keel_path: None,
            dir: None,
        }
    }

    #[test]
    fn init_creates_a_complete_project_from_the_templates() {
        let parent = fsutil::unique_temp_dir("init-unit");
        create_dir_all(&parent).unwrap();
        run(&env(&parent), &args("todo-app", "ios,android,web")).unwrap();
        let root = parent.canonicalize().unwrap().join("todo-app");
        let files = fsutil::list_files(&root);
        for expected in [
            "keel.toml",
            "Cargo.toml",
            ".gitignore",
            "README.md",
            "core/Cargo.toml",
            "core/src/lib.rs",
            "ios/TodoApp.xcodeproj/project.pbxproj",
            "ios/TodoApp/MainApp.swift",
            "android/app/src/main/kotlin/com/example/todoapp/MainActivity.kt",
            "android/settings.gradle.kts",
            "web/package.json",
            "web/src/App.tsx",
            "generated/swift/Package.swift",
            "generated/swift/Sources/TodoAppCore/Generated/Stores.swift",
            "generated/kotlin/src/main/kotlin/com/example/todoapp/core/Stores.kt",
            "generated/ts/src/stores.ts",
            "generated/.keel-generated",
        ] {
            assert!(files.iter().any(|f| f == expected), "missing {expected}; have {files:#?}");
        }
        // No placeholder survives rendering.
        for file in &files {
            if let Ok(text) = std::fs::read_to_string(root.join(file)) {
                assert!(!text.contains("@@"), "{file} still has a placeholder");
            }
        }
        // The project reads back as a project.
        let project = Project::open(&root).unwrap();
        assert_eq!(project.config.name, "todo-app");
        assert_eq!(project.config.id, "com.example.todoapp");
        assert_eq!(project.config.platforms, Platform::ALL.to_vec());
        let core = std::fs::read_to_string(root.join("core/Cargo.toml")).unwrap();
        assert!(core.contains("name = \"todo-app-core\"") && core.contains("keel = \"0.1\""), "{core}");
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn only_the_chosen_platforms_get_shells() {
        let parent = fsutil::unique_temp_dir("init-web");
        create_dir_all(&parent).unwrap();
        run(&env(&parent), &args("site", "web")).unwrap();
        let root = parent.canonicalize().unwrap().join("site");
        assert!(root.join("web/package.json").is_file());
        assert!(!root.join("ios").exists() && !root.join("android").exists());
        assert!(root.join("generated/ts/src/index.ts").is_file());
        assert!(!root.join("generated/swift").exists());
        let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
        assert!(readme.contains("## Web") && !readme.contains("## iOS"), "{readme}");
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn a_non_empty_directory_is_refused_with_the_way_out() {
        let parent = fsutil::unique_temp_dir("init-exists");
        create_dir_all(&parent.join("taken")).unwrap();
        std::fs::write(parent.join("taken/file"), "x").unwrap();
        let e = run(&env(&parent), &args("taken", "web")).unwrap_err();
        assert_eq!(e.code, Code::WouldOverwrite);
        assert!(e.fix.contains("keel adopt"), "{e}");
        assert_eq!(std::fs::read_to_string(parent.join("taken/file")).unwrap(), "x");
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn bad_arguments_are_explained_before_anything_is_written() {
        let parent = fsutil::unique_temp_dir("init-bad");
        create_dir_all(&parent).unwrap();
        assert_eq!(run(&env(&parent), &args("1bad", "web")).unwrap_err().code, Code::BadArgument);
        assert_eq!(run(&env(&parent), &args("ok", "tv")).unwrap_err().code, Code::BadArgument);
        let mut with_id = args("ok", "web");
        with_id.id = Some("Bad_Id".into());
        assert_eq!(run(&env(&parent), &with_id).unwrap_err().code, Code::BadArgument);
        let mut with_path = args("ok", "web");
        with_path.keel_path = Some(parent.join("not-a-checkout"));
        assert_eq!(run(&env(&parent), &with_path).unwrap_err().code, Code::BadArgument);
        assert!(fsutil::is_empty_dir(&parent), "nothing was written");
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn the_embedded_schema_is_canonical_and_generates_bindings() {
        let schema = parse_schema_json(templates::CORE_SCHEMA, "demo-core").unwrap();
        assert_eq!(schema.canonical_json(), templates::CORE_SCHEMA.trim());
        assert!(schema.objects.iter().any(|o| o.name == "Todos"));
    }

    #[test]
    fn checkout_mode_writes_relative_paths_the_shells_can_follow() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let parent = fsutil::unique_temp_dir("init-path");
        create_dir_all(&parent).unwrap();
        let mut a = args("demo", "ios,android,web");
        a.keel_path = Some(repo.clone());
        run(&env(&parent), &a).unwrap();
        let root = parent.canonicalize().unwrap().join("demo");
        let core = std::fs::read_to_string(root.join("core/Cargo.toml")).unwrap();
        // The path in core/Cargo.toml resolves to the checkout's `keel` crate.
        let dep = core.lines().find(|l| l.starts_with("keel = ")).unwrap();
        let path = dep.split('"').nth(1).unwrap();
        assert!(root.join("core").join(path).join("Cargo.toml").is_file(), "{dep}");
        let project = Project::open(&root).unwrap();
        assert_eq!(project.keel_repo().unwrap(), repo);
        let pbx = std::fs::read_to_string(root.join("ios/Demo.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbx.contains("XCLocalSwiftPackageReference") && pbx.contains("runtimes/swift/KeelRuntime"), "pbxproj");
        let settings = std::fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(settings.contains("includeBuild(") && settings.contains("runtimes/kotlin/keel-runtime"), "{settings}");
        assert!(root.join("android/gradlew").is_file(), "the wrapper is copied from the checkout");
        let vite = std::fs::read_to_string(root.join("web/vite.config.ts")).unwrap();
        assert!(vite.contains("@keel/runtime") && vite.contains("runtimes/ts/@keel/runtime/src/index.ts"), "{vite}");
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn a_registry_project_names_published_packages() {
        let parent = fsutil::unique_temp_dir("init-registry");
        create_dir_all(&parent).unwrap();
        run(&env(&parent), &args("demo", "ios,android,web")).unwrap();
        let root = parent.canonicalize().unwrap().join("demo");
        let pbx = std::fs::read_to_string(root.join("ios/Demo.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbx.contains("XCRemoteSwiftPackageReference") && pbx.contains("keel-swift"), "pbxproj");
        let app = std::fs::read_to_string(root.join("android/app/build.gradle.kts")).unwrap();
        assert!(app.contains("dev.keel:runtime:0.1.0\""), "{app}");
        let pkg = std::fs::read_to_string(root.join("web/package.json")).unwrap();
        assert!(pkg.contains("\"@keel/runtime\": \"^0.1.0\""), "{pkg}");
        let _ = std::fs::remove_dir_all(parent);
    }
}
