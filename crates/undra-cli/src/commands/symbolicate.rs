//! `undra symbolicate`.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::cli::SymbolicateArgs;
use crate::error::{CliError, Code, Result};
use crate::project::Project;
use crate::symbols::manifest::{Entry, Format, Manifest};
use crate::symbols::resolve::{
    self, Located, MatchedBy, Report, format_frame, manifest_path, normalize_image_id,
    parse_address, parse_report,
};
use crate::sys::{Os, Sys};
use crate::toolchain::Toolchain;

use super::Env;

/// Runs `undra symbolicate`.
///
/// # Errors
///
/// `C0009` for input that is not a report or an address, or that no symbol file fits; `C0003`
/// without the tool that reads the symbol file; `C0004` when it fails.
pub fn run(env: &Env<'_>, args: &SymbolicateArgs) -> Result<()> {
    let mut report = read_inputs(args)?;
    if let Some(id) = &args.image_id {
        report.image_id = Some(normalize_image_id(id).ok_or_else(|| {
            CliError::bad_argument(
                format!("`{id}` is not an image id"),
                "an image id is the hex of an ELF build id or a Mach-O UUID, or the SHA-256 of a wasm module",
                "copy it from the report's `imageId` (a UUID with dashes is fine)",
            )
        })?);
    }
    if report.frames.is_empty() {
        return Err(CliError::bad_argument(
            "there is nothing to resolve: no report and no address was given",
            "`undra symbolicate` resolves the addresses of a panic report (JSON) or addresses on the command line",
            "undra symbolicate report.json, or: undra symbolicate --platform android --image-id <build id> 0x1234 0x5678",
        ));
    }
    let platform = args.platform.as_deref().map(parse_platform).transpose()?;
    let toolchain = Toolchain::detect(env.sys);
    let (dir, manifest) = load_manifest(env, args)?;

    // A dSYM named on the command line decides on its own (the app's, which the manifest cannot know).
    let resolved = if let Some(dsym) = &args.dsym {
        macho(env.sys, &toolchain, dsym, &report, args.arch.as_deref())?
    } else {
        let (entry, how) = resolve::select(&manifest, &report, platform).map_err(|why| {
            CliError::new(
                Code::BadArgument,
                format!("no symbol file fits this report: {why}"),
                format!(
                    "the manifest in {} lists the artefacts `undra build --release` made; the report names the image it came from",
                    dir.display()
                ),
                "pass the directory of the build that shipped (`--symbols <dir>`), `--platform`, or for iOS the app's dSYM (`--dsym <App.dSYM>`)",
            )
        })?;
        warn_on_mismatch(env, entry, &report, how);
        match entry.format {
            Format::Elf => {
                let file = symbol_file(&dir, entry)?;
                resolve::resolve_elf(env.sys, &toolchain, &file, &addresses(&report))?
            }
            Format::Wasm => {
                let file = symbol_file(&dir, entry)?;
                let map = entry
                    .function_map
                    .as_deref()
                    .map(|m| manifest_path(&dir, m));
                let dwarf = entry
                    .dwarf
                    .as_deref()
                    .map(|d| manifest_path(&dir, d))
                    .filter(|d| d.is_file());
                resolve::resolve_wasm(
                    env.sys,
                    &toolchain,
                    &file,
                    map.as_deref(),
                    dwarf.as_deref(),
                    &report.frames,
                )?
            }
            Format::Macho => {
                let dsym = match &entry.symbols {
                    Some(symbols) => manifest_path(&dir, symbols),
                    None => spotlight(env.sys, &toolchain, report.image_id.as_deref())
                        .ok_or_else(|| need_dsym(&report))?,
                };
                macho(env.sys, &toolchain, &dsym, &report, args.arch.as_deref())?
            }
        }
    };
    let mut resolved = resolved;
    resolve::demangle(env.sys, &toolchain, &mut resolved);

    let mut found = 0;
    for (frame, located) in report.frames.iter().zip(&resolved) {
        match located.first() {
            Some(first) if *first != Located::default() => {
                found += 1;
                for line in located {
                    env.ui.line(&format_frame(frame.address, line));
                }
            }
            _ => env
                .ui
                .line(&format_frame(frame.address, &Located::default())),
        }
    }
    if found == 0 {
        return Err(CliError::new(
            Code::BadArgument,
            format!(
                "none of the {} addresses could be resolved",
                report.frames.len()
            ),
            "the addresses are not inside the image these symbols belong to (another build, or addresses that are not image-relative offsets)",
            "check the report's `imageId` against the manifest (`undra symbolicate` prints a warning when the schema hash or version differs), or pass the symbols of the build that shipped with `--symbols`",
        ));
    }
    Ok(())
}

fn addresses(report: &Report) -> Vec<u64> {
    report.frames.iter().map(|f| f.address).collect()
}

/// The report, and the bare addresses, the command line names.
fn read_inputs(args: &SymbolicateArgs) -> Result<Report> {
    let mut report = Report::default();
    for input in &args.inputs {
        let text = if input == "-" {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| CliError::io("read", Path::new("<stdin>"), &e))?;
            Some(text)
        } else if parse_address(input).is_some() && !Path::new(input).is_file() {
            report.frames.push(parse_address(input).expect("checked"));
            None
        } else if Path::new(input).is_file() {
            Some(
                std::fs::read_to_string(input)
                    .map_err(|e| CliError::io("read", Path::new(input), &e))?,
            )
        } else {
            return Err(CliError::bad_argument(
                format!("`{input}` is neither a file nor an address"),
                "inputs are a report (a JSON file, or - for standard input) and addresses (0x1a2b, 6699, wasm-function[41]:0x1a2b)",
                "undra symbolicate report.json",
            ));
        };
        if let Some(text) = text {
            let parsed = parse_report(&text).map_err(|why| {
                CliError::bad_argument(
                    format!("`{input}` is not a panic report: {why}"),
                    "a report is the JSON of the `UndraPanicReport` the app's `onPanic` receives",
                    "see `undra symbolicate --help` for the shape",
                )
            })?;
            report.frames.extend(parsed.frames);
            report.namespace = parsed.namespace.or(report.namespace);
            report.image_id = parsed.image_id.or(report.image_id);
            report.core_version = parsed.core_version.or(report.core_version);
            report.schema_hash = parsed.schema_hash.or(report.schema_hash);
        }
    }
    Ok(report)
}

fn parse_platform(text: &str) -> Result<&'static str> {
    match text.trim().to_ascii_lowercase().as_str() {
        "ios" => Ok("ios"),
        "android" => Ok("android"),
        "web" | "wasm" => Ok("web"),
        "host" | "macos" | "jvm" => Ok("host"),
        other => Err(CliError::bad_argument(
            format!("`{other}` is not a platform of a symbol file"),
            "the manifest lists artefacts per platform",
            "use one of: ios, android, web, host",
        )),
    }
}

/// The symbol directory and its manifest: `--symbols`, else `build/symbols` of the project.
fn load_manifest(env: &Env<'_>, args: &SymbolicateArgs) -> Result<(PathBuf, Manifest)> {
    let dir = match &args.symbols {
        Some(dir) => dir.clone(),
        None => match Project::discover(&env.start_dir()?) {
            Ok(project) => project.build_dir().join("symbols"),
            // A dSYM needs no manifest: without a project it is enough.
            Err(e) if e.code == Code::NoProject && args.dsym.is_some() => PathBuf::from("."),
            Err(e) => return Err(e),
        },
    };
    let manifest = Manifest::read(&dir);
    if manifest.entries.is_empty() && args.dsym.is_none() {
        return Err(CliError::new(
            Code::BadArgument,
            format!(
                "{} has no symbol manifest with artefacts",
                dir.join("manifest.json").display()
            ),
            "`undra build --release` writes the symbol files and manifest.json below build/symbols (not with --no-symbols)",
            "build the release you are symbolicating (or fetch its symbols artifact) and pass its directory with `--symbols`; for iOS alone, `--dsym <App.dSYM>` is enough",
        ));
    }
    Ok((dir, manifest))
}

/// The symbol file of an entry, which must exist.
fn symbol_file(dir: &Path, entry: &Entry) -> Result<PathBuf> {
    let relative = entry.symbols.as_deref().ok_or_else(|| {
        CliError::bad_argument(
            format!(
                "the manifest entry for {} {} lists no symbol file",
                entry.platform, entry.arch
            ),
            "an entry without `symbols` has its symbols elsewhere (iOS: the app's dSYM)",
            "pass `--dsym` for iOS",
        )
    })?;
    let file = manifest_path(dir, relative);
    if !file.is_file() {
        return Err(CliError::new(
            Code::Io,
            format!("the symbol file {} does not exist", file.display()),
            "the manifest names it, but this directory does not have it (the symbols artifact of that build may not be unpacked here)",
            "restore the file or pass the symbols of the build with `--symbols`",
        ));
    }
    Ok(file)
}

/// Says what does not add up between the report and the entry it was matched to.
fn warn_on_mismatch(env: &Env<'_>, entry: &Entry, report: &Report, how: MatchedBy) {
    if how == MatchedBy::Namespace && report.image_id.is_some() {
        env.ui.warn(&format!(
            "the report's image id {} is not in the manifest: using the only {} artefact of `{}`; the addresses are those of another build if it is not this one",
            report.image_id.as_deref().unwrap_or_default(),
            entry.platform,
            entry.namespace
        ));
    }
    if let (Some(report_hash), Some(entry_hash)) = (report.schema_hash, entry.schema_hash) {
        if report_hash != entry_hash {
            env.ui.warn(&format!(
                "the report's schema hash {report_hash:#018x} is not the symbols' {entry_hash:#018x}: they are not the same build of the core"
            ));
        }
    }
    if let Some(version) = &report.core_version {
        if !entry.core_version.is_empty() && version != &entry.core_version {
            env.ui.warn(&format!(
                "the report's core version {version} is not the symbols' {}",
                entry.core_version
            ));
        }
    }
}

fn need_dsym(report: &Report) -> CliError {
    CliError::new(
        Code::BadArgument,
        "an iOS crash is resolved with the app's dSYM, which was not given and was not found",
        format!(
            "a static library has no symbols of its own: the Rust frames are in the dSYM of the app that links the core{}",
            report.image_id.as_ref().map_or(String::new(), |id| format!(
                " (the report names its UUID as {id})"
            ))
        ),
        "pass it: `undra symbolicate report.json --dsym path/to/App.app.dSYM` (from the Xcode archive's dSYMs folder, or the build products)",
    )
}

/// Spotlight's answer for the dSYM with this UUID (macOS).
fn spotlight(sys: &dyn Sys, toolchain: &Toolchain, image_id: Option<&str>) -> Option<PathBuf> {
    if sys.os() != Os::Macos {
        return None;
    }
    let id = image_id.filter(|id| id.len() == 32)?;
    let dashed = format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    )
    .to_ascii_uppercase();
    let mdfind = toolchain.which(sys, "mdfind")?;
    let query = format!("com_apple_xcode_dsym_uuids == {dashed}");
    let out = sys.run(&mdfind, &[&query], &toolchain.env_pairs())?;
    out.stdout
        .lines()
        .map(PathBuf::from)
        .find(|p| p.extension().is_some_and(|e| e == "dSYM"))
}

/// Resolves a report against a dSYM (or the DWARF file of one).
fn macho(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    dsym: &Path,
    report: &Report,
    arch: Option<&str>,
) -> Result<Vec<Vec<Located>>> {
    let id = report.image_id.as_deref();
    let dwarf = resolve::dwarf_file(dsym, id).map_err(|why| {
        CliError::new(
            Code::BadArgument,
            format!("{}: {why}", dsym.display()),
            "a dSYM is a bundle (`App.app.dSYM`) with the DWARF file below Contents/Resources/DWARF",
            "pass the dSYM bundle of the app that crashed",
        )
    })?;
    let slice = resolve::pick_slice(&dwarf, id, arch).map_err(|why| {
        CliError::new(
            Code::BadArgument,
            why,
            "the dSYM and the report must be of the same build of the app (same UUID)",
            "use the dSYM of the build that crashed, or pass `--arch`",
        )
    })?;
    resolve::resolve_macho(sys, toolchain, &dwarf, &slice, &addresses(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(inputs: &[&str]) -> SymbolicateArgs {
        SymbolicateArgs {
            inputs: inputs.iter().map(|s| (*s).to_owned()).collect(),
            platform: None,
            image_id: None,
            symbols: None,
            dsym: None,
            arch: None,
        }
    }

    #[test]
    fn addresses_and_report_files_are_both_inputs() {
        let dir = crate::fsutil::unique_temp_dir("symbolicate-input");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("report.json");
        std::fs::write(
            &file,
            r#"{"namespace":"acme","imageId":"AB-CD","frames":[{"address":4096}]}"#,
        )
        .unwrap();
        let report = read_inputs(&args(&[
            "0x20",
            file.to_str().unwrap(),
            "wasm-function[3]:0x40",
        ]))
        .unwrap();
        let frames: Vec<(u64, Option<u32>)> = report
            .frames
            .iter()
            .map(|f| (f.address, f.function))
            .collect();
        assert_eq!(frames, [(0x20, None), (4096, None), (0x40, Some(3))]);
        assert_eq!(report.namespace.as_deref(), Some("acme"));
        assert_eq!(report.image_id.as_deref(), Some("abcd"));
        // Neither a file nor an address: said, with the way out.
        let error = read_inputs(&args(&["report.jsno"])).unwrap_err();
        assert_eq!(error.code, Code::BadArgument);
        assert!(
            error.what.contains("neither a file nor an address"),
            "{}",
            error.what
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn platforms_have_aliases_and_an_error_that_lists_them() {
        assert_eq!(parse_platform("wasm").unwrap(), "web");
        assert_eq!(parse_platform("macOS").unwrap(), "host");
        assert!(
            parse_platform("tv")
                .unwrap_err()
                .fix
                .contains("ios, android, web, host")
        );
    }

    #[test]
    fn a_dsym_is_looked_up_by_uuid_through_spotlight() {
        use crate::sys::fake::FakeSys;
        let sys = FakeSys::macos()
            .with_tool("mdfind", "/usr/bin/mdfind")
            .with_output(
                "mdfind",
                "com_apple_xcode_dsym_uuids == 72B416A9-50E2-3938-9AEB-F6AD16BB1385",
                "/b/App.app.dSYM\n",
            );
        let toolchain = Toolchain::default();
        assert_eq!(
            spotlight(
                &sys,
                &toolchain,
                Some("72b416a950e239389aeb" /* too short */)
            ),
            None
        );
        assert_eq!(
            spotlight(&sys, &toolchain, Some("72b416a950e239389aebf6ad16bb1385")),
            Some(PathBuf::from("/b/App.app.dSYM"))
        );
    }
}
