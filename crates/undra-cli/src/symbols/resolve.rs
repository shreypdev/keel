//! Resolving the addresses of a panic report to `symbol (file:line)` with the symbol files of a
//! release build (`undra symbolicate`, ADR-046).
//!
//! # The input
//!
//! A report as the apps' `onPanic` produces it, serialised as JSON (camelCase, what the three
//! runtimes' `UndraPanicReport` encode to):
//!
//! ```json
//! {
//!   "namespace": "playground_core",
//!   "imageId": "3f2a9c1e0b7d4a55c6e8d9f001122334455667788",
//!   "coreVersion": "0.1.0",
//!   "schemaHash": "0x53241303b2d08c5e",
//!   "frames": [ { "address": 134567, "symbol": null, "file": null, "line": null },
//!               { "address": "0x20e5f" } ]
//! }
//! ```
//!
//! Only `frames[].address` is needed; the rest selects the symbol file. An address is a number or
//! a string (`"0x20e5f"`, decimal digits, or a V8 stack position `wasm-function[41]:0x1a2b`).
//! `imageId` matches the manifest's `imageId` (lowercase hex; a UUID with dashes is accepted).
//!
//! # What an address means
//!
//! * **Native** (iOS, Android, host): the report's `address` is the instruction address minus the
//!   load address of the image that holds the core, pointing into the *call* instruction (not the
//!   one after it, so nothing is subtracted before looking it up). On Android the image is the
//!   `lib<ns>.so`, whose first segment is at virtual address 0, so the address *is* the virtual
//!   address `llvm-symbolizer` takes. On iOS the image is the app executable (the core is linked
//!   into it), whose `__TEXT` segment starts at `vmaddr` (0x100000000 for an arm64 executable):
//!   `atos` is given `vmaddr + address`, with `vmaddr` read from the app's dSYM.
//! * **wasm**: the module offset of the `wasm-function[i]:0x…` line of a V8 stack trace (`0x…`
//!   counts bytes from the start of the module). `i` is the function index; the function map
//!   (`<ns>.wasm.functions.txt`) names it, exactly: the debug module has the shipped module's code.
//!   The line is the line where the function is *defined*, from the DWARF module
//!   (`<ns>.dwarf.wasm`, found by the function's name): `wasm-opt` cannot keep DWARF through the
//!   passes that make the shipped module small, so the lines of a frame inside a function are not
//!   known; the panic's own `location` is in the report.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{CliError, Code, Result};
use crate::sys::{Os, Sys};
use crate::toolchain::Toolchain;

use super::image;
use super::manifest::{Entry, Manifest};
use super::wasm;

/// One frame of the input: where, and for a wasm frame which function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputFrame {
    /// The report's `address` (see the module docs for what it counts from).
    pub address: u64,
    /// The wasm function index of a `wasm-function[i]:0x…` position.
    pub function: Option<u32>,
}

/// What `undra symbolicate` reads from a report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The core's namespace.
    pub namespace: Option<String>,
    /// The image identity, normalised (lowercase hex, no separators).
    pub image_id: Option<String>,
    /// The core's version.
    pub core_version: Option<String>,
    /// The core's schema hash.
    pub schema_hash: Option<u64>,
    /// The frames, innermost first.
    pub frames: Vec<InputFrame>,
}

/// One resolved frame: a function, and where its source is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Located {
    /// The function, demangled when the tool could.
    pub symbol: Option<String>,
    /// The source file (remapped in a release build: `/undra/app/…`, `/undra/src/…`, `~/…`; ADR-052).
    pub file: Option<String>,
    /// The line.
    pub line: Option<u32>,
}

/// The identity text normalised: lowercase hex without `0x` or separators; `None` when it is not hex.
#[must_use]
pub fn normalize_image_id(text: &str) -> Option<String> {
    let cleaned: String = text
        .trim()
        .trim_start_matches("0x")
        .chars()
        .filter(|c| *c != '-' && *c != ':')
        .collect::<String>()
        .to_ascii_lowercase();
    (!cleaned.is_empty() && cleaned.chars().all(|c| c.is_ascii_hexdigit())).then_some(cleaned)
}

/// An address given as text: `0x20e5f`, `134567`, or a V8 position `wasm-function[41]:0x1a2b`
/// (anywhere in a line of a stack trace).
#[must_use]
pub fn parse_address(text: &str) -> Option<InputFrame> {
    let text = text.trim();
    if let Some(at) = text.find("wasm-function[") {
        let rest = &text[at + "wasm-function[".len()..];
        let (index, after) = rest.split_once(']')?;
        let address = after.strip_prefix(':')?;
        let address = address
            .split(|c: char| !c.is_ascii_hexdigit() && c != 'x' && c != 'X')
            .next()?;
        return Some(InputFrame {
            address: parse_number(address)?,
            function: index.trim().parse().ok(),
        });
    }
    Some(InputFrame {
        address: parse_number(text)?,
        function: None,
    })
}

fn parse_number(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Reads a report from its JSON.
///
/// # Errors
///
/// A sentence saying what is wrong with the text.
pub fn parse_report(text: &str) -> std::result::Result<Report, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("it is not JSON: {e}"))?;
    let frames = if let Some(list) = value.as_array() {
        list
    } else {
        value.get("frames").and_then(Value::as_array).ok_or(
            "it has no `frames` array (a report is { \"frames\": [ { \"address\": ... } ], ... })",
        )?
    };
    let mut out = Report::default();
    for (i, frame) in frames.iter().enumerate() {
        let address = frame.get("address").unwrap_or(frame);
        let parsed = match address {
            Value::Number(n) => n.as_u64().map(|address| InputFrame {
                address,
                function: None,
            }),
            Value::String(s) => parse_address(s),
            _ => None,
        };
        let mut parsed = parsed.ok_or_else(|| {
            format!(
                "frame {i} has no usable `address` (a number, \"0x…\" or \"wasm-function[i]:0x…\")"
            )
        })?;
        // A frame that names itself `wasm-function[i]` carries the function index.
        if parsed.function.is_none() {
            parsed.function = frame
                .get("symbol")
                .and_then(Value::as_str)
                .and_then(parse_function_symbol);
        }
        out.frames.push(parsed);
    }
    let text_of = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| value.get(k).and_then(Value::as_str))
            .map(ToOwned::to_owned)
    };
    out.namespace = text_of(&["namespace"]).filter(|s| !s.is_empty());
    out.core_version = text_of(&["coreVersion", "core_version"]).filter(|s| !s.is_empty());
    out.image_id = text_of(&["imageId", "image_id"]).and_then(|id| normalize_image_id(&id));
    out.schema_hash = ["schemaHash", "schema_hash"]
        .iter()
        .find_map(|k| match value.get(k)? {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => parse_number(s),
            _ => None,
        });
    Ok(out)
}

/// The index of a frame symbol `wasm-function[41]` (what the TypeScript runtime names a frame of
/// a module without names).
fn parse_function_symbol(symbol: &str) -> Option<u32> {
    let rest = symbol.trim().strip_prefix("wasm-function[")?;
    rest.split(']').next()?.parse().ok()
}

/// How an entry was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchedBy {
    /// The report's image id is the entry's.
    ImageId,
    /// The namespace (and platform) pick exactly one entry.
    Namespace,
}

/// The manifest entry whose symbols resolve `report`.
///
/// The image id decides (it names one build of one image); without a match the namespace does, as
/// long as it leaves one entry (one per platform and architecture in a manifest: say `platform`
/// or pass the image id when it does not).
///
/// # Errors
///
/// A sentence saying why no entry, or too many, fit.
pub fn select<'a>(
    manifest: &'a Manifest,
    report: &Report,
    platform: Option<&str>,
) -> std::result::Result<(&'a Entry, MatchedBy), String> {
    let in_platform = |e: &&Entry| platform.is_none_or(|p| e.platform == p);
    if let Some(id) = &report.image_id {
        let by_id: Vec<&Entry> = manifest
            .entries
            .iter()
            .filter(in_platform)
            .filter(|e| e.image_id.as_deref() == Some(id))
            .collect();
        if let Some(first) = by_id.first() {
            return Ok((first, MatchedBy::ImageId));
        }
    }
    let by_namespace: Vec<&Entry> = manifest
        .entries
        .iter()
        .filter(in_platform)
        .filter(|e| report.namespace.as_ref().is_none_or(|n| &e.namespace == n))
        .collect();
    match by_namespace.as_slice() {
        [] => Err(match (&report.image_id, &report.namespace) {
            (None, None) => "the report names neither an image id nor a namespace, and the manifest has no artefact for this platform".to_owned(),
            (id, ns) => format!(
                "the manifest has no artefact{}{}{}",
                id.as_ref().map_or(String::new(), |id| format!(" with image id {id}")),
                if id.is_some() && ns.is_some() { " or" } else { "" },
                ns.as_ref().map_or(String::new(), |ns| format!(" of namespace `{ns}`"))
            ),
        }),
        [one] => Ok((one, MatchedBy::Namespace)),
        many => {
            let list: Vec<String> = many
                .iter()
                .map(|e| format!("{} {} (image {})", e.platform, e.arch, e.image_id.as_deref().unwrap_or("unknown")))
                .collect();
            Err(format!(
                "{} artefacts fit and the report's image id matches none of them: {}",
                many.len(),
                list.join("; ")
            ))
        }
    }
}

/// The path a manifest names (`symbols`, relative to the manifest's directory), without `..`.
#[must_use]
pub fn manifest_path(dir: &Path, relative: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for component in dir.join(relative).components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---- reading tool output ----------------------------------------------------------------------

/// `line` as `file:line` (the column, if any, dropped).
fn split_location(location: &str) -> (Option<String>, Option<u32>) {
    let location = location.trim();
    if location.is_empty() || location.starts_with("??") {
        return (None, None);
    }
    let mut parts = location.rsplitn(3, ':');
    let last = parts.next().unwrap_or_default();
    let middle = parts.next();
    let first = parts.next();
    // `file:line:column`, `file:line` or a bare `file`.
    match (first, middle, last.parse::<u32>()) {
        (Some(file), Some(line), Ok(_)) => (Some(file.to_owned()), line.parse().ok()),
        (None, Some(file), Ok(line)) => (Some(file.to_owned()), Some(line)),
        _ => (Some(location.to_owned()), None),
    }
}

/// The blocks `llvm-symbolizer --print-address` prints: for each queried address, its frames
/// (the function, then its `file:line:column`), innermost first; blocks end at a blank line.
///
/// A frame whose function is unknown (`??`) has no symbol; one without a location no file.
#[must_use]
pub fn parse_symbolizer(output: &str) -> Vec<Vec<Located>> {
    let mut blocks: Vec<Vec<Located>> = Vec::new();
    let mut lines = output.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim_end();
        if !line.starts_with("0x") {
            continue;
        }
        let mut frames = Vec::new();
        while let Some(function) = lines.next_if(|l| !l.trim().is_empty()) {
            let location = lines.next_if(|l| !l.trim().is_empty()).unwrap_or_default();
            let (file, line) = split_location(location);
            let symbol = function.trim();
            frames.push(Located {
                symbol: (!symbol.is_empty() && !symbol.starts_with("??"))
                    .then(|| symbol.to_owned()),
                file,
                line: line.filter(|l| *l != 0),
            });
        }
        blocks.push(frames);
    }
    blocks
}

/// One line of `atos -fullPath` output: `symbol (in image) (file:line)`, `symbol (in image) + 24`,
/// or the bare address when it knows nothing.
#[must_use]
pub fn parse_atos_line(line: &str) -> Located {
    let line = line.trim();
    if line.is_empty() || (line.starts_with("0x") && !line.contains(' ')) {
        return Located::default();
    }
    let (symbol, rest) = match line.find(" (in ") {
        Some(at) => (&line[..at], &line[at + 5..]),
        None => (line, ""),
    };
    let location = rest
        .find(") (")
        .map(|at| rest[at + 3..].trim_end_matches(')'))
        .filter(|l| !l.is_empty());
    let (file, line_number) = location.map_or((None, None), split_location);
    Located {
        symbol: Some(symbol.to_owned()).filter(|s| !s.is_empty()),
        file,
        line: line_number,
    }
}

/// `address symbol (file:line)`: the line `undra symbolicate` prints for a frame.
#[must_use]
pub fn format_frame(address: u64, frame: &Located) -> String {
    let symbol = frame.symbol.as_deref().unwrap_or("??");
    match (&frame.file, frame.line) {
        (Some(file), Some(line)) => format!("{address:#x} {symbol} ({file}:{line})"),
        (Some(file), None) => format!("{address:#x} {symbol} ({file})"),
        _ => format!("{address:#x} {symbol}"),
    }
}

// ---- the platform resolvers -------------------------------------------------------------------

fn tool_failed(tool: &str, output: &crate::sys::CmdOutput) -> CliError {
    CliError::new(
        Code::ToolFailed,
        format!("`{tool}` failed while resolving the addresses"),
        "it could not read the symbol file or did not accept the addresses",
        "run it by hand on the same file to see why; `undra doctor` checks the toolchain",
    )
    .with_detail(output.text())
}

/// Resolves `addresses` (virtual addresses of an ELF image) with `llvm-symbolizer` on its
/// unstripped twin.
///
/// # Errors
///
/// `C0003` when no symbolizer is installed, `C0004` when it fails.
pub fn resolve_elf(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    symbol_file: &Path,
    addresses: &[u64],
) -> Result<Vec<Vec<Located>>> {
    symbolizer(sys, toolchain, symbol_file, addresses)
}

/// Runs `llvm-symbolizer` (or `llvm-addr2line`) for `addresses` in `file`.
fn symbolizer(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    file: &Path,
    addresses: &[u64],
) -> Result<Vec<Vec<Located>>> {
    let tool = super::tools::find(sys, toolchain, &["llvm-symbolizer", "llvm-addr2line"])
        .ok_or_else(|| {
            CliError::missing_tool(
                "llvm-symbolizer",
                "resolving addresses in an ELF or wasm symbol file",
                "install the Android NDK (`sdkmanager \"ndk;27.2.12479018\"`; its LLVM tools are used) or LLVM (`brew install llvm`, `apt install llvm`)",
            )
        })?;
    let obj = format!("--obj={}", file.display());
    let hex: Vec<String> = addresses.iter().map(|a| format!("{a:#x}")).collect();
    // One frame per address: the function that contains it and the line of the instruction, as the
    // frames of a panic report are (an inlined callee shows as the line it was inlined at).
    let mut args = vec![
        obj.as_str(),
        "--demangle",
        "--no-inlines",
        "--print-address",
    ];
    args.extend(hex.iter().map(String::as_str));
    let out = sys
        .run(&tool, &args, &toolchain.env_pairs())
        .ok_or_else(|| CliError::io("run", &tool, &std::io::Error::other("it did not start")))?;
    if !out.success {
        return Err(tool_failed("llvm-symbolizer", &out));
    }
    let mut blocks = parse_symbolizer(&out.stdout);
    blocks.resize(addresses.len(), Vec::new());
    Ok(blocks)
}

/// The DWARF file of a dSYM: the bundle's `Contents/Resources/DWARF/<name>` (the one whose slices
/// include `image_id`, else the only one), or the file itself when `path` is not a bundle.
///
/// # Errors
///
/// A sentence when the bundle holds no DWARF file.
pub fn dwarf_file(path: &Path, image_id: Option<&str>) -> std::result::Result<PathBuf, String> {
    let inside = path.join("Contents/Resources/DWARF");
    if !inside.is_dir() {
        return Ok(path.to_path_buf());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&inside)
        .map_err(|e| format!("cannot read {}: {e}", inside.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    if let Some(id) = image_id {
        let found = files.iter().find(|f| {
            std::fs::read(f)
                .ok()
                .and_then(|bytes| image::macho_slices(&bytes).ok())
                .is_some_and(|slices| slices.iter().any(|s| s.uuid.as_deref() == Some(id)))
        });
        if let Some(found) = found {
            return Ok(found.clone());
        }
    }
    files
        .into_iter()
        .next()
        .ok_or_else(|| format!("{} holds no DWARF file", inside.display()))
}

/// The slice of a dSYM's DWARF file that `image_id` names (the only slice when there is one; the
/// first of `arch` when the image id is unknown).
///
/// # Errors
///
/// A sentence when the file is not Mach-O, or no slice fits.
pub fn pick_slice(
    dwarf: &Path,
    image_id: Option<&str>,
    arch: Option<&str>,
) -> std::result::Result<image::MachSlice, String> {
    let bytes =
        std::fs::read(dwarf).map_err(|e| format!("cannot read {}: {e}", dwarf.display()))?;
    let slices = image::macho_slices(&bytes)
        .map_err(|why| format!("{} is not a Mach-O file: {why}", dwarf.display()))?;
    if let Some(id) = image_id {
        if let Some(slice) = slices.iter().find(|s| s.uuid.as_deref() == Some(id)) {
            return Ok(slice.clone());
        }
        return Err(format!(
            "the dSYM {} has UUID {}, not the report's image id {id}: it is the dSYM of another build of the app",
            dwarf.display(),
            slices
                .iter()
                .filter_map(|s| s.uuid.as_deref())
                .collect::<Vec<_>>()
                .join(" or ")
        ));
    }
    match (arch, slices.as_slice()) {
        (Some(arch), _) => slices
            .iter()
            .find(|s| s.arch == arch)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "the dSYM has no {arch} slice (it has {})",
                    slices
                        .iter()
                        .map(|s| s.arch.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        (None, [only]) => Ok(only.clone()),
        (None, _) => Err(format!(
            "the dSYM has {} slices and the report names no image id: pass `--arch` or `--image-id`",
            slices.len()
        )),
    }
}

/// Resolves `addresses` (offsets from the load address of the app image) with `atos` on the
/// slice `slice` of the dSYM's DWARF file.
///
/// # Errors
///
/// `C0003` when `atos` is missing (it ships with Xcode), `C0004` when it fails.
pub fn resolve_macho(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    dwarf: &Path,
    slice: &image::MachSlice,
    addresses: &[u64],
) -> Result<Vec<Vec<Located>>> {
    if sys.os() != Os::Macos {
        return Err(CliError::new(
            Code::Unsupported,
            "Mach-O symbols are resolved with `atos`, which exists only on macOS",
            "iOS and macOS crash reports are symbolicated with Apple's tools",
            "run `undra symbolicate` on a Mac, or upload the app's dSYM to your crash reporter",
        ));
    }
    let atos = super::tools::find(sys, toolchain, &["atos"]).ok_or_else(|| {
        CliError::missing_tool(
            "atos",
            "resolving addresses in a dSYM",
            "install Xcode or the command line tools: `xcode-select --install`",
        )
    })?;
    let vmaddr = slice.text_vmaddr.unwrap_or(0);
    let dwarf_text = dwarf.display().to_string();
    let hex: Vec<String> = addresses
        .iter()
        .map(|a| format!("{:#x}", vmaddr.wrapping_add(*a)))
        .collect();
    let mut args = vec![
        "-o",
        dwarf_text.as_str(),
        "-arch",
        slice.arch.as_str(),
        "-fullPath",
    ];
    args.extend(hex.iter().map(String::as_str));
    let out = sys
        .run(&atos, &args, &toolchain.env_pairs())
        .ok_or_else(|| CliError::io("run", &atos, &std::io::Error::other("it did not start")))?;
    if !out.success {
        return Err(tool_failed("atos", &out));
    }
    let mut frames: Vec<Vec<Located>> = out
        .stdout
        .lines()
        .map(|line| {
            let located = parse_atos_line(line);
            if located == Located::default() {
                Vec::new()
            } else {
                vec![located]
            }
        })
        .collect();
    frames.resize(addresses.len(), Vec::new());
    Ok(frames)
}

/// The function index that contains module offset `offset`, from the code section of the module.
#[must_use]
pub fn function_at(module: &[u8], offset: u64) -> Option<u32> {
    wasm::function_ranges(module)
        .ok()?
        .iter()
        .find(|r| r.start <= offset && offset < r.end)
        .map(|r| r.index)
}

/// How many bytes past the start of a function body are tried for its first line-table row (the
/// local declarations come first, then the first instruction).
const BODY_PROBES: u64 = 24;

/// Resolves wasm frames: the function name from the function map (or the names section of the
/// debug module), the line where the function is defined from the DWARF module.
///
/// # Errors
///
/// `C0004` when the debug module cannot be read.
pub fn resolve_wasm(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    debug_module: &Path,
    function_map: Option<&Path>,
    dwarf_module: Option<&Path>,
    frames: &[InputFrame],
) -> Result<Vec<Vec<Located>>> {
    let bytes = std::fs::read(debug_module).map_err(|e| CliError::io("read", debug_module, &e))?;
    let mut map = function_map
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| wasm::parse_function_map(&text))
        .unwrap_or_default();
    if map.is_empty() {
        map = wasm::function_names(&bytes);
    }
    let named: Vec<(Option<u32>, Option<String>)> = frames
        .iter()
        .map(|frame| {
            let index = frame
                .function
                .or_else(|| function_at(&bytes, frame.address));
            let name = index
                .and_then(|i| wasm::function_name(&map, i))
                .map(ToOwned::to_owned);
            (index, name)
        })
        .collect();

    // The definition line of each frame's function, from the DWARF module: its function by name,
    // the first line-table row at the start of its body.
    let mut lines: Vec<Option<(String, u32)>> = vec![None; frames.len()];
    if let Some(dwarf) = dwarf_module.and_then(|p| std::fs::read(p).ok().map(|b| (p, b))) {
        let (path, dwarf_bytes) = dwarf;
        let code_start = wasm::code_section_start(&dwarf_bytes).unwrap_or(0);
        let by_name: std::collections::BTreeMap<String, u32> = wasm::function_names(&dwarf_bytes)
            .into_iter()
            .map(|(index, name)| (name, index))
            .collect();
        let ranges = wasm::function_ranges(&dwarf_bytes).unwrap_or_default();
        let mut probes: Vec<(usize, Vec<u64>)> = Vec::new();
        for (at, (_, name)) in named.iter().enumerate() {
            let Some(index) = name.as_ref().and_then(|n| by_name.get(n)) else {
                continue;
            };
            let Some(range) = ranges.iter().find(|r| r.index == *index) else {
                continue;
            };
            let first = range.body.saturating_sub(code_start);
            probes.push((at, (0..BODY_PROBES).map(|k| first + k).collect()));
        }
        let addresses: Vec<u64> = probes.iter().flat_map(|(_, a)| a.iter().copied()).collect();
        if !addresses.is_empty() {
            if let Ok(found) = symbolizer(sys, toolchain, path, &addresses) {
                let mut found = found.into_iter();
                for (at, group) in &probes {
                    let mut line = None;
                    for _ in group {
                        let frame = found.next().and_then(|f| f.into_iter().next());
                        if line.is_none() {
                            line = frame.and_then(|f| Some((f.file?, f.line?)));
                        }
                    }
                    lines[*at] = line;
                }
            }
        }
    }

    Ok(named
        .into_iter()
        .zip(lines)
        .map(|((index, name), line)| {
            let symbol = name.or_else(|| index.map(|i| format!("wasm-function[{i}]")));
            let located = Located {
                symbol,
                file: line.as_ref().map(|(file, _)| file.clone()),
                line: line.map(|(_, line)| line),
            };
            if located == Located::default() {
                Vec::new()
            } else {
                vec![located]
            }
        })
        .collect())
}

/// Demangles the names that still are Rust symbols (`_RNv…`, `_ZN…`) with `llvm-cxxfilt` (Xcode's
/// has the Rust v0 scheme), leaving a name as it is when no tool is installed or it does not
/// recognise it.
pub fn demangle(sys: &dyn Sys, toolchain: &Toolchain, frames: &mut [Vec<Located>]) {
    let mangled = |symbol: &str| symbol.starts_with("_R") || symbol.starts_with("_ZN");
    let names: Vec<String> = frames
        .iter()
        .flatten()
        .filter_map(|f| f.symbol.clone())
        .filter(|s| mangled(s))
        .collect();
    if names.is_empty() {
        return;
    }
    // Xcode's is the one that knows v0 symbols; the NDK's is older.
    let tool = if sys.os() == Os::Macos {
        super::tools::xcrun_find(sys, toolchain, "llvm-cxxfilt")
    } else {
        None
    }
    .or_else(|| super::tools::find(sys, toolchain, &["llvm-cxxfilt", "c++filt"]));
    let Some(tool) = tool else { return };
    let args: Vec<&str> = names.iter().map(String::as_str).collect();
    let Some(out) = sys
        .run(&tool, &args, &toolchain.env_pairs())
        .filter(|out| out.success)
    else {
        return;
    };
    let demangled: Vec<&str> = out.stdout.lines().collect();
    if demangled.len() != names.len() {
        return;
    }
    for frame in frames.iter_mut().flatten() {
        if let Some(symbol) = &frame.symbol {
            if let Some(at) = names.iter().position(|n| n == symbol) {
                frame.symbol = Some(demangled[at].to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest::Format;
    use super::*;

    #[test]
    fn a_report_is_read_from_the_json_the_runtimes_produce() {
        let report = parse_report(
            r#"{
              "message": "kaboom", "namespace": "playground_core", "coreVersion": "0.1.0",
              "schemaHash": "0x53241303b2d08c5e", "imageId": "DEADBEEF-0001-0203-0405-060708090A0B",
              "frames": [
                { "address": 134567, "symbol": null, "file": null, "line": null },
                { "address": "0x20e5f" },
                { "address": "wasm-function[41]:0x1a2b" },
                { "address": 7, "symbol": "wasm-function[9]" }
              ]
            }"#,
        )
        .unwrap();
        assert_eq!(report.namespace.as_deref(), Some("playground_core"));
        assert_eq!(report.core_version.as_deref(), Some("0.1.0"));
        assert_eq!(report.schema_hash, Some(0x5324_1303_b2d0_8c5e));
        assert_eq!(
            report.image_id.as_deref(),
            Some("deadbeef000102030405060708090a0b")
        );
        let frames: Vec<(u64, Option<u32>)> = report
            .frames
            .iter()
            .map(|f| (f.address, f.function))
            .collect();
        assert_eq!(
            frames,
            [
                (134_567, None),
                (0x20e5f, None),
                (0x1a2b, Some(41)),
                (7, Some(9))
            ]
        );
        // snake_case spellings and a number for the hash are accepted too; a bare array is frames.
        let report =
            parse_report(r#"{"schema_hash": 99, "image_id": "ab", "frames": []}"#).unwrap();
        assert_eq!(
            (report.schema_hash, report.image_id.as_deref()),
            (Some(99), Some("ab"))
        );
        assert_eq!(parse_report("[16, \"0x20\"]").unwrap().frames.len(), 2);
        assert!(parse_report("{}").unwrap_err().contains("frames"));
        assert!(
            parse_report(r#"{"frames": [{"address": true}]}"#)
                .unwrap_err()
                .contains("frame 0")
        );
        assert!(parse_report("nope").unwrap_err().contains("not JSON"));
    }

    #[test]
    fn addresses_come_as_hex_decimal_or_v8_positions() {
        assert_eq!(parse_address("0x10").unwrap().address, 16);
        assert_eq!(parse_address("  4096 ").unwrap().address, 4096);
        let v8 =
            parse_address("    at foo (wasm://wasm/9b2a1c3e:wasm-function[1530]:0x2ff1a)").unwrap();
        assert_eq!((v8.address, v8.function), (0x2ff1a, Some(1530)));
        assert_eq!(parse_address("explode"), None);
        assert_eq!(normalize_image_id("0xAB-cd"), Some("abcd".to_owned()));
        assert_eq!(normalize_image_id("not hex"), None);
    }

    fn entry(platform: &str, arch: &str, id: Option<&str>) -> Entry {
        Entry {
            platform: platform.to_owned(),
            namespace: "acme".to_owned(),
            core_version: "1.0.0".to_owned(),
            schema_hash: None,
            arch: arch.to_owned(),
            format: Format::Elf,
            image_id: id.map(ToOwned::to_owned),
            sha256: String::new(),
            shipped: String::new(),
            shipped_bytes: 0,
            symbols: Some("x".to_owned()),
            function_map: None,
            dwarf: None,
        }
    }

    #[test]
    fn the_image_id_picks_the_artefact_and_the_namespace_only_when_it_is_the_only_choice() {
        let manifest = Manifest {
            entries: vec![
                entry("android", "arm64-v8a", Some("aa")),
                entry("android", "x86_64", Some("bb")),
                entry("web", "wasm32", Some("cc")),
            ],
        };
        let by_id = Report {
            image_id: Some("bb".to_owned()),
            ..Report::default()
        };
        let (found, how) = select(&manifest, &by_id, None).unwrap();
        assert_eq!((found.arch.as_str(), how), ("x86_64", MatchedBy::ImageId));
        // One web artefact: the namespace is enough.
        let by_namespace = Report {
            namespace: Some("acme".to_owned()),
            ..Report::default()
        };
        let (found, how) = select(&manifest, &by_namespace, Some("web")).unwrap();
        assert_eq!(
            (found.platform.as_str(), how),
            ("web", MatchedBy::Namespace)
        );
        // Two ABIs and no image id: ambiguous, and the error lists them.
        let error = select(&manifest, &by_namespace, Some("android")).unwrap_err();
        assert!(
            error.contains("arm64-v8a") && error.contains("x86_64"),
            "{error}"
        );
        // An image id nobody has, and a namespace nobody has.
        let nobody = Report {
            image_id: Some("ff".to_owned()),
            namespace: Some("other".to_owned()),
            ..Report::default()
        };
        assert!(
            select(&manifest, &nobody, None)
                .unwrap_err()
                .contains("image id ff")
        );
    }

    #[test]
    fn manifest_paths_are_resolved_without_dot_dot() {
        assert_eq!(
            manifest_path(Path::new("/p/build/symbols"), "../host/libx.dylib.dSYM"),
            PathBuf::from("/p/build/host/libx.dylib.dSYM")
        );
        assert_eq!(
            manifest_path(Path::new("/p/build/symbols"), "android/arm64-v8a/libx.so"),
            PathBuf::from("/p/build/symbols/android/arm64-v8a/libx.so")
        );
    }

    #[test]
    fn the_symbolizers_blocks_are_read_with_their_inlined_frames() {
        let output = "0x3de78\natomic_load<u32>\n/rustc/abc/library/core/src/sync/atomic.rs:3891:24\nload\n/rustc/abc/library/core/src/sync/atomic.rs:2856:26\n\n0x10\n??\n??:0:0\n\n";
        let blocks = parse_symbolizer(output);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].len(), 2);
        assert_eq!(blocks[0][0].symbol.as_deref(), Some("atomic_load<u32>"));
        assert_eq!(
            blocks[0][0].file.as_deref(),
            Some("/rustc/abc/library/core/src/sync/atomic.rs")
        );
        assert_eq!(blocks[0][0].line, Some(3891));
        assert_eq!(blocks[0][1].symbol.as_deref(), Some("load"));
        assert_eq!(blocks[1][0], Located::default());
    }

    #[test]
    fn atos_lines_are_read_with_and_without_a_location() {
        let l =
            parse_atos_line("explode (in harness) (~/src/examples/playground/core/src/lab.rs:222)");
        assert_eq!(l.symbol.as_deref(), Some("explode"));
        assert_eq!(
            l.file.as_deref(),
            Some("~/src/examples/playground/core/src/lab.rs")
        );
        assert_eq!(l.line, Some(222));
        let l = parse_atos_line("playground_core::lab::explode::h1234 (in harness) + 24");
        assert_eq!(
            l.symbol.as_deref(),
            Some("playground_core::lab::explode::h1234")
        );
        assert_eq!((l.file, l.line), (None, None));
        assert_eq!(parse_atos_line("0x100003f20"), Located::default());
    }

    #[test]
    fn a_frame_prints_as_address_symbol_file_line() {
        let located = Located {
            symbol: Some("explode".to_owned()),
            file: Some("~/lab.rs".to_owned()),
            line: Some(222),
        };
        assert_eq!(
            format_frame(0x20e5f, &located),
            "0x20e5f explode (~/lab.rs:222)"
        );
        assert_eq!(format_frame(16, &Located::default()), "0x10 ??");
        let no_line = Located {
            symbol: Some("f".to_owned()),
            file: Some("x.rs".to_owned()),
            line: None,
        };
        assert_eq!(format_frame(1, &no_line), "0x1 f (x.rs)");
    }
}
