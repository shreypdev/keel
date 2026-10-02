//! Symbol files: what `undra build --release` writes next to the artefacts it ships, and what
//! `undra symbolicate` reads back (ADR-046).
//!
//! A release build keeps the line tables of the core (`debug = "line-tables-only"`, no `strip` in
//! the shim's profile), strips the copies it ships and keeps the unstripped ones, so the code is
//! the same and the addresses of a crash report resolve:
//!
//! | Platform | Shipped (stripped) | Symbols |
//! |---|---|---|
//! | iOS | `ios/<Ns>Core.xcframework/<slice>/lib<ns>.a`: nothing is stripped; the prelinked object's debug map names Cargo's objects, which have the DWARF | the **app's own dSYM** (Xcode, `dwarf-with-dsym`, follows the map) |
//! | Android | `android/jniLibs/<abi>/lib<ns>.so` | `symbols/android/<abi>/lib<ns>.so`, and `symbols/android/native-debug-symbols.zip` (Play layout) |
//! | Web | `web/<ns>.wasm` | `symbols/web/<ns>.debug.wasm` (the same code with its names), `symbols/web/<ns>.wasm.functions.txt` and `symbols/web/<ns>.dwarf.wasm` (DWARF, for browsers) |
//! | Host | `host/lib<ns>.{dylib,so}` | `lib<ns>.dylib.dSYM` / `lib<ns>.so.debug` next to it |
//!
//! `symbols/manifest.json` ([`manifest`]) lists, per shipped image, the namespace, the core
//! version, the schema hash and the image identity a panic report carries. `--no-symbols` skips all
//! of it (the shipped copies are still stripped); debug builds write none.

pub(crate) mod image;
pub(crate) mod manifest;
pub(crate) mod resolve;
pub(crate) mod sha256;
pub(crate) mod tools;
pub(crate) mod wasm;
pub(crate) mod zip;

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use crate::builds::host;
use crate::cargo::{Profile, unpacked_debuginfo};
use crate::error::Result;
use crate::schema;
use crate::session::Session;

pub use manifest::{Entry, Format, Manifest};

/// The part of a manifest entry that is the same for every artefact of one build: whose core it
/// is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The core's namespace.
    pub namespace: String,
    /// The core crate's version (what `export_core!` is given).
    pub core_version: String,
    /// The schema hash, read from the built core; `None` when that failed (a warning said why).
    pub schema_hash: Option<u64>,
}

/// The symbol outputs of one `undra build`: where they go, whether they are wanted, and whose
/// core they describe.
pub struct Symbols<'s, 'a> {
    session: &'s Session<'a>,
    /// Whether symbol outputs are written (`--no-symbols` turns them off).
    pub enabled: bool,
    /// Whether this is a release build: only then is the schema hash read (it costs a build of the
    /// core for this machine; the web dev loop, whose every rebuild writes its debug module, must not pay it).
    release: bool,
    identity: OnceCell<Identity>,
}

impl<'s, 'a> Symbols<'s, 'a> {
    /// The symbol outputs of a build of `session` (`release`: a release build).
    #[must_use]
    pub fn new(session: &'s Session<'a>, enabled: bool, release: bool) -> Symbols<'s, 'a> {
        Symbols {
            session,
            enabled,
            release,
            identity: OnceCell::new(),
        }
    }

    /// The `--config` arguments a build of `profile` adds for the symbol outputs.
    ///
    /// With symbols (the default) a release build keeps the line tables the shim's profile asks for,
    /// and on Apple targets (`apple`) asks Cargo to leave the debug info in the objects
    /// (`split-debuginfo = "unpacked"`). Without them (`--no-symbols`) the profile of before
    /// ADR-046 is restored, so the shipped code and bytes are what they were: no debug info, symbols
    /// stripped by the linker. A debug build needs nothing: it is unstripped anyway.
    #[must_use]
    pub fn cargo_config(&self, profile: Profile, apple: bool) -> Vec<String> {
        match (profile, self.enabled) {
            (Profile::Dev, _) => Vec::new(),
            (release, true) if apple => vec![unpacked_debuginfo(release)],
            (_, true) => Vec::new(),
            (_, false) => vec![
                "profile.release.debug=false".to_owned(),
                "profile.release.strip=\"symbols\"".to_owned(),
            ],
        }
    }

    /// `build/symbols`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.session.project.build_dir().join("symbols")
    }

    /// Whose core this is: the namespace and version, and (in a release build) the schema hash read
    /// from a host build of the core, the way `undra bindgen` reads it (a build that cached the same
    /// library costs nothing). When the hash cannot be read, or the build is a debug one, the
    /// manifest says `null`.
    ///
    /// # Errors
    ///
    /// See [`Session::core`] and [`Session::namespace`].
    pub fn identity(&self) -> Result<&Identity> {
        self.identity_from(|| None)
    }

    /// [`Symbols::identity`], with a way to learn the schema hash that costs less than a build of
    /// the core for this machine (`known`: a wasm module can tell it to `node`). When `known` has
    /// no answer, a release build reads it from the host build.
    ///
    /// # Errors
    ///
    /// See [`Symbols::identity`].
    pub fn identity_from(&self, known: impl FnOnce() -> Option<u64>) -> Result<&Identity> {
        if let Some(identity) = self.identity.get() {
            return Ok(identity);
        }
        let namespace = self.session.namespace()?;
        let core = self.session.core()?;
        let version = if core.version.is_empty() {
            "0.0.0".to_owned()
        } else {
            core.version.clone()
        };
        let schema_hash = known().or_else(|| {
            if !self.release {
                return None;
            }
            self.session
                .ui
                .detail("reading the schema hash of the core for the symbol manifest");
            match host::cdylib(self.session, false).and_then(|library| {
                schema::load_from_library(&library, &namespace, &core.package, false)
            }) {
                Ok(schema) => Some(schema.hash()),
                Err(e) => {
                    self.session.ui.warn(&format!(
                        "the schema hash could not be read for the symbol manifest ({}); it will say null",
                        e.what
                    ));
                    None
                }
            }
        });
        Ok(self.identity.get_or_init(|| Identity {
            namespace,
            core_version: version,
            schema_hash,
        }))
    }

    /// Records `entry` in `build/symbols/manifest.json`, replacing the entry it supersedes.
    ///
    /// # Errors
    ///
    /// `C0010` when the manifest cannot be written.
    pub fn record(&self, entry: Entry) -> Result<()> {
        let dir = self.dir();
        let mut manifest = Manifest::read(&dir);
        manifest.upsert(entry);
        manifest.write(&dir)
    }

    /// Forgets what an earlier build wrote for `platform` (its files and manifest entries): a
    /// build that does not write symbols must not leave files that describe another build.
    ///
    /// # Errors
    ///
    /// `C0010` when the manifest cannot be written.
    pub fn forget(&self, platform: &str) -> Result<()> {
        let dir = self.dir();
        let Ok(namespace) = self.session.namespace() else {
            return Ok(());
        };
        if !dir.join(manifest::FILE_NAME).is_file() {
            return Ok(());
        }
        let mut manifest = Manifest::read(&dir);
        manifest.remove_platform(platform, &namespace);
        manifest.write(&dir)?;
        crate::fsutil::remove_dir_all(&dir.join(platform))
    }
}

/// `path` relative to `base`, with `/` separators (the manifest's spelling): `..` steps climb out of
/// `base` when `path` is beside it (the host library's dSYM, `../host/lib<ns>.dylib.dSYM`, is not
/// below `build/symbols`). `path` itself when the two share no root.
#[must_use]
pub fn slash_relative(base: &Path, path: &Path) -> String {
    use std::path::Component;
    let base: Vec<Component<'_>> = base.components().collect();
    let target: Vec<Component<'_>> = path.components().collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return path.display().to_string();
    }
    let mut parts: Vec<String> = vec!["..".to_owned(); base.len() - common];
    parts.extend(
        target[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Symbols` for the tests of `cargo_config`, which never touches its session.
    fn config_of(enabled: bool, profile: Profile, apple: bool) -> Vec<String> {
        let session = crate::session::Session::new(
            crate::project::Project {
                root: PathBuf::from("/p"),
                config: crate::config::ProjectConfig::new(
                    "p",
                    "com.example.p",
                    crate::config::Platform::ALL.to_vec(),
                ),
            },
            &crate::sys::RealSys,
            crate::ui::Ui::plain(),
        );
        Symbols::new(&session, enabled, true).cargo_config(profile, apple)
    }

    #[test]
    fn symbols_ask_cargo_for_unpacked_debug_info_on_apple_and_no_symbols_restores_the_old_profile()
    {
        // With symbols the shim's own profile (line tables, no strip) is what is built; an Apple
        // build also keeps the debug info in the objects.
        assert_eq!(
            config_of(true, Profile::Release, true),
            ["profile.release.split-debuginfo=\"unpacked\""]
        );
        assert!(config_of(true, Profile::Release, false).is_empty());
        assert!(config_of(true, Profile::ReleaseWasm, false).is_empty());
        // `--no-symbols`: no debug info and the linker strips, as before ADR-046 (the wasm profile
        // inherits `release`).
        for apple in [true, false] {
            assert_eq!(
                config_of(false, Profile::Release, apple),
                [
                    "profile.release.debug=false",
                    "profile.release.strip=\"symbols\""
                ]
            );
        }
        assert_eq!(
            config_of(false, Profile::ReleaseWasm, false),
            [
                "profile.release.debug=false",
                "profile.release.strip=\"symbols\""
            ]
        );
        // A debug build needs nothing: it is unstripped anyway.
        assert!(config_of(false, Profile::Dev, true).is_empty());
        assert!(config_of(true, Profile::Dev, true).is_empty());
    }

    #[test]
    fn manifest_paths_use_slashes_and_are_relative() {
        assert_eq!(
            slash_relative(
                Path::new("/p/build"),
                Path::new("/p/build/android/jniLibs/arm64-v8a/libx.so")
            ),
            "android/jniLibs/arm64-v8a/libx.so"
        );
        assert_eq!(
            slash_relative(
                Path::new("/p/build/symbols"),
                Path::new("/p/build/host/libx.dylib.dSYM")
            ),
            "../host/libx.dylib.dSYM"
        );
        assert_eq!(
            slash_relative(Path::new("/p/build"), Path::new("/p/build")),
            ""
        );
    }
}
