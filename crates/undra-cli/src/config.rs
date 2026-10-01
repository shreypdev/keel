//! `undra.toml`: the file that makes a directory an Undra project.
//!
//! ```toml
//! [project]
//! name = "todo"
//! id = "com.example.todo"
//! platforms = ["ios", "android", "web"]
//!
//! [core]
//! path = "core"
//! namespace = "todo"   # optional: names the core's symbol and libraries (ADR-044)
//! ```
//!
//! Every key but `project.name` and `project.id` has a default, and an unknown key is an error
//! (a typo must not silently fall back to a default). [`ProjectConfig::render`] writes the file
//! `undra init` creates, with comments that say what each section is for.

use std::path::Path;

use crate::error::{CliError, Result};
use crate::toml_lite::{self, Document, Entry, Value, quote};

/// A platform an app is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Platform {
    /// iOS: an XCFramework and a SwiftUI app.
    Ios,
    /// Android: `jniLibs/` and a Compose app.
    Android,
    /// The web: a wasm module and a React app.
    Web,
}

impl Platform {
    /// Every platform, in the order they are printed.
    pub const ALL: [Platform; 3] = [Platform::Ios, Platform::Android, Platform::Web];

    /// The name used in `undra.toml` and on the command line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Web => "web",
        }
    }

    /// Parses a platform name.
    ///
    /// # Errors
    ///
    /// Lists the valid names.
    pub fn parse(text: &str) -> Result<Platform> {
        match text.trim().to_ascii_lowercase().as_str() {
            "ios" => Ok(Platform::Ios),
            "android" => Ok(Platform::Android),
            "web" | "wasm" => Ok(Platform::Web),
            other => Err(CliError::bad_argument(
                format!("`{other}` is not a platform Undra builds for"),
                "a platform decides which toolchain builds the core and which app shell is created",
                "use one of: ios, android, web",
            )),
        }
    }

    /// Parses a comma-separated list such as `ios,android`.
    ///
    /// # Errors
    ///
    /// The first invalid name, or an empty list.
    pub fn parse_list(text: &str) -> Result<Vec<Platform>> {
        let mut out = Vec::new();
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let platform = Platform::parse(part)?;
            if !out.contains(&platform) {
                out.push(platform);
            }
        }
        if out.is_empty() {
            return Err(CliError::bad_argument(
                "the platform list is empty",
                "an app without a platform has nothing to build",
                "list at least one of: ios, android, web",
            ));
        }
        out.sort();
        Ok(out)
    }
}

/// Names of the generated bindings, overriding what `undra-bindgen` derives from the crate name.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BindingsConfig {
    /// The Swift module (`Sources/<module>`); default `<Pascal>Core`.
    pub swift_module: Option<String>,
    /// The Kotlin package; default `<project id>.core`.
    pub kotlin_package: Option<String>,
    /// The npm scope, without `@`; default `app`.
    pub ts_scope: Option<String>,
    /// The npm package name without the scope; default the core's package name.
    pub ts_package: Option<String>,
    /// Map `i64`/`u64` to `number` instead of `bigint` in TypeScript.
    pub ts_js_number: bool,
    /// Emit `throws(E)` on Swift port requirements (calls always use plain
    /// `throws`, ADR-032); `true` unless turned off.
    pub swift_typed_throws: Option<bool>,
}

/// iOS build settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IosConfig {
    /// The minimum iOS version (`IPHONEOS_DEPLOYMENT_TARGET`).
    pub deployment_target: String,
    /// Simulator architectures: `arm64` and/or `x86_64`.
    pub simulator_archs: Vec<String>,
}

impl Default for IosConfig {
    fn default() -> Self {
        IosConfig {
            deployment_target: "17.0".to_owned(),
            simulator_archs: vec!["arm64".to_owned()],
        }
    }
}

/// Android build settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidConfig {
    /// The ABIs to build: `arm64-v8a`, `x86_64`, `armeabi-v7a`, `x86`.
    pub abis: Vec<String>,
    /// The minimum API level (`cargo ndk --platform`).
    pub min_sdk: u32,
}

impl Default for AndroidConfig {
    fn default() -> Self {
        AndroidConfig {
            abis: vec!["arm64-v8a".to_owned(), "x86_64".to_owned()],
            min_sdk: 26,
        }
    }
}

/// Web build settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebConfig {
    /// `z` (smallest) or `s` (small): the `opt-level` of the wasm build (SPEC 7: "measured").
    pub opt_level: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        WebConfig {
            opt_level: "z".to_owned(),
        }
    }
}

/// Where the platform runtimes are found, when not derived from `[undra] path`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RuntimesConfig {
    /// The directory of the `UndraRuntime` Swift package.
    pub swift: Option<String>,
    /// The directory of the Kotlin runtime Gradle build (`runtimes/kotlin/undra-runtime`).
    pub kotlin: Option<String>,
    /// The directory of the `@undra/runtime` npm package.
    pub ts: Option<String>,
}

/// The contents of `undra.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConfig {
    /// The project name (`todo-app`).
    pub name: String,
    /// The application id / bundle identifier (`com.example.todo`).
    pub id: String,
    /// The platforms the app targets.
    pub platforms: Vec<Platform>,
    /// The core crate's directory, relative to the project.
    pub core_path: String,
    /// The core's Cargo package name, when it cannot be found by looking at the directory.
    pub core_package: Option<String>,
    /// The core's namespace (ADR-044): it names the core's one C export (`<namespace>_undra_api`),
    /// its libraries (`lib<namespace>.so`, `<namespace>.wasm`, ...) and the generated entry point
    /// (`Undra<Namespace>`). `None`: the core's package name in snake case.
    pub core_namespace: Option<String>,
    /// Where `undra bindgen` writes, relative to the project.
    pub generated: String,
    /// Where `undra build` writes artifacts, relative to the project.
    pub build: String,
    /// A checkout of the Undra repository the crates and runtimes come from (relative to the
    /// project); absent when they come from the registries.
    pub undra_path: Option<String>,
    /// The Undra version the registries are asked for.
    pub undra_version: String,
    /// Binding names.
    pub bindings: BindingsConfig,
    /// iOS settings.
    pub ios: IosConfig,
    /// Android settings.
    pub android: AndroidConfig,
    /// Web settings.
    pub web: WebConfig,
    /// Runtime locations.
    pub runtimes: RuntimesConfig,
}

/// The Undra release line (`<major>.<minor>`) this CLI belongs to: what `undra init` asks the
/// package registries for (`@undra/runtime`, `dev.undra:runtime`, the Swift package). It follows
/// the workspace version, so a release never leaves a scaffold asking for the previous line.
pub const UNDRA_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);

/// Where the Undra crates are fetched from (until they are on crates.io): this repository.
pub const UNDRA_REPO_URL: &str = "https://github.com/shreypdev/undra";

/// The git tag of this CLI's release, `v<version>`: what `undra init` pins the core's `undra`
/// dependency to, so the project uses the crates the CLI was released with.
pub const UNDRA_RELEASE_TAG: &str = concat!("v", env!("CARGO_PKG_VERSION"));

impl ProjectConfig {
    /// A configuration with every default, for a project called `name`.
    #[must_use]
    pub fn new(name: &str, id: &str, platforms: Vec<Platform>) -> ProjectConfig {
        ProjectConfig {
            name: name.to_owned(),
            id: id.to_owned(),
            platforms,
            core_path: "core".to_owned(),
            core_package: None,
            core_namespace: None,
            generated: "generated".to_owned(),
            build: "build".to_owned(),
            undra_path: None,
            undra_version: UNDRA_VERSION.to_owned(),
            bindings: BindingsConfig::default(),
            ios: IosConfig::default(),
            android: AndroidConfig::default(),
            web: WebConfig::default(),
            runtimes: RuntimesConfig::default(),
        }
    }

    /// Parses `text`, read from `file` (used in messages).
    ///
    /// # Errors
    ///
    /// A syntax error, an unknown table or key, a value of the wrong type, or a missing
    /// required key.
    pub fn parse(text: &str, file: &Path) -> Result<ProjectConfig> {
        let doc = toml_lite::parse(text).map_err(|e| {
            CliError::bad_config(file, e.to_string(), "fix the line named above; undra.toml is TOML with tables, strings, booleans, integers and arrays")
        })?;
        let reader = Reader { doc: &doc, file };
        reader.check_tables(&[
            "project", "core", "paths", "undra", "bindings", "ios", "android", "web", "runtimes",
        ])?;
        if let Some(root) = doc.tables.get("").filter(|t| !t.is_empty()) {
            let (key, entry) = root.iter().next().expect("not empty");
            return Err(CliError::bad_config(
                file,
                format!("line {}: `{key}` is outside any table", entry.line),
                "put it under the right [table], for example `name` belongs under [project]",
            ));
        }

        reader.check_keys("project", &["name", "id", "platforms"])?;
        let name = reader.require_str("project", "name")?;
        let id = reader.require_str("project", "id")?;
        let platforms = match reader.get("project", "platforms") {
            Some(entry) => {
                let list = reader.as_str_list(entry, "project", "platforms")?;
                Platform::parse_list(&list.join(",")).map_err(|e| {
                    CliError::bad_config(file, format!("line {}: {}", entry.line, e.what), e.fix)
                })?
            }
            None => Platform::ALL.to_vec(),
        };

        let mut cfg = ProjectConfig::new(&name, &id, platforms);

        reader.check_keys("core", &["path", "package", "namespace"])?;
        if let Some(v) = reader.opt_str("core", "path")? {
            cfg.core_path = v;
        }
        cfg.core_package = reader.opt_str("core", "package")?;
        if let Some(entry) = reader.get("core", "namespace") {
            let Value::Str(namespace) = &entry.value else {
                return Err(CliError::bad_config(
                    file,
                    format!("line {}: namespace must be a string", entry.line),
                    "write it in quotes: namespace = \"acme_pay\"",
                ));
            };
            if let Err(why) = check_namespace(namespace) {
                return Err(CliError::bad_config(
                    file,
                    format!(
                        "line {}: `{namespace}` is not a core namespace: {why}",
                        entry.line
                    ),
                    "use lowercase letters, digits and `_`, starting with a letter, at most 32 characters (for example \"acme_pay\")",
                ));
            }
            cfg.core_namespace = Some(namespace.clone());
        }

        reader.check_keys("paths", &["generated", "build"])?;
        if let Some(v) = reader.opt_str("paths", "generated")? {
            cfg.generated = v;
        }
        if let Some(v) = reader.opt_str("paths", "build")? {
            cfg.build = v;
        }

        reader.check_keys("undra", &["path", "version"])?;
        cfg.undra_path = reader.opt_str("undra", "path")?;
        if let Some(v) = reader.opt_str("undra", "version")? {
            cfg.undra_version = v;
        }

        reader.check_keys(
            "bindings",
            &[
                "swift_module",
                "kotlin_package",
                "ts_scope",
                "ts_package",
                "ts_js_number",
                "swift_typed_throws",
            ],
        )?;
        cfg.bindings = BindingsConfig {
            swift_module: reader.opt_str("bindings", "swift_module")?,
            kotlin_package: reader.opt_str("bindings", "kotlin_package")?,
            ts_scope: reader.opt_str("bindings", "ts_scope")?,
            ts_package: reader.opt_str("bindings", "ts_package")?,
            ts_js_number: reader
                .opt_bool("bindings", "ts_js_number")?
                .unwrap_or(false),
            swift_typed_throws: reader.opt_bool("bindings", "swift_typed_throws")?,
        };

        reader.check_keys("ios", &["deployment_target", "simulator_archs"])?;
        if let Some(v) = reader.opt_str("ios", "deployment_target")? {
            cfg.ios.deployment_target = v;
        }
        if let Some(entry) = reader.get("ios", "simulator_archs") {
            let archs = reader.as_str_list(entry, "ios", "simulator_archs")?;
            for arch in &archs {
                if arch != "arm64" && arch != "x86_64" {
                    return Err(CliError::bad_config(
                        file,
                        format!(
                            "line {}: `{arch}` is not an iOS simulator architecture",
                            entry.line
                        ),
                        "use \"arm64\" and/or \"x86_64\"",
                    ));
                }
            }
            if archs.is_empty() {
                return Err(CliError::bad_config(
                    file,
                    format!("line {}: simulator_archs is empty", entry.line),
                    "list at least one of \"arm64\", \"x86_64\"",
                ));
            }
            cfg.ios.simulator_archs = archs;
        }

        reader.check_keys("android", &["abis", "min_sdk"])?;
        if let Some(entry) = reader.get("android", "abis") {
            let abis = reader.as_str_list(entry, "android", "abis")?;
            for abi in &abis {
                if !["arm64-v8a", "x86_64", "armeabi-v7a", "x86"].contains(&abi.as_str()) {
                    return Err(CliError::bad_config(
                        file,
                        format!("line {}: `{abi}` is not an Android ABI", entry.line),
                        "use \"arm64-v8a\", \"x86_64\", \"armeabi-v7a\" or \"x86\"",
                    ));
                }
            }
            if abis.is_empty() {
                return Err(CliError::bad_config(
                    file,
                    format!("line {}: abis is empty", entry.line),
                    "list at least one ABI, for example \"arm64-v8a\"",
                ));
            }
            cfg.android.abis = abis;
        }
        if let Some(entry) = reader.get("android", "min_sdk") {
            match entry.value {
                Value::Int(n) if (21..=99).contains(&n) => {
                    cfg.android.min_sdk = u32::try_from(n).expect("in range")
                }
                _ => {
                    return Err(CliError::bad_config(
                        file,
                        format!(
                            "line {}: min_sdk must be an integer API level (21 or more)",
                            entry.line
                        ),
                        "Undra targets API 26 and up (SPEC 0); use 26 unless you know your users need less",
                    ));
                }
            }
        }

        reader.check_keys("web", &["opt_level"])?;
        if let Some(entry) = reader.get("web", "opt_level") {
            match &entry.value {
                Value::Str(s) if s == "z" || s == "s" => cfg.web.opt_level.clone_from(s),
                _ => {
                    return Err(CliError::bad_config(
                        file,
                        format!("line {}: opt_level must be \"z\" or \"s\"", entry.line),
                        "SPEC 7 allows opt-level z (smallest) or s (small); measure both with `undra build --platform web`",
                    ));
                }
            }
        }

        reader.check_keys("runtimes", &["swift", "kotlin", "ts"])?;
        cfg.runtimes = RuntimesConfig {
            swift: reader.opt_str("runtimes", "swift")?,
            kotlin: reader.opt_str("runtimes", "kotlin")?,
            ts: reader.opt_str("runtimes", "ts")?,
        };
        Ok(cfg)
    }

    /// The text `undra init` writes: every setting that matters, with comments.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let platforms = self
            .platforms
            .iter()
            .map(|p| quote(p.name()))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "# undra.toml: this file makes the directory an Undra project (`undra --help` for the commands).\n\
             \n\
             [project]\n\
             name = {}\n\
             # The Android applicationId, the iOS bundle identifier and the base of the Kotlin package.\n\
             id = {}\n\
             platforms = [{platforms}]\n\
             \n\
             [core]\n\
             # The crate `undra bindgen`, `undra build` and `undra dev` work on.\n\
             path = {}",
            quote(&self.name),
            quote(&self.id),
            quote(&self.core_path),
        );
        if let Some(package) = &self.core_package {
            let _ = writeln!(out, "package = {}", quote(package));
        }
        match &self.core_namespace {
            Some(namespace) => {
                let _ = writeln!(
                    out,
                    "# Names the core's symbol, libraries and generated entry point; unique per app.\n\
                     namespace = {}",
                    quote(namespace)
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "# namespace = \"todo\"   # names the core's symbol, libraries and entry point; default: the crate name"
                );
            }
        }
        let _ = writeln!(
            out,
            "\n[paths]\n\
             # Written by `undra bindgen` (Swift package, Gradle module, npm package). Commit it or\n\
             # regenerate it in CI with `undra bindgen --check`.\n\
             generated = {}\n\
             # Written by `undra build` (XCFramework, jniLibs, wasm). Do not commit.\n\
             build = {}",
            quote(&self.generated),
            quote(&self.build),
        );
        let _ = writeln!(
            out,
            "\n[undra]\n\
             # Where Undra comes from. `path` is a checkout of the Undra repository (crates and\n\
             # runtimes are used from there); without it, the release `version` names is used: the\n\
             # crates by git tag (see core/Cargo.toml), the runtimes from their package registries.\n\
             version = {}",
            quote(&self.undra_version)
        );
        if let Some(path) = &self.undra_path {
            let _ = writeln!(out, "path = {}", quote(path));
        }
        let _ = writeln!(
            out,
            "\n[bindings]\n\
             # Names of the generated code. Defaults are derived from the core crate's name.\n\
             # swift_module = \"TodoCore\"\n\
             # kotlin_package = \"com.example.todo.core\"\n\
             # ts_scope = \"app\"\n\
             # ts_js_number = false        # i64/u64 as `number` instead of `bigint`\n\
             # swift_typed_throws = true   # port requirements: `throws(E)`; false emits plain `throws`"
        );
        if let Some(v) = &self.bindings.swift_module {
            let _ = writeln!(out, "swift_module = {}", quote(v));
        }
        if let Some(v) = &self.bindings.kotlin_package {
            let _ = writeln!(out, "kotlin_package = {}", quote(v));
        }
        if let Some(v) = &self.bindings.ts_scope {
            let _ = writeln!(out, "ts_scope = {}", quote(v));
        }
        if let Some(v) = &self.bindings.ts_package {
            let _ = writeln!(out, "ts_package = {}", quote(v));
        }
        if self.bindings.ts_js_number {
            let _ = writeln!(out, "ts_js_number = true");
        }
        if let Some(v) = self.bindings.swift_typed_throws {
            let _ = writeln!(out, "swift_typed_throws = {v}");
        }
        let archs = self
            .ios
            .simulator_archs
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(", ");
        let abis = self
            .android
            .abis
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "\n[ios]\n\
             deployment_target = {}\n\
             # Simulator slices of the XCFramework. Add \"x86_64\" for Intel Macs.\n\
             simulator_archs = [{archs}]\n\
             \n\
             [android]\n\
             abis = [{abis}]\n\
             min_sdk = {}\n\
             \n\
             [web]\n\
             # \"z\" is smallest, \"s\" is small and a little faster: measure both.\n\
             opt_level = {}",
            quote(&self.ios.deployment_target),
            self.android.min_sdk,
            quote(&self.web.opt_level),
        );
        let runtimes = [
            ("swift", &self.runtimes.swift),
            ("kotlin", &self.runtimes.kotlin),
            ("ts", &self.runtimes.ts),
        ];
        if runtimes.iter().any(|(_, v)| v.is_some()) {
            let _ = writeln!(
                out,
                "\n[runtimes]\n# Overrides for where each platform runtime lives (relative to this file)."
            );
            for (key, value) in runtimes {
                if let Some(v) = value {
                    let _ = writeln!(out, "{key} = {}", quote(v));
                }
            }
        }
        out
    }
}

/// The longest core namespace (ADR-044): it is part of a C symbol and of file names.
pub const MAX_NAMESPACE_LEN: usize = 32;

/// Checks a core namespace (`[core] namespace`, ADR-044): a lowercase C identifier of 1 to 32
/// bytes that starts with a letter. Lowercase only, so that `lib<namespace>.so` and
/// `<namespace>.wasm` never differ by case alone on a case-insensitive file system.
///
/// # Errors
///
/// What is wrong with it, in words.
pub fn check_namespace(namespace: &str) -> std::result::Result<(), String> {
    if namespace.is_empty() {
        return Err("it is empty".to_owned());
    }
    if namespace.len() > MAX_NAMESPACE_LEN {
        return Err(format!(
            "it is {} characters long, and a namespace has at most {MAX_NAMESPACE_LEN}",
            namespace.len()
        ));
    }
    if !namespace.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err("it must start with a lowercase letter".to_owned());
    }
    if let Some(c) = namespace
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_'))
    {
        return Err(format!(
            "`{c}` is not allowed (lowercase letters, digits and `_` only)"
        ));
    }
    Ok(())
}

struct Reader<'a> {
    doc: &'a Document,
    file: &'a Path,
}

impl Reader<'_> {
    fn get(&self, table: &str, key: &str) -> Option<&Entry> {
        self.doc.tables.get(table).and_then(|t| t.get(key))
    }

    fn check_tables(&self, known: &[&str]) -> Result<()> {
        for name in self.doc.tables.keys() {
            if !name.is_empty() && !known.contains(&name.as_str()) {
                return Err(CliError::bad_config(
                    self.file,
                    format!("unknown table [{name}]"),
                    format!(
                        "undra.toml has these tables: {}",
                        known
                            .iter()
                            .map(|k| format!("[{k}]"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
        Ok(())
    }

    fn check_keys(&self, table: &str, known: &[&str]) -> Result<()> {
        if let Some(t) = self.doc.tables.get(table) {
            for (key, entry) in t {
                if !known.contains(&key.as_str()) {
                    return Err(CliError::bad_config(
                        self.file,
                        format!("line {}: unknown key `{key}` in [{table}]", entry.line),
                        format!("[{table}] has these keys: {}", known.join(", ")),
                    ));
                }
            }
        }
        Ok(())
    }

    fn wrong_type(&self, entry: &Entry, table: &str, key: &str, expected: &str) -> CliError {
        /// A value of the kind that was expected, as TOML.
        fn example_value(expected: &str) -> &'static str {
            match expected {
                "a string" => "\"text\"",
                "true or false" => "true",
                "an array of strings" => "[\"a\", \"b\"]",
                _ => "\"value\"",
            }
        }
        CliError::bad_config(
            self.file,
            format!(
                "line {}: `{key}` in [{table}] is {} but must be {expected}",
                entry.line,
                entry.value.describe()
            ),
            format!(
                "write it as {expected}, for example `{key} = {}`",
                example_value(expected)
            ),
        )
    }

    fn opt_str(&self, table: &str, key: &str) -> Result<Option<String>> {
        match self.get(table, key) {
            None => Ok(None),
            Some(Entry {
                value: Value::Str(s),
                ..
            }) => Ok(Some(s.clone())),
            Some(entry) => Err(self.wrong_type(entry, table, key, "a string")),
        }
    }

    fn require_str(&self, table: &str, key: &str) -> Result<String> {
        self.opt_str(table, key)?.ok_or_else(|| {
            CliError::bad_config(
                self.file,
                format!("[{table}] has no `{key}`"),
                format!("add `{key} = \"...\"` under [{table}]"),
            )
        })
    }

    fn opt_bool(&self, table: &str, key: &str) -> Result<Option<bool>> {
        match self.get(table, key) {
            None => Ok(None),
            Some(Entry {
                value: Value::Bool(b),
                ..
            }) => Ok(Some(*b)),
            Some(entry) => Err(self.wrong_type(entry, table, key, "true or false")),
        }
    }

    fn as_str_list(&self, entry: &Entry, table: &str, key: &str) -> Result<Vec<String>> {
        let Value::Array(items) = &entry.value else {
            return Err(self.wrong_type(entry, table, key, "an array of strings"));
        };
        items
            .iter()
            .map(|item| match item {
                Value::Str(s) => Ok(s.clone()),
                _ => Err(self.wrong_type(entry, table, key, "an array of strings")),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    fn file() -> &'static Path {
        Path::new("undra.toml")
    }

    fn sample() -> ProjectConfig {
        let mut cfg = ProjectConfig::new(
            "todo-app",
            "com.example.todoapp",
            vec![Platform::Ios, Platform::Web],
        );
        cfg.undra_path = Some("../../undra".into());
        cfg.bindings.kotlin_package = Some("com.example.todoapp.core".into());
        cfg.bindings.swift_typed_throws = Some(false);
        cfg.ios.simulator_archs = vec!["arm64".into(), "x86_64".into()];
        cfg.runtimes.ts = Some("../rt".into());
        cfg
    }

    #[test]
    fn render_and_parse_round_trip() {
        let cfg = sample();
        let text = cfg.render();
        assert_eq!(ProjectConfig::parse(&text, file()).unwrap(), cfg, "{text}");
    }

    #[test]
    fn minimal_file_gets_defaults() {
        let cfg = ProjectConfig::parse("[project]\nname = \"a\"\nid = \"com.example.a\"\n", file())
            .unwrap();
        assert_eq!(cfg.platforms, Platform::ALL.to_vec());
        assert_eq!(cfg.core_path, "core");
        assert_eq!(cfg.generated, "generated");
        assert_eq!(cfg.android.abis, ["arm64-v8a", "x86_64"]);
        assert_eq!(cfg.web.opt_level, "z");
    }

    #[test]
    fn mistakes_are_explained() {
        let cases = [
            ("[project]\nname = \"a\"\n", "no `id`"),
            (
                "[project]\nname = \"a\"\nid = \"b\"\nplatfroms = []\n",
                "unknown key `platfroms`",
            ),
            (
                "[project]\nname = \"a\"\nid = \"b\"\n[tooling]\n",
                "unknown table [tooling]",
            ),
            ("name = \"a\"\n", "outside any table"),
            (
                "[project]\nname = 3\nid = \"b\"\n",
                "is an integer but must be a string",
            ),
            (
                "[project]\nname = \"a\"\nid = \"b\"\nplatforms = [\"ios\", \"tv\"]\n",
                "`tv` is not a platform",
            ),
            (
                "[project]\nname = \"a\"\nid = \"b\"\n[android]\nabis = [\"mips\"]\n",
                "not an Android ABI",
            ),
            (
                "[project]\nname = \"a\"\nid = \"b\"\n[android]\nmin_sdk = 3\n",
                "min_sdk",
            ),
            (
                "[project]\nname = \"a\"\nid = \"b\"\n[web]\nopt_level = \"3\"\n",
                "opt_level",
            ),
            (
                "[project]\nname = \"a\"\nid = \"b\"\n[ios]\nsimulator_archs = [\"ppc\"]\n",
                "not an iOS simulator",
            ),
            ("[project\n", "must end with `]`"),
        ];
        for (text, needle) in cases {
            let e = ProjectConfig::parse(text, file()).unwrap_err();
            assert_eq!(e.code, Code::BadConfig, "{text:?}");
            assert!(
                e.what.contains(needle) || e.fix.contains(needle),
                "{text:?}: {e}"
            );
        }
    }

    #[test]
    fn platform_lists() {
        assert_eq!(
            Platform::parse_list("web, ios,ios").unwrap(),
            vec![Platform::Ios, Platform::Web]
        );
        assert_eq!(Platform::parse("WASM").unwrap(), Platform::Web);
        assert!(Platform::parse_list("").is_err());
        assert!(Platform::parse_list("ios,tv").is_err());
    }
}
