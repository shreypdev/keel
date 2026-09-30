//! The command line: arguments and the help text that goes with them.
//!
//! The help is meant to teach: each command says what it does, what it needs, what it writes and
//! shows an example, so `keel <command> --help` is enough to use it.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// The `keel` command.
#[derive(Parser, Debug)]
#[command(
    name = "keel",
    version,
    about = "Keel: one Rust core, native iOS, Android and web apps",
    long_about = "Keel owns everything under the pixels of a native app (domain logic, reactive state, the data \
layer, persistence) in one Rust core. The UI stays SwiftUI, Compose and React; `keel` generates the bindings \
they call, builds the core for each platform and serves it to a running app while you edit it.\n\n\
A Keel project is a directory with a keel.toml. Commands find it from the current directory upwards (or use -C).",
    after_long_help = "\
GETTING STARTED
    keel init todo --platforms ios,android,web    a new project: core crate + one app per platform
    cd todo
    keel dev                                      serve the core to running apps, rebuilding on change
    keel build --release                          XCFramework, jniLibs and wasm for the apps to link
    keel bindgen                                  regenerate Swift, Kotlin and TypeScript after a core change

EXISTING APP
    keel adopt ../MyApp                           add a Keel core to an app you already have, step by step

CHECK THE MACHINE
    keel doctor                                   toolchains, SDKs and targets, with the fix for each gap

Errors are printed as `error[keel::C00NN]` with what happened, why, and what to do; every code is
explained at https://keel.dev/errors/C00NN."
)]
pub struct Cli {
    /// Run as if started in this directory (the project is the nearest keel.toml from here up).
    #[arg(short = 'C', long = "project-dir", global = true, value_name = "DIR")]
    pub project_dir: Option<PathBuf>,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new Keel project: a core crate and an app shell per platform.
    #[command(
        long_about = "Creates a project directory with:\n\
  keel.toml        the project file\n\
  core/            the Rust core: a working to-do store with #[keel::store] and #[keel::api]\n\
  generated/       Swift, Kotlin and TypeScript bindings for that core (already generated)\n\
  ios/ android/ web/   one minimal, real app per platform, using the generated bindings\n\n\
Nothing is downloaded: the templates are part of this binary. Point --keel-path at a checkout of the Keel \
repository to use its crates and runtimes directly; without it the project depends on released versions.",
        after_long_help = "\
EXAMPLES
    keel init todo                                   all three platforms
    keel init todo --platforms ios,web               only those shells
    keel init todo --id dev.acme.todo                choose the application id / bundle identifier
    keel init todo --keel-path ~/src/keel            use a local checkout of Keel

NEXT
    cd todo && keel dev        then run an app shell (each README says how)"
    )]
    Init(InitArgs),
    /// Generate the Swift, Kotlin and TypeScript bindings from the core's schema.
    #[command(
        long_about = "Reads the core's schema and writes the bindings the app shells import:\n\
  <out>/swift/     a Swift package (Sources/<Module>/Generated/*.swift + Package.swift)\n\
  <out>/kotlin/    a Gradle module (src/main/kotlin/<package>/*.kt, build.gradle.kts, and a .gitignore for Gradle's output)\n\
  <out>/ts/        an npm package (src/*.ts, package.json, tsconfig.json)\n\n\
By default the schema comes from the core itself: the core is built as a host library with `keel-ffi` \
linked in, loaded, and asked for `keel_schema_json` (docs/SPEC.md 13). With --schema it is read from a file \
instead and nothing is built. Files an earlier run wrote and this one does not are removed; files you added \
next to them are never touched.",
        after_long_help = "\
EXAMPLES
    keel bindgen                          build the core, extract the schema, write <project>/generated
    keel bindgen --out ../shared/bindings
    keel bindgen --schema schema.json     no build: generate from a schema file
    keel bindgen --check                  exit 1 if the generated files are stale (for CI)
    keel bindgen --docs                   include the core's doc comments (see below)

DOC COMMENTS
    The library's own schema export is the canonical form that the schema hash is computed from, which
    has no doc comments. --docs runs the core once more to read the full schema, so the generated code
    carries the same documentation the Rust source has."
    )]
    Bindgen(BindgenArgs),
    /// Build the core for iOS, Android, the web or this machine.
    #[command(
        long_about = "Builds the core for each platform and puts the result where the app shells look for it (below \
`build/`):\n\
  ios       build/ios/KeelCore.xcframework            device + simulator slices, with the C header\n\
  android   build/android/jniLibs/<abi>/libkeel_core.so   arm64-v8a and x86_64, 16 KB page aligned\n\
  web       build/web/keel_core.wasm                  release profile, then wasm-opt -Oz if installed\n\
  host      build/host/libkeel_core.{dylib,so}        for the JVM tests and keel bindgen\n\n\
The library is the core plus the Keel C ABI, built from a small crate generated below `target/keel/` \
(you never write it). Sizes are printed at the end next to the budgets of the design.",
        after_long_help = "\
EXAMPLES
    keel build                                   every platform in keel.toml, debug
    keel build --release                         optimized: what you ship
    keel build --platform web --release
    keel build --platform android,host

iOS DEBUG BUILDS
    Link the library with -force_load (the generated Xcode project already does), or the core's
    registrations are dropped by the linker and the schema is empty. Release builds need no flag."
    )]
    Build(BuildArgs),
    /// Serve the core over a WebSocket to running apps, rebuilding when the code changes.
    #[command(
        long_about = "Runs the core on this machine and serves it over a WebSocket. A simulator, a phone or a \
browser tab connects with the `remote` transport of its Keel runtime and uses this core instead of a built-in \
one: edit Rust, save, and the core is rebuilt and restarted; reload the app to reconnect.\n\n\
Logs, including the development records of docs/SPEC.md 5.10 (a line per transaction commit, port call \
and panic), are printed here. Clocks, randomness and logging are answered by this machine because a remote \
client cannot answer a synchronous port.",
        after_long_help = "\
EXAMPLES
    keel dev                               listen on 127.0.0.1:7443
    keel dev --addr 0.0.0.0:7443           reachable from a phone on your network (no authentication!)
    keel dev --no-watch                    build once and serve

CONNECTING
    web       KeelCore.load({ mode: \"remote\", url: \"ws://127.0.0.1:7443\", expectedSchemaHash })
    iOS       KEEL_DEV_URL=ws://<your Mac>:7443 (the generated app reads it in debug builds)
    Android   the Kotlin runtime has no remote transport on Android yet; use the JVM or the web

The server has no authentication. Keep the default loopback address unless a device has to reach it."
    )]
    Dev(DevArgs),
    /// Check the toolchains and SDKs this machine has against what the project needs.
    #[command(
        long_about = "Checks what the platforms need and prints one line per finding with the fix for each gap: \
Rust and its targets, Xcode (and whether xcode-select points at it), the Android SDK, NDK and cargo-ndk, \
Node, wasm-opt and a JDK. Inside a project only the platforms of keel.toml are checked; elsewhere all of \
them. Exits with status 1 when something the project needs is missing.",
        after_long_help = "\
EXAMPLES
    keel doctor
    keel doctor --platform ios"
    )]
    Doctor(DoctorArgs),
    /// Add a Keel core to an existing app, without touching the app's own project files.
    #[command(
        long_about = "Looks at an existing iOS, Android or web app repository and adds a Keel core to it the \
conservative way: one new `keel/` directory (a core crate, keel.toml and KEEL_ADOPT.md) and a list of the \
exact steps to wire it into the app. The app's own project files are never modified: you make the few \
edits yourself, and the steps are written down with the paths already filled in.",
        after_long_help = "\
EXAMPLES
    keel adopt ../MyApp                          detect platforms, write ../MyApp/keel/
    keel adopt . --platform ios                  only the iOS steps
    keel adopt ../MyApp --keel-path ~/src/keel   use a local checkout of Keel

WHAT IT DETECTS
    iOS      *.xcodeproj / Package.swift        Android  settings.gradle(.kts) + AndroidManifest.xml
    web      package.json (vite, webpack, next, ...)"
    )]
    Adopt(AdoptArgs),
}

/// Arguments of `keel init`.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// The project name: letters, digits, `-` and `_` (for example `todo-app`).
    pub name: String,

    /// Platforms to create app shells for: ios, android, web (comma separated).
    #[arg(
        long,
        visible_alias = "targets",
        value_name = "LIST",
        default_value = "ios,android,web"
    )]
    pub platforms: String,

    /// Application id and iOS bundle identifier; default com.example.<name>.
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,

    /// Use the crates and runtimes of this checkout of the Keel repository instead of released
    /// versions.
    #[arg(long, value_name = "DIR", env = "KEEL_PATH")]
    pub keel_path: Option<PathBuf>,

    /// Create the project inside this directory (default: the current one).
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

/// Arguments of `keel bindgen`.
#[derive(Args, Debug)]
pub struct BindgenArgs {
    /// Generate from this schema JSON file instead of building the core.
    #[arg(long, value_name = "FILE")]
    pub schema: Option<PathBuf>,

    /// Write below this directory (default: `[paths] generated` of keel.toml, else `generated`).
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,

    /// Extract the schema from a release build of the core.
    #[arg(long)]
    pub release: bool,

    /// Only these platforms' bindings (ios = Swift, android = Kotlin, web = TypeScript).
    #[arg(long, value_name = "LIST")]
    pub platforms: Option<String>,

    /// Write nothing; exit with an error if the generated files differ from what would be written.
    #[arg(long)]
    pub check: bool,

    /// Keep the core's doc comments in the bindings (runs the core to read the full schema).
    #[arg(long)]
    pub docs: bool,

    /// Name of the core crate, for a schema that has none (a canonical schema JSON file).
    #[arg(long, value_name = "NAME")]
    pub crate_name: Option<String>,
}

/// Arguments of `keel build`.
#[derive(Args, Debug)]
pub struct BuildArgs {
    /// What to build for: ios, android, web, host (comma separated; default every platform in
    /// keel.toml).
    #[arg(long, visible_alias = "platforms", value_name = "LIST")]
    pub platform: Option<String>,

    /// Optimized build (LTO, one codegen unit) instead of a debug build. The web build is always
    /// optimized.
    #[arg(long)]
    pub release: bool,
}

/// Arguments of `keel dev`.
#[derive(Args, Debug)]
pub struct DevArgs {
    /// Address to listen on (port 0 picks a free one).
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:7443")]
    pub addr: String,

    /// Build once and serve; do not rebuild when the code changes.
    #[arg(long)]
    pub no_watch: bool,

    /// Log records below this level (0 trace .. 5 fatal) are not printed.
    #[arg(long, value_name = "LEVEL", default_value_t = 1)]
    pub log_level: u8,
}

/// Arguments of `keel doctor`.
#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Check only these platforms (ios, android, web; comma separated).
    #[arg(long, value_name = "LIST")]
    pub platform: Option<String>,
}

/// Arguments of `keel adopt`.
#[derive(Args, Debug)]
pub struct AdoptArgs {
    /// The app repository (default: the current directory).
    pub path: Option<PathBuf>,

    /// Only these platforms (ios, android, web; comma separated); default every one detected.
    #[arg(long, value_name = "LIST")]
    pub platform: Option<String>,

    /// Name of the core (default: the repository directory's name).
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Use the crates and runtimes of this checkout of the Keel repository.
    #[arg(long, value_name = "DIR", env = "KEEL_PATH")]
    pub keel_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn init_defaults_and_aliases() {
        let cli = Cli::try_parse_from(["keel", "init", "todo"]).unwrap();
        let Command::Init(args) = cli.command else {
            panic!("not init")
        };
        assert_eq!(args.name, "todo");
        assert_eq!(args.platforms, "ios,android,web");

        let cli = Cli::try_parse_from([
            "keel",
            "init",
            "todo",
            "--targets",
            "web",
            "--keel-path",
            "/k",
        ])
        .unwrap();
        let Command::Init(args) = cli.command else {
            panic!("not init")
        };
        assert_eq!(args.platforms, "web");
        assert_eq!(args.keel_path, Some(PathBuf::from("/k")));
    }

    #[test]
    fn build_accepts_the_spec_flags() {
        let cli =
            Cli::try_parse_from(["keel", "build", "--platform", "ios,web", "--release"]).unwrap();
        let Command::Build(args) = cli.command else {
            panic!("not build")
        };
        assert_eq!(args.platform.as_deref(), Some("ios,web"));
        assert!(args.release);
    }

    #[test]
    fn dev_defaults_to_loopback() {
        let cli = Cli::try_parse_from(["keel", "dev"]).unwrap();
        let Command::Dev(args) = cli.command else {
            panic!("not dev")
        };
        assert_eq!(args.addr, "127.0.0.1:7443");
        assert!(!args.no_watch);
        assert_eq!(args.log_level, 1);
    }

    #[test]
    fn the_project_dir_is_global() {
        let cli = Cli::try_parse_from(["keel", "doctor", "-C", "/p"]).unwrap();
        assert_eq!(cli.project_dir, Some(PathBuf::from("/p")));
    }

    #[test]
    fn unknown_commands_fail() {
        assert!(Cli::try_parse_from(["keel", "frobnicate"]).is_err());
        assert!(Cli::try_parse_from(["keel"]).is_err());
    }

    #[test]
    fn every_command_has_teaching_help() {
        let command = Cli::command();
        for sub in command.get_subcommands() {
            let long = sub
                .get_long_about()
                .map(ToString::to_string)
                .unwrap_or_default();
            assert!(
                long.len() > 120,
                "`keel {}` needs a real --help text",
                sub.get_name()
            );
            assert!(
                sub.get_after_long_help()
                    .is_some_and(|h| h.to_string().contains("keel ")),
                "`keel {}` needs examples",
                sub.get_name()
            );
        }
    }
}
