#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod emit;
pub mod naming;
pub mod provenance;
pub mod stdlib;

mod kotlin;
mod model;
mod swift;
mod ts;
mod validate;
mod zero;

use std::io;
use std::path::Path;

use undra_meta::Schema;

pub use model::{
    QUERY_FETCH_NEXT_PAGE_ID, QUERY_INVALIDATE_ID, QUERY_REFETCH_ID, QUERY_SET_POLL_INTERVAL_ID,
    QUERY_STATUS,
};
pub use validate::{BindgenError, validate};

/// One generated source file: a path relative to the output root and its
/// contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the output root, with `/` separators.
    pub path: String,
    /// The file's text, ending in a newline.
    pub contents: String,
}

impl GeneratedFile {
    /// Writes `files` below `root`, creating directories as needed.
    ///
    /// # Errors
    ///
    /// Returns the first I/O error.
    pub fn write_all(files: &[GeneratedFile], root: &Path) -> io::Result<()> {
        for file in files {
            let path = root.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &file.contents)?;
        }
        Ok(())
    }
}

/// How a generated Swift store is observed by SwiftUI (ADR-045).
///
/// `Observation` is the `@Observable` macro (iOS 17 / macOS 14 and later); `ObservableObject` is Combine's
/// `ObservableObject` with `@Published` properties, which every iOS and macOS version the Swift runtime supports
/// (iOS 15 / macOS 12 and later) has. Everything else in the generated code is the same.
///
/// ```
/// use undra_bindgen::SwiftObservation;
///
/// assert_eq!(SwiftObservation::for_ios(17), SwiftObservation::Observation);
/// assert_eq!(SwiftObservation::for_ios(16), SwiftObservation::ObservableObject);
/// assert_eq!("observable-object".parse(), Ok(SwiftObservation::ObservableObject));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SwiftObservation {
    /// `@MainActor @Observable` stores; needs iOS 17 / macOS 14.
    #[default]
    Observation,
    /// `@MainActor` stores that are `ObservableObject`s with `@Published` properties; works from iOS 15 / macOS 12.
    ObservableObject,
}

impl SwiftObservation {
    /// The first iOS major version that has Observation.
    pub const OBSERVATION_MIN_IOS: u32 = 17;

    /// The first iOS major version the Swift runtime supports (ADR-045).
    pub const RUNTIME_MIN_IOS: u32 = 15;

    /// The mode a deployment target of iOS `major` gets unless it is told otherwise: `Observation` from iOS 17,
    /// `ObservableObject` below.
    #[must_use]
    pub fn for_ios(major: u32) -> SwiftObservation {
        if major >= Self::OBSERVATION_MIN_IOS {
            SwiftObservation::Observation
        } else {
            SwiftObservation::ObservableObject
        }
    }

    /// The spelling used in `undra.toml` (`[bindings] swift_observation`) and on the command line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SwiftObservation::Observation => "observation",
            SwiftObservation::ObservableObject => "observable-object",
        }
    }
}

impl std::fmt::Display for SwiftObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SwiftObservation {
    type Err = String;

    fn from_str(text: &str) -> Result<SwiftObservation, String> {
        match text.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "observation" | "observable" => Ok(SwiftObservation::Observation),
            "observable-object" | "observableobject" => Ok(SwiftObservation::ObservableObject),
            other => Err(format!(
                "`{other}` is not a Swift observation mode (use `observation` or `observable-object`)"
            )),
        }
    }
}

/// The range of `@undra/runtime` a generated `package.json` asks for by default: the release this generator belongs to
/// and the ones compatible with it (ADR-063: `@undra/runtime` is installed from the release's asset, and npm checks a
/// peer range against the installed package's version). `scripts/bump-version.sh` moves every copy of it in the
/// repository.
pub const RUNTIME_RANGE: &str = concat!("^", env!("CARGO_PKG_VERSION"));

/// Configuration of the generators.
///
/// [`Generator::for_crate`] derives every name from the core's crate name;
/// override the public fields to taste.
///
/// ```
/// use undra_bindgen::Generator;
///
/// let mut generator = Generator::for_crate("playground-core");
/// assert_eq!(generator.namespace, "playground_core");
/// assert_eq!(generator.swift_module, "PlaygroundCore");
/// assert_eq!(generator.kotlin_package, "dev.undra.generated.playground_core");
/// assert_eq!(generator.ts_package_name(), "@app/playground-core");
///
/// generator.ts_scope = "acme".into();
/// assert_eq!(generator.ts_package_name(), "@acme/playground-core");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generator {
    /// The core's namespace (`[core] namespace` of `undra.toml`, ADR-044): it names the core's
    /// one C export (`<namespace>_undra_api`), its libraries and the generated entry point of every
    /// language (`Undra<Namespace>`, see [`naming::CoreNames`]), which loads this core and is the
    /// default core of every generated API. Default: the crate name in snake case.
    pub namespace: String,
    /// Name of the SwiftPM module the Swift files belong to; also the
    /// directory below `Sources/`.
    pub swift_module: String,
    /// Swift port requirements use `throws(E)` (`true`) or plain `throws`
    /// (`false`). Calls never use typed throws (ADR-032): a call fails with its
    /// `E`, `CancellationError` or `UndraCallError`, which one `throws` carries.
    ///
    /// A port is implemented by the host, so `throws(E)` there tells the
    /// implementer exactly which errors the core understands.
    pub swift_typed_throws: bool,
    /// How generated Swift stores are observed (ADR-045): `@Observable` (the default, iOS 17 and later) or
    /// `ObservableObject` with `@Published` properties (iOS 15 and later). `undra bindgen` derives it from
    /// `[ios] deployment_target` unless `[bindings] swift_observation` says otherwise.
    pub swift_observation: SwiftObservation,
    /// The major version of the lowest iOS the generated Swift supports (the major of `[ios]
    /// deployment_target`; default 17). The wire `Duration` is `Swift.Duration` from 16 and the runtime's
    /// `UndraDuration` below (`Swift.Duration` is iOS 16 / macOS 13), and the generated package declares it as
    /// its platform floor.
    pub swift_min_ios: u32,
    /// Kotlin package of every generated file; the files are written below
    /// `src/main/kotlin/<package path>/`.
    pub kotlin_package: String,
    /// npm scope of the generated package (`acme` gives `@acme/<package>`).
    /// Empty for an unscoped package.
    pub ts_scope: String,
    /// npm package name without the scope.
    pub ts_package: String,
    /// Map `i64` and `u64` to `number` instead of `bigint`. Values outside
    /// `Number.MAX_SAFE_INTEGER` then fail to decode with an
    /// `unsafe_integer` wire error. This is a global switch until the schema
    /// grows a per-field `#[undra(js_number)]` flag.
    pub ts_js_number: bool,
    /// `version` of the generated `package.json`.
    pub package_version: String,
    /// The range of `@undra/runtime` the generated `package.json` asks for (its peer and dev dependency). Default: this
    /// generator's own release, `^<version>` ([`RUNTIME_RANGE`]). `undra bindgen` sets the release a project pins
    /// (`[undra] version`, ADR-063) so the generated package's bytes do not depend on which `undra` wrote them.
    pub ts_runtime_range: String,
    /// Declare the standard library (the ten standard ports and their eight types) like any
    /// other item. Off by default: every runtime already ships them, so generating them again
    /// would declare a second `FsError` in the app (see [`stdlib`] and ADR-024). Turn it on only
    /// to generate the standard library itself, as the `undra-ports` tests do.
    pub emit_standard_library: bool,
}

impl Generator {
    /// A generator whose names are derived from `crate_name`.
    #[must_use]
    pub fn for_crate(crate_name: &str) -> Generator {
        let snake = crate_name.replace(['-', '.'], "_").to_ascii_lowercase();
        Generator {
            namespace: naming::CoreNames::default_namespace(crate_name),
            swift_module: naming::pascal(crate_name),
            swift_typed_throws: true,
            swift_observation: SwiftObservation::Observation,
            swift_min_ios: SwiftObservation::OBSERVATION_MIN_IOS,
            kotlin_package: format!("dev.undra.generated.{snake}"),
            ts_scope: "app".to_owned(),
            ts_package: crate_name.replace('_', "-").to_ascii_lowercase(),
            ts_js_number: false,
            package_version: "0.1.0".to_owned(),
            ts_runtime_range: RUNTIME_RANGE.to_owned(),
            emit_standard_library: false,
        }
    }

    /// The names derived from [`namespace`](Self::namespace) (entry point, artefacts, C symbol).
    #[must_use]
    pub fn core_names(&self) -> naming::CoreNames {
        naming::CoreNames::new(&self.namespace)
    }

    /// The npm name of the generated package, `@scope/name` or `name`.
    #[must_use]
    pub fn ts_package_name(&self) -> String {
        if self.ts_scope.is_empty() {
            self.ts_package.clone()
        } else {
            format!("@{}/{}", self.ts_scope, self.ts_package)
        }
    }

    /// Validates `schema` (see [`validate`]), for this configuration.
    ///
    /// # Errors
    ///
    /// Returns every problem found.
    pub fn validate(&self, schema: &Schema) -> Result<(), Vec<BindgenError>> {
        validate::validate_for(schema, self.emit_standard_library)
    }

    /// Generates the Swift package sources:
    /// `Sources/<Module>/Generated/{Types,Errors,Objects,Stores,Ports,Queries,Ids}.swift`.
    ///
    /// # Errors
    ///
    /// Returns the validation errors when the schema cannot be generated.
    pub fn swift(&self, schema: &Schema) -> Result<Vec<GeneratedFile>, Vec<BindgenError>> {
        let mut problems = self.validate(schema).err().unwrap_or_default();
        if problems.is_empty() {
            problems = self.swift_floor_errors(schema);
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(swift::generate(
            &model::Model::new(schema, model::Lang::Swift, self.emit_standard_library),
            self,
        ))
    }

    /// What this configuration's Swift floor cannot express about `schema` (ADR-045): the iOS 15 / 16 store shape is
    /// an `ObservableObject`, which has an `objectWillChange` publisher of its own, so a store member of that name
    /// would collide. Empty for the default `@Observable` stores and for a schema that is fine in either mode. The
    /// errors are E0051, like every name the generated code already uses.
    #[must_use]
    pub fn swift_floor_errors(&self, schema: &Schema) -> Vec<BindgenError> {
        validate::swift_floor(schema, self.swift_observation)
    }

    /// Generates the Kotlin sources:
    /// `src/main/kotlin/<package path>/{Types,Errors,Objects,Stores,Ports,Queries,Ids}.kt`.
    ///
    /// # Errors
    ///
    /// Returns the validation errors when the schema cannot be generated.
    pub fn kotlin(&self, schema: &Schema) -> Result<Vec<GeneratedFile>, Vec<BindgenError>> {
        self.validate(schema)?;
        Ok(kotlin::generate(
            &model::Model::new(schema, model::Lang::Kotlin, self.emit_standard_library),
            self,
        ))
    }

    /// Generates the TypeScript package: `src/{types,errors,objects,stores,ports,queries,ids,index}.ts`,
    /// `package.json` and `tsconfig.json`.
    ///
    /// # Errors
    ///
    /// Returns the validation errors when the schema cannot be generated.
    pub fn typescript(&self, schema: &Schema) -> Result<Vec<GeneratedFile>, Vec<BindgenError>> {
        self.validate(schema)?;
        Ok(ts::generate(
            &model::Model::new(schema, model::Lang::TypeScript, self.emit_standard_library),
            self,
        ))
    }
}
