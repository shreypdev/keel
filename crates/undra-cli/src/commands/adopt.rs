//! `undra adopt`: add an Undra core to an app that already exists.
//!
//! Conservative by design. The app's own project files (an `.xcodeproj`, Gradle scripts, a
//! `package.json`) are never modified: editing them reliably means parsing formats that change
//! with every IDE release, and a wrong edit costs the team an afternoon. Instead `adopt` writes
//! one new directory, `undra/`, holding a core crate, an `undra.toml`, bindings for the platforms it
//! found, and `UNDRA_ADOPT.md`: the exact edits to make, with this repository's paths filled in.

use std::path::{Path, PathBuf};

use undra_bindgen::naming::CoreNames;

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

/// Runs `undra adopt`.
///
/// # Errors
///
/// `C0009` when the path is not a directory, names no app, or an argument is invalid; `C0008`
/// when `undra/` already exists with files in it.
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
            "`undra adopt` looks at an existing app repository and the path has to be a directory",
            "check the path; for a brand new app use `undra init <name>`",
        )
    })?;
    if !repo.is_dir() {
        return Err(CliError::bad_argument(
            format!("{} is not a directory", repo.display()),
            "`undra adopt` looks at an existing app repository",
            "pass the repository's directory",
        ));
    }

    let detected = detect(&repo);
    let platforms = choose_platforms(&detected, args.platform.as_deref(), &repo, env)?;
    let undra_dir = repo.join("undra");
    if !is_empty_dir(&undra_dir) {
        return Err(CliError::new(
            Code::WouldOverwrite,
            format!("{} already exists and is not empty", undra_dir.display()),
            "`undra adopt` writes one new directory and will not merge into an existing one",
            "if it is an Undra project already, use `undra -C undra <command>`; otherwise move it away and run `undra adopt` again",
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
    let undra_repo = match &args.undra_path {
        Some(path) => {
            let resolved = path.canonicalize().map_err(|e| {
                CliError::new(
                    Code::BadArgument,
                    format!("--undra-path {}: {e}", path.display()),
                    "the path has to be a checkout of the Undra repository",
                    "check the path, or leave --undra-path out to use released versions",
                )
            })?;
            require_checkout(&resolved)?;
            config.undra_path = Some(portable(
                &relative_path(&undra_dir, &resolved).unwrap_or_else(|| resolved.clone()),
            ));
            Some(resolved)
        }
        None => None,
    };
    let setup = Setup {
        root: canonicalize_lenient(&undra_dir),
        names,
        config,
        repo: undra_repo,
    };

    ui.step(&format!("Adding an Undra core to {}", repo.display()));
    let vars = variables(&setup);
    scaffold_core(&setup, &vars)?;
    let generated = generate_bindings(&setup)?;

    let runtimes = match &setup.repo {
        Some(repo) => Runtimes::in_repo(repo),
        None => Runtimes::from_registries(&setup.config.undra_version),
    };
    let steps = Steps {
        undra_dir: &setup.root,
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
            "UNDRA_SOURCE_NOTE",
            match &setup.repo {
                Some(r) => format!("The core uses the Undra crates, and the apps the Undra runtimes, from the checkout at `{}`.", r.display()),
                None => "The core depends on the released Undra crates; the apps on the matching runtimes (Swift package, Maven artifact, npm package).".to_owned(),
            },
        );
    let guide = guide_vars
        .render(templates::ADOPT_GUIDE.contents)
        .map_err(|name| {
            CliError::new(
                Code::ToolFailed,
                format!(
                    "an undra-cli template uses the placeholder @@{name}@@ and nothing sets it"
                ),
                "this is a bug in undra-cli, not in your arguments",
                "report it at https://github.com/shreypdev/undra/issues",
            )
        })?;
    write_if_changed(&setup.root.join(templates::ADOPT_GUIDE.path), &guide)?;

    let rel_undra = portable(
        relative_path(&repo, &setup.root)
            .as_deref()
            .unwrap_or(&setup.root),
    );
    ui.line(&format!(
        "Detected: {}. Created {rel_undra}/ with a core crate and bindings for schema hash {:#018x}; nothing else was changed.",
        detected.describe(&repo),
        generated.hash
    ));
    ui.line("");
    ui.line(&format!(
        "Steps to wire it in are in {rel_undra}/UNDRA_ADOPT.md. In short:"
    ));
    let core_names = CoreNames::new(
        &setup
            .config
            .core_namespace
            .clone()
            .unwrap_or_else(|| CoreNames::default_namespace(&setup.names.core_package)),
    );
    for platform in &platforms {
        ui.line(&format!(
            "  {}",
            summary_line(*platform, &rel_undra, &core_names)
        ));
    }
    ui.line(&format!(
        "  cd {rel_undra} && undra doctor && undra build --release"
    ));
    Ok(())
}

fn summary_line(platform: Platform, undra: &str, names: &CoreNames) -> String {
    match platform {
        Platform::Ios => format!(
            "iOS      add the package {undra}/generated/swift and link {undra}/build/ios/{}.xcframework",
            names.bundle()
        ),
        Platform::Android => format!(
            "Android  include {undra}/generated/kotlin as a Gradle module and package {undra}/build/android/jniLibs"
        ),
        Platform::Web => format!(
            "web      make @undra/runtime and the bindings in {undra}/generated/ts resolvable, load {undra}/build/web/{}.wasm",
            names.namespace()
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
            "`undra adopt` looks for an .xcodeproj or Package.swift, a Gradle build with the Android application plugin, or a package.json with a bundler or UI framework, up to four directories deep",
            "point it at the app's repository, say which platform you mean with `--platform ios|android|web`, or start from scratch with `undra init <name>`",
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
    undra_dir: &'a Path,
    names: &'a Names,
    config: &'a ProjectConfig,
    runtimes: &'a Runtimes,
}

impl Steps<'_> {
    fn rel(&self, from: &Path, to: &Path) -> String {
        portable(&relative_path(from, to).unwrap_or_else(|| to.to_path_buf()))
    }

    fn generated(&self) -> PathBuf {
        self.undra_dir.join(&self.config.generated)
    }

    fn build(&self) -> PathBuf {
        self.undra_dir.join(&self.config.build)
    }

    /// The names of the core (ADR-044): its namespace, libraries and generated entry.
    fn core_names(&self) -> CoreNames {
        CoreNames::new(
            &self
                .config
                .core_namespace
                .clone()
                .unwrap_or_else(|| CoreNames::default_namespace(&self.names.core_package)),
        )
    }

    fn ios(&self, app: &IosApp) -> String {
        let module = &self.names.swift_module;
        let package = self.rel(&app.dir, &self.generated().join("swift"));
        let core = self.core_names();
        let bundle = format!("{}.xcframework", core.bundle());
        let entry = core.entry();
        let library = format!("lib{}.a", core.namespace());
        let xcframework = self.rel(&app.dir, &self.build().join("ios").join(&bundle));
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
                "the `UndraRuntime` package at `{}` (the generated package depends on it)",
                dir.display()
            ),
            RuntimeRef::Registry { version } => {
                format!("`UndraRuntime` {version}.x from https://github.com/shreypdev/undra-swift")
            }
        };
        text.push_str(&format!(
            "   It pulls in {runtime}; add the product `UndraRuntime` to the app target too.\n"
        ));
        text.push_str(&format!(
            "2. **Link the core.** `undra build --platform ios` writes `{bundle}` (`{xcframework}`). Add it to the target's\n   *Frameworks, Libraries, and Embedded Content* (Do Not Embed). Each slice is one prelinked object (`{library}`) whose only\n   global symbol is the core's entry, which the generated package calls, so it needs no `-force_load` and other static\n   libraries (another Undra core included) link next to it.\n"
        ));
        text.push_str(&format!(
            "3. **Load the core at startup**, before any Undra object exists:\n\n   ```swift\n   import UndraRuntime\n   import {module}\n\n   @main struct MyApp: App {{\n       init() {{\n           do {{ try {entry}.load() }}\n           catch {{ fatalError(\"Undra did not start: \\(error)\") }}\n       }}\n       // ...\n   }}\n   ```\n\n   `{entry}` is the generated entry of the core: it checks the core was built from the bindings' schema, and every generated\n   type uses the core it loaded. Then use the generated store wherever a screen needs it: `@State private var todos = try! Todos()`\n   (a `@MainActor @Observable` class; read its properties in a SwiftUI view).\n"
        ));
        text.push_str(&format!("4. **Optional, for `undra dev`:** add `NSAppTransportSecurity > NSAllowsLocalNetworking = YES` to the Info.plist so the app may connect to\n   `ws://` on your network, and call `{entry}.load(.remote(url: \"ws://<your Mac>:7443\"))` in debug builds. The runtime reconnects by itself (`core.connectionState`); when `undra dev` restarts the core it reports `.closed(.sessionLost)` and the app loads a new core.\n\n"));
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
                "implementation(project(\":core-bindings\"))\n   implementation(\"dev.undra:runtime:{version}\")\n   implementation(\"dev.undra:android-adapters:{version}\")"
            )
        } else {
            format!(
                "implementation project(':core-bindings')\n   implementation 'dev.undra:runtime:{version}'\n   implementation 'dev.undra:android-adapters:{version}'"
            )
        };
        let jni_line = if kts {
            format!("sourceSets {{ getByName(\"main\").jniLibs.srcDir(\"{jni}\") }}")
        } else {
            format!("sourceSets {{ main {{ jniLibs.srcDir '{jni}' }} }}")
        };
        text.push_str(&format!(
            "2. **Depend on it and package the native libraries** in the app module's `{build}` (inside `android {{ }}` for the\n   source set):\n\n   ```\n   dependencies {{\n   {deps}\n   }}\n   android {{\n       {jni_line}\n   }}\n   ```\n\n   `undra build --platform android --release` writes `lib{namespace}.so` for the ABIs in undra.toml (arm64-v8a, x86_64) below\n   `build/android/jniLibs`; use `--release` when you package (a debug core is tens of megabytes per ABI).\n   The app needs `minSdk` {} or higher and Kotlin/Java 11 bytecode.\n",
            self.config.android.min_sdk,
            namespace = self.core_names().namespace(),
        ));
        text.push_str(&format!(
            "3. **Load the core once per process**, in your `Application` subclass (register it with `android:name` in the manifest):\n\n   ```kotlin\n   import {package}.{entry}\n   import dev.undra.android.AndroidPlatformDefaults\n\n   class App : Application() {{\n       override fun onCreate() {{\n           super.onCreate()\n           val core = {entry}.load()\n           AndroidPlatformDefaults.install(core, this)\n       }}\n   }}\n   ```\n\n   `{entry}` is the generated entry of the core: it loads `lib{namespace}.so`, checks the core was built from the bindings'\n   schema, and every generated class uses the core it loaded.\n   `install` gives the core every platform port: `Http`, `Kv`, `SecureStore`, `Fs`, `Connectivity` and `Lifecycle` (without it an Android core has no network or\n   storage). It needs the `INTERNET` and `ACCESS_NETWORK_STATE` permissions; `android-adapters` declares both, so they merge into your manifest.\n   Then `val todos = Todos()` (a generated store; its `StateFlow` properties work with `collectAsState()`).\n",
            entry = self.core_names().entry(),
            namespace = self.core_names().namespace(),
        ));
        text.push_str("4. **Shrinking.** Nothing to add: the core's `JNI_OnLoad` registers the natives of the generated `UndraCoreNative` by name, and the\n   bindings and the runtime ship the R8 rules that keep them (`META-INF/proguard`).\n\n");
        text.push_str("5. **Optional, for `undra dev`:** in debug builds pass `mode = Mode.REMOTE, remoteUrl = \"ws://10.0.2.2:7443\"` (the emulator's name for your computer; a USB device uses `adb reverse tcp:7443 tcp:7443`\n   and `ws://127.0.0.1:7443`, which `undra dev --android` sets up) to `LoadOptions`, and allow cleartext traffic in a **debug-only** manifest\n   (`app/src/debug/AndroidManifest.xml`: `<application android:usesCleartextTraffic=\"true\" />`; the `INTERNET` permission is already in your manifest through `android-adapters`).\n   The runtime reconnects by itself (`core.connectionState`); when `undra dev` restarts the core it reports `Closed(SESSION_LOST)` and the app loads a new core.\n\n");
        text
    }

    fn web(&self, app: &WebApp) -> String {
        let generated = self.rel(&app.dir, &self.generated().join("ts"));
        let core = self.core_names();
        let entry = core.entry();
        let wasm_file = format!("{}.wasm", core.namespace());
        let wasm = self.rel(&app.dir, &self.build().join("web").join(&wasm_file));
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
                    "1. **Resolve the runtime and the bindings.** Both are used from their TypeScript sources, so there is no build step between a core\n   change and the browser. Alias them in your bundler (Vite shown; webpack `resolve.alias` and a `tsconfig.json` `paths` entry work the same way):\n\n   ```ts\n   resolve: {{ alias: {{\n     \"@undra/runtime\": fileURLToPath(new URL(\"{src}\", import.meta.url)),\n     \"{ts_package}\": fileURLToPath(new URL(\"{generated}/src/index.ts\", import.meta.url)),\n   }} }},\n   server: {{ fs: {{ allow: [\"..\", \"{}\"] }} }},\n   ```\n",
                    self.rel(&app.dir, dir)
                ));
            }
            RuntimeRef::Registry { version } => {
                text.push_str(&format!(
                    "1. **Install the runtime and the bindings.**\n\n   ```sh\n   npm install @undra/runtime@^{version}.0 {generated}\n   ```\n\n   (`{generated}` is the generated package, `{ts_package}`; it imports `@undra/runtime` as a peer dependency.)\n"
                ));
            }
        }
        text.push_str(&format!(
            "2. **Load the core before rendering**, in the app's entry point:\n\n   ```ts\n   import {{ {entry}, Todos }} from \"{ts_package}\";\n   import wasmUrl from \"{wasm}?url\"; // a bundler asset: Vite shown; webpack 5: new URL(\"{wasm}\", import.meta.url)\n\n   await {entry}.load({{ mode: \"wasm-main\", wasm: new URL(wasmUrl, location.href) }});\n   const todos = await Todos.create();\n   ```\n\n   `undra build --platform web` writes `build/web/{wasm_file}`; `{entry}` checks the core was built from the bindings' schema, and every\n   generated class uses the core it loaded. Signals are `todos.visible.get()` / `.subscribe(fn)`; in React read them with\n   `useSyncExternalStore` (see the `web/` app of an `undra init` project).\n"
        ));
        text.push_str(&format!("3. **Optional, for `undra dev`:** `await {entry}.load({{ mode: \"remote\", url: \"ws://127.0.0.1:7443\" }})`.\n\n"));
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
            undra_path: None,
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
            added.iter().all(|f| f.starts_with("undra/")),
            "only undra/ is added: {added:?}"
        );
        for expected in [
            "undra/undra.toml",
            "undra/core/Cargo.toml",
            "undra/core/src/lib.rs",
            "undra/UNDRA_ADOPT.md",
            "undra/generated/swift/Package.swift",
            "undra/generated/ts/src/index.ts",
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
        let project = Project::open(&root.join("undra")).unwrap();
        assert_eq!(project.config.id, "com.acme.myapp");
        assert_eq!(project.config.name, "myapp");
        assert_eq!(project.config.platforms, Platform::ALL.to_vec());
        let guide = std::fs::read_to_string(root.join("undra/UNDRA_ADOPT.md")).unwrap();
        assert!(!guide.contains("@@"), "{guide}");
        assert!(
            guide.contains("## iOS") && guide.contains("## Android") && guide.contains("## Web"),
            "{guide}"
        );
        assert!(
            guide.contains("MyApp.xcodeproj")
                && guide.contains("`../undra/build/ios/MyappCore.xcframework`")
                && guide.contains("try UndraMyappCore.load()"),
            "{guide}"
        );
        assert!(
            !guide.contains("-force_load $(") && !guide.contains("UNDRA_LINK_CORE"),
            "{guide}"
        );
        assert!(
            guide.contains(
                "project(\":core-bindings\").projectDir = file(\"../undra/generated/kotlin\")"
            ),
            "{guide}"
        );
        assert!(
            guide.contains("jniLibs.srcDir(\"../../undra/build/android/jniLibs\")"),
            "{guide}"
        );
        assert!(
            guide.contains("../undra/build/web/myapp_core.wasm")
                && guide.contains("await UndraMyappCore.load({ mode: \"wasm-main\""),
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
        let guide = std::fs::read_to_string(root.join("undra/UNDRA_ADOPT.md")).unwrap();
        assert!(
            guide.contains("## Web") && !guide.contains("## iOS"),
            "{guide}"
        );
        assert!(!root.join("undra/generated/swift").exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_repository_with_no_app_is_explained() {
        let root = unique_temp_dir("adopt-none");
        write(&root, "README.md", "# nothing");
        let e = run(&env(&root), &args(&root)).unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(
            e.fix.contains("--platform") && e.fix.contains("undra init"),
            "{e}"
        );
        assert!(!root.join("undra").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_existing_undra_directory_is_never_merged_into() {
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
        let guide = std::fs::read_to_string(root.join("undra/UNDRA_ADOPT.md")).unwrap();
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
