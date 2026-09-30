#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod emit;
pub mod naming;
pub mod stdlib;

mod kotlin;
mod model;
mod swift;
mod ts;
mod validate;

use std::io;
use std::path::Path;

use keel_meta::Schema;

pub use model::{QUERY_INVALIDATE_ID, QUERY_REFETCH_ID, QUERY_STATUS};
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

/// Configuration of the generators.
///
/// [`Generator::for_crate`] derives every name from the core's crate name;
/// override the public fields to taste.
///
/// ```
/// use keel_bindgen::Generator;
///
/// let mut generator = Generator::for_crate("playground-core");
/// assert_eq!(generator.swift_module, "PlaygroundCore");
/// assert_eq!(generator.kotlin_package, "dev.keel.generated.playground_core");
/// assert_eq!(generator.ts_package_name(), "@app/playground-core");
///
/// generator.ts_scope = "acme".into();
/// assert_eq!(generator.ts_package_name(), "@acme/playground-core");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generator {
    /// Name of the SwiftPM module the Swift files belong to; also the
    /// directory below `Sources/`.
    pub swift_module: String,
    /// Emit `throws(E)` for methods that return `Result<T, E>` (Swift 6).
    /// When `false` they emit plain `throws`.
    ///
    /// Typed throws cannot express task cancellation or a broken core
    /// contract: in typed mode a cancelled `Task` awaiting such a method, or a
    /// core panic, stops the process with a diagnostic instead of throwing.
    /// Turn it off for code that relies on structured cancellation.
    pub swift_typed_throws: bool,
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
    /// grows a per-field `#[keel(js_number)]` flag.
    pub ts_js_number: bool,
    /// `version` of the generated `package.json`.
    pub package_version: String,
    /// Declare the standard library (the ten standard ports and their eight types) like any
    /// other item. Off by default: every runtime already ships them, so generating them again
    /// would declare a second `FsError` in the app (see [`stdlib`] and ADR-024). Turn it on only
    /// to generate the standard library itself, as the `keel-ports` tests do.
    pub emit_standard_library: bool,
}

impl Generator {
    /// A generator whose names are derived from `crate_name`.
    #[must_use]
    pub fn for_crate(crate_name: &str) -> Generator {
        let snake = crate_name.replace(['-', '.'], "_").to_ascii_lowercase();
        Generator {
            swift_module: naming::pascal(crate_name),
            swift_typed_throws: true,
            kotlin_package: format!("dev.keel.generated.{snake}"),
            ts_scope: "app".to_owned(),
            ts_package: crate_name.replace('_', "-").to_ascii_lowercase(),
            ts_js_number: false,
            package_version: "0.1.0".to_owned(),
            emit_standard_library: false,
        }
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
        self.validate(schema)?;
        Ok(swift::generate(
            &model::Model::new(schema, model::Lang::Swift, self.emit_standard_library),
            self,
        ))
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
