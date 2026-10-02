//! `build/symbols/manifest.json`: which symbol file belongs to which shipped artefact.
//!
//! ```json
//! {
//!   "version": 1,
//!   "undra": "0.1.0",
//!   "artifacts": [
//!     {
//!       "platform": "android",
//!       "namespace": "acme_pay",
//!       "coreVersion": "1.4.0",
//!       "schemaHash": "0x53241303b2d08c5e",
//!       "arch": "arm64-v8a",
//!       "format": "elf",
//!       "imageId": "3f2a9c1e0b7d4a55c6e8d9f001122334455667788",
//!       "sha256": "9c1f…",
//!       "shipped": "android/jniLibs/arm64-v8a/libacme_pay.so",
//!       "symbols": "android/arm64-v8a/libacme_pay.so"
//!     }   // a web entry adds "functionMap" and "dwarf" (see `builds::web`)
//!   ]
//! }
//! ```
//!
//! One entry per shipped image: an Android ABI, an iOS slice, the web module, a host library.
//! Paths use `/`; `symbols` is relative to the manifest's own directory (`build/symbols`, so the
//! directory can be uploaded as one artifact), `shipped` to `build/`. `imageId` is what a
//! `PanicReport` of that image carries (ADR-046): the ELF build id, the Mach-O `LC_UUID`, the
//! SHA-256 of the wasm module (lowercase hex). It is `null` for an iOS slice: a static library
//! has no `LC_UUID`, the image that holds the core is the *app*, and its UUID exists only once the
//! app is linked (it is the UUID of the app's dSYM).
//!
//! Entries are keyed by `(platform, namespace, arch)`: a build of one platform replaces its own
//! entries and leaves the others, so `undra build` for each platform in turn (or in separate CI
//! jobs, merged by uploading to the same directory) accumulates one manifest.

use std::path::Path;

use serde_json::{Value, json};

use crate::error::Result;
use crate::fsutil::write_if_changed;

/// The manifest format this CLI writes and reads.
pub const FORMAT_VERSION: u64 = 1;

/// The file's name below `build/symbols/`.
pub const FILE_NAME: &str = "manifest.json";

/// What kind of image an entry describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A Mach-O image (iOS, macOS).
    Macho,
    /// An ELF image (Android).
    Elf,
    /// A wasm module (web).
    Wasm,
}

impl Format {
    /// The word in the manifest.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Format::Macho => "macho",
            Format::Elf => "elf",
            Format::Wasm => "wasm",
        }
    }

    /// Parses the word of a manifest.
    #[must_use]
    pub fn parse(word: &str) -> Option<Format> {
        match word {
            "macho" => Some(Format::Macho),
            "elf" => Some(Format::Elf),
            "wasm" => Some(Format::Wasm),
            _ => None,
        }
    }
}

/// One shipped image and the files that symbolicate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `ios`, `android`, `web` or `host`.
    pub platform: String,
    /// The core's namespace (ADR-044).
    pub namespace: String,
    /// The version of the core crate, as `export_core!` reports it.
    pub core_version: String,
    /// The schema hash of the core, when it could be read.
    pub schema_hash: Option<u64>,
    /// The Android ABI, the iOS slice (`ios-arm64`, `ios-arm64-simulator`), `wasm32`, or the host's architecture.
    pub arch: String,
    /// What kind of image it is.
    pub format: Format,
    /// The identity of the image a report names (see the module docs); `None` when it is not known at build time.
    pub image_id: Option<String>,
    /// The SHA-256 of the shipped file.
    pub sha256: String,
    /// The shipped file, relative to `build/`.
    pub shipped: String,
    /// The shipped file's size in bytes.
    pub shipped_bytes: u64,
    /// The file that resolves addresses (the unstripped twin, the debug module), relative to `build/symbols`;
    /// `None` for iOS, whose symbols are the app's dSYM.
    pub symbols: Option<String>,
    /// The function map of a wasm module, relative to `build/symbols`.
    pub function_map: Option<String>,
    /// The module a wasm build optimised with its DWARF kept (a debugger's, not address-compatible
    /// with the shipped module), relative to `build/symbols`.
    pub dwarf: Option<String>,
}

impl Entry {
    /// The entry as JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut object = json!({
            "platform": self.platform,
            "namespace": self.namespace,
            "coreVersion": self.core_version,
            "schemaHash": self.schema_hash.map(|h| format!("{h:#018x}")),
            "arch": self.arch,
            "format": self.format.word(),
            "imageId": self.image_id,
            "sha256": self.sha256,
            "shipped": self.shipped,
            "shippedBytes": self.shipped_bytes,
            "symbols": self.symbols,
        });
        if let Some(map) = &self.function_map {
            object["functionMap"] = json!(map);
        }
        if let Some(dwarf) = &self.dwarf {
            object["dwarf"] = json!(dwarf);
        }
        object
    }

    /// Reads an entry back; `None` for an object that is not one.
    #[must_use]
    pub fn from_json(value: &Value) -> Option<Entry> {
        let text = |key: &str| value.get(key)?.as_str().map(ToOwned::to_owned);
        Some(Entry {
            platform: text("platform")?,
            namespace: text("namespace")?,
            core_version: text("coreVersion").unwrap_or_default(),
            schema_hash: value
                .get("schemaHash")
                .and_then(Value::as_str)
                .and_then(|h| u64::from_str_radix(h.trim_start_matches("0x"), 16).ok()),
            arch: text("arch")?,
            format: Format::parse(value.get("format")?.as_str()?)?,
            image_id: text("imageId"),
            sha256: text("sha256").unwrap_or_default(),
            shipped: text("shipped").unwrap_or_default(),
            shipped_bytes: value
                .get("shippedBytes")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            symbols: text("symbols"),
            function_map: text("functionMap"),
            dwarf: text("dwarf"),
        })
    }

    fn key(&self) -> (&str, &str, &str) {
        (&self.platform, &self.namespace, &self.arch)
    }
}

/// The manifest of a `build/symbols` directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Its entries, ordered by platform, namespace and architecture.
    pub entries: Vec<Entry>,
}

impl Manifest {
    /// Reads `<dir>/manifest.json`; a missing, unreadable or foreign file is an empty manifest.
    #[must_use]
    pub fn read(dir: &Path) -> Manifest {
        let Ok(text) = std::fs::read_to_string(dir.join(FILE_NAME)) else {
            return Manifest::default();
        };
        Manifest::parse(&text).unwrap_or_default()
    }

    /// Parses the text of a manifest.
    ///
    /// # Errors
    ///
    /// A sentence saying why the text is not a manifest of this format.
    pub fn parse(text: &str) -> std::result::Result<Manifest, String> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("it is not JSON: {e}"))?;
        let version = value.get("version").and_then(Value::as_u64);
        if version != Some(FORMAT_VERSION) {
            return Err(format!(
                "its `version` is {}, this undra reads version {FORMAT_VERSION}",
                version.map_or("missing".to_owned(), |v| v.to_string())
            ));
        }
        let entries = value
            .get("artifacts")
            .and_then(Value::as_array)
            .ok_or("it has no `artifacts` array")?
            .iter()
            .filter_map(Entry::from_json)
            .collect();
        Ok(Manifest { entries })
    }

    /// Adds `entry`, replacing the one with the same platform, namespace and architecture.
    pub fn upsert(&mut self, entry: Entry) {
        self.entries.retain(|e| e.key() != entry.key());
        self.entries.push(entry);
        self.entries.sort_by(|a, b| a.key().cmp(&b.key()));
    }

    /// Forgets every entry of `platform` and `namespace` (a build is about to write them again).
    pub fn remove_platform(&mut self, platform: &str, namespace: &str) {
        self.entries
            .retain(|e| !(e.platform == platform && e.namespace == namespace));
    }

    /// The manifest as the text `manifest.json` holds.
    #[must_use]
    pub fn to_text(&self) -> String {
        let doc = json!({
            "version": FORMAT_VERSION,
            "undra": crate::version::SEMVER,
            "artifacts": self.entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
        });
        let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Writes `<dir>/manifest.json`.
    ///
    /// # Errors
    ///
    /// `C0010` when it cannot be written.
    pub fn write(&self, dir: &Path) -> Result<()> {
        write_if_changed(&dir.join(FILE_NAME), &self.to_text())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(platform: &str, arch: &str, id: Option<&str>) -> Entry {
        Entry {
            platform: platform.to_owned(),
            namespace: "acme_pay".to_owned(),
            core_version: "1.4.0".to_owned(),
            schema_hash: Some(0x5324_1303_b2d0_8c5e),
            arch: arch.to_owned(),
            format: Format::Elf,
            image_id: id.map(ToOwned::to_owned),
            sha256: "ab".repeat(32),
            shipped: format!("android/jniLibs/{arch}/libacme_pay.so"),
            shipped_bytes: 812_345,
            symbols: Some(format!("android/{arch}/libacme_pay.so")),
            function_map: None,
            dwarf: None,
        }
    }

    #[test]
    fn an_entry_survives_its_json() {
        let mut e = entry("web", "wasm32", Some("cd"));
        e.format = Format::Wasm;
        e.function_map = Some("web/acme_pay.wasm.functions.txt".to_owned());
        e.dwarf = Some("web/acme_pay.dwarf.wasm".to_owned());
        let json = e.to_json();
        assert_eq!(json["dwarf"], "web/acme_pay.dwarf.wasm");
        assert_eq!(json["schemaHash"], "0x53241303b2d08c5e");
        assert_eq!(json["format"], "wasm");
        assert_eq!(json["functionMap"], "web/acme_pay.wasm.functions.txt");
        assert_eq!(Entry::from_json(&json), Some(e));
        // No schema hash and no image id are `null`, not missing: the shape is the same.
        let mut ios = entry("ios", "ios-arm64", None);
        ios.schema_hash = None;
        ios.symbols = None;
        let json = ios.to_json();
        assert!(json["schemaHash"].is_null() && json["imageId"].is_null());
        assert!(json["symbols"].is_null());
        assert!(json.get("functionMap").is_none());
        assert_eq!(Entry::from_json(&json), Some(ios));
    }

    #[test]
    fn upserts_replace_by_platform_namespace_and_arch_and_keep_an_order() {
        let mut m = Manifest::default();
        m.upsert(entry("android", "x86_64", Some("01")));
        m.upsert(entry("android", "arm64-v8a", Some("02")));
        m.upsert(entry("android", "x86_64", Some("03")));
        let ids: Vec<(&str, &str)> = m
            .entries
            .iter()
            .map(|e| (e.arch.as_str(), e.image_id.as_deref().unwrap()))
            .collect();
        assert_eq!(ids, [("arm64-v8a", "02"), ("x86_64", "03")]);
        m.upsert(entry("web", "wasm32", Some("04")));
        m.remove_platform("android", "acme_pay");
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].platform, "web");
    }

    #[test]
    fn the_text_round_trips_and_foreign_files_are_empty_manifests() {
        let mut m = Manifest::default();
        m.upsert(entry("android", "arm64-v8a", Some("02")));
        let text = m.to_text();
        assert!(
            text.contains("\"version\": 1") && text.ends_with("}\n"),
            "{text}"
        );
        assert_eq!(Manifest::parse(&text).unwrap(), m);
        assert!(Manifest::parse("[]").is_err());
        assert!(
            Manifest::parse("{\"version\": 9, \"artifacts\": []}")
                .unwrap_err()
                .contains("version")
        );
        let dir = crate::fsutil::unique_temp_dir("manifest");
        assert_eq!(Manifest::read(&dir), Manifest::default());
        m.write(&dir).unwrap();
        assert_eq!(Manifest::read(&dir), m);
        let _ = std::fs::remove_dir_all(dir);
    }
}
