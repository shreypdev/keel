//! The pieces of a web build's symbol outputs that are plain wasm: removing the sections a stripped
//! module does not carry, reading the names of a module, and the function map `wasm-opt
//! --print-function-map` writes.
//!
//! One `wasm-opt` run over the module *without its DWARF* (`-Oz -g`: the names stay) makes the
//! optimised module with its names (`<ns>.debug.wasm`) and its function map; the module that ships
//! is that module with its name section removed and nothing else changed, so a `wasm-function[i]:0x…`
//! of a production stack trace names the same function and the same byte offset in both (ADR-046).
//! The DWARF cannot be in that run: binaryen skips every optimisation pass that cannot update DWARF
//! when it keeps it, and the shipped module would grow 5 to 7%. A second run that does keep it
//! (`<ns>.dwarf.wasm`) is for debuggers and for the definition lines `undra symbolicate` prints.

/// Whether a custom section is one a shipped module does not carry: the names, the DWARF, the
/// producers, and the pointers to either.
#[must_use]
pub fn is_debug_section(name: &str) -> bool {
    name == "name"
        || name.starts_with(".debug")
        || name == "producers"
        || name == "sourceMappingURL"
        || name == "external_debug_info"
}

/// A `node` script that instantiates the wasm module named by its first argument with every
/// import stubbed, runs its `_initialize` (the core's registrations) and prints
/// `undra_schema_hash()` as `0x…`: the schema hash of the core, without building it for this
/// machine (a web project has Node, and the web build runs on every change in the dev loop).
pub const SCHEMA_HASH_SCRIPT: &str = r#"
const module = new WebAssembly.Module(require("fs").readFileSync(process.argv[1]));
const imports = {};
for (const i of WebAssembly.Module.imports(module)) (imports[i.module] ??= {})[i.name] = () => 0;
WebAssembly.instantiate(module, imports).then((instance) => {
  const e = instance.exports;
  if (e._initialize) e._initialize();
  console.log("0x" + BigInt.asUintN(64, e.undra_schema_hash()).toString(16).padStart(16, "0"));
}).catch(() => process.exit(1));
"#;

/// Whether a custom section is DWARF (or points at it): what a names-only module does not carry.
#[must_use]
pub fn is_dwarf_section(name: &str) -> bool {
    name.starts_with(".debug") || name == "sourceMappingURL" || name == "external_debug_info"
}

/// Reads a LEB128 unsigned integer, returning it and the bytes it took.
fn leb_u32(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut result = 0_u32;
    for (i, byte) in bytes.iter().enumerate().take(5) {
        result |= u32::from(byte & 0x7f).checked_shl(7 * i as u32)?;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
    }
    None
}

/// One section of a module: its id, the bytes it spans (header included) and, for a custom one,
/// its name.
struct Section<'a> {
    id: u8,
    whole: &'a [u8],
    name: Option<String>,
}

/// Splits a wasm module into its sections.
fn sections(bytes: &[u8]) -> Result<Vec<Section<'_>>, String> {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err("it is not a wasm module (no \\0asm header)".to_owned());
    }
    let mut out = Vec::new();
    let mut at = 8;
    while at < bytes.len() {
        let start = at;
        let id = bytes[at];
        at += 1;
        let (size, used) = leb_u32(&bytes[at..]).ok_or("a section size is cut off")?;
        at += used;
        let end = at
            .checked_add(size as usize)
            .filter(|end| *end <= bytes.len())
            .ok_or("a section runs past the end of the module")?;
        let name = if id == 0 {
            let (len, used) = leb_u32(&bytes[at..end]).ok_or("a custom section name is cut off")?;
            let from = at + used;
            let to = from
                .checked_add(len as usize)
                .filter(|to| *to <= end)
                .ok_or("a custom section name runs past its section")?;
            Some(String::from_utf8_lossy(&bytes[from..to]).into_owned())
        } else {
            None
        };
        out.push(Section {
            id,
            whole: &bytes[start..end],
            name,
        });
        at = end;
    }
    Ok(out)
}

/// The names of the custom sections of a module, in order.
///
/// # Errors
///
/// A sentence saying why the bytes are not a readable wasm module.
#[cfg(test)]
pub fn custom_section_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    Ok(sections(bytes)?
        .into_iter()
        .filter(|s| s.id == 0)
        .filter_map(|s| s.name)
        .collect())
}

/// The module without its debug and name sections ([`is_debug_section`]); every other byte is
/// copied as it is.
///
/// # Errors
///
/// A sentence saying why the bytes are not a readable wasm module.
pub fn strip_debug_sections(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = bytes[..8.min(bytes.len())].to_vec();
    for section in sections(bytes)? {
        if section.id == 0 && section.name.as_deref().is_some_and(is_debug_section) {
            continue;
        }
        out.extend_from_slice(section.whole);
    }
    Ok(out)
}

/// The module without its DWARF sections ([`is_dwarf_section`]): the names stay, and so does every
/// other byte.
///
/// # Errors
///
/// A sentence saying why the bytes are not a readable wasm module.
pub fn strip_dwarf_sections(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = bytes[..8.min(bytes.len())].to_vec();
    for section in sections(bytes)? {
        if section.id == 0 && section.name.as_deref().is_some_and(is_dwarf_section) {
            continue;
        }
        out.extend_from_slice(section.whole);
    }
    Ok(out)
}

/// The function names of a module's `name` section, as `(index, name)`; empty when it has none.
#[must_use]
pub fn function_names(bytes: &[u8]) -> Vec<(u32, String)> {
    let Ok(all) = sections(bytes) else {
        return Vec::new();
    };
    let Some(names) = all
        .iter()
        .find(|s| s.id == 0 && s.name.as_deref() == Some("name"))
    else {
        return Vec::new();
    };
    // Past the section header, and the section's own name.
    let Some((_, size_len)) = leb_u32(&names.whole[1..]) else {
        return Vec::new();
    };
    let payload = &names.whole[1 + size_len..];
    let Some((name_len, used)) = leb_u32(payload) else {
        return Vec::new();
    };
    let mut at = used + name_len as usize;
    let mut out = Vec::new();
    while at < payload.len() {
        let id = payload[at];
        let Some((size, used)) = payload.get(at + 1..).and_then(leb_u32) else {
            break;
        };
        let body_start = at + 1 + used;
        let body_end = body_start + size as usize;
        if id == 1 {
            let Some(body) = payload.get(body_start..body_end) else {
                break;
            };
            let Some((count, mut p)) = leb_u32(body) else {
                break;
            };
            for _ in 0..count {
                let Some((index, used)) = body.get(p..).and_then(leb_u32) else {
                    break;
                };
                p += used;
                let Some((len, used)) = body.get(p..).and_then(leb_u32) else {
                    break;
                };
                p += used;
                let Some(raw) = body.get(p..p + len as usize) else {
                    break;
                };
                p += len as usize;
                out.push((index, String::from_utf8_lossy(raw).into_owned()));
            }
        }
        at = body_end;
    }
    out
}

/// Where one function of a module is: its index in the function index space (imports first) and
/// the byte range of its body, counted from the start of the module like the `0x…` of a
/// `wasm-function[i]:0x…` stack line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionRange {
    /// The function index.
    pub index: u32,
    /// The offset of the body's size field.
    pub start: u64,
    /// The offset of the body itself (its local declarations), just past the size field: what the
    /// DWARF of the module calls the function's address.
    pub body: u64,
    /// The offset just past the body.
    pub end: u64,
}

/// The offset of the code section's contents (the function count): what the line tables of the
/// module's DWARF count their addresses from.
#[must_use]
pub fn code_section_start(bytes: &[u8]) -> Option<u64> {
    let mut at = 8;
    while at < bytes.len() {
        let id = *bytes.get(at)?;
        let (size, used) = leb_u32(bytes.get(at + 1..)?)?;
        let contents = at + 1 + used;
        if id == 10 {
            return Some(contents as u64);
        }
        at = contents.checked_add(size as usize)?;
    }
    None
}

/// The number of functions the module imports (they come first in the function index space).
fn imported_functions(section: &[u8]) -> Option<u32> {
    let skip_limits = |at: &mut usize| -> Option<()> {
        let (flags, used) = leb_u32(section.get(*at..)?)?;
        *at += used;
        let (_, used) = leb_u32(section.get(*at..)?)?;
        *at += used;
        if flags & 1 == 1 {
            let (_, used) = leb_u32(section.get(*at..)?)?;
            *at += used;
        }
        Some(())
    };
    let (count, mut at) = leb_u32(section)?;
    let mut functions = 0;
    for _ in 0..count {
        for _ in 0..2 {
            let (len, used) = leb_u32(section.get(at..)?)?;
            at += used + len as usize;
        }
        let kind = *section.get(at)?;
        at += 1;
        match kind {
            0 => {
                functions += 1;
                let (_, used) = leb_u32(section.get(at..)?)?;
                at += used;
            }
            1 => {
                at += 1; // the element type
                skip_limits(&mut at)?;
            }
            2 => skip_limits(&mut at)?,
            3 => at += 2, // value type and mutability
            4 => {
                at += 1; // the attribute
                let (_, used) = leb_u32(section.get(at..)?)?;
                at += used;
            }
            _ => return None,
        }
    }
    Some(functions)
}

/// The byte ranges of the function bodies of a module, in index order.
///
/// # Errors
///
/// A sentence saying why the bytes are not a module this function reads.
pub fn function_ranges(bytes: &[u8]) -> Result<Vec<FunctionRange>, String> {
    let mut imported = 0;
    let mut out = Vec::new();
    for section in sections(bytes)? {
        let contents = |whole: &[u8]| -> usize {
            let (_, used) = leb_u32(&whole[1..]).unwrap_or((0, 1));
            1 + used
        };
        match section.id {
            2 => {
                let offset = contents(section.whole);
                imported = imported_functions(&section.whole[offset..])
                    .ok_or("the import section is not readable")?;
            }
            10 => {
                let offset = contents(section.whole);
                let base = section.whole.as_ptr() as usize - bytes.as_ptr() as usize;
                let payload = &section.whole[offset..];
                let (count, mut at) = leb_u32(payload).ok_or("the code section is cut off")?;
                for i in 0..count {
                    let (size, used) =
                        leb_u32(payload.get(at..).ok_or("a function body is cut off")?)
                            .ok_or("a function body size is cut off")?;
                    let start = (base + offset + at) as u64;
                    at += used + size as usize;
                    out.push(FunctionRange {
                        index: imported + i,
                        start,
                        body: start + used as u64,
                        end: (base + offset + at) as u64,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The function names of `wasm-opt --print-function-map`: one `index:name` per line.
#[must_use]
pub fn parse_function_map(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter_map(|line| {
            let (index, name) = line.split_once(':')?;
            Some((index.trim().parse().ok()?, name.trim().to_owned()))
        })
        .collect()
}

/// The name of function `index` in a function map, if it has one.
#[must_use]
pub fn function_name(map: &[(u32, String)], index: u32) -> Option<&str> {
    map.iter()
        .find(|(i, _)| *i == index)
        .map(|(_, name)| name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(id: u8, payload: &[u8]) -> Vec<u8> {
        let mut s = vec![id, u8::try_from(payload.len()).unwrap()];
        s.extend_from_slice(payload);
        s
    }

    fn custom(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut body = vec![u8::try_from(name.len()).unwrap()];
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(payload);
        section(0, &body)
    }

    fn module(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        for p in parts {
            m.extend_from_slice(p);
        }
        m
    }

    #[test]
    fn debug_and_name_sections_go_and_everything_else_stays_byte_for_byte() {
        let kept = [
            section(1, &[0x01, 0x60, 0x00, 0x00]),
            custom(
                "target_features",
                &[0x01, 0x2b, 0x04, b'a', b'b', b'c', b'd'],
            ),
            section(10, &[0x01, 0x02, 0x00, 0x0b]),
        ];
        let full = module(&[
            kept[0].clone(),
            custom(".debug_line", &[1, 2, 3, 4, 5]),
            kept[1].clone(),
            kept[2].clone(),
            custom("name", &[0, 1, 2]),
            custom("producers", &[0]),
            custom("sourceMappingURL", &[0]),
        ]);
        assert_eq!(
            custom_section_names(&full).unwrap(),
            [
                ".debug_line",
                "target_features",
                "name",
                "producers",
                "sourceMappingURL"
            ]
        );
        let stripped = strip_debug_sections(&full).unwrap();
        assert_eq!(
            stripped,
            module(&[kept[0].clone(), kept[1].clone(), kept[2].clone()])
        );
        assert_eq!(
            custom_section_names(&stripped).unwrap(),
            ["target_features"]
        );
        // Stripping a stripped module changes nothing.
        assert_eq!(strip_debug_sections(&stripped).unwrap(), stripped);
    }

    #[test]
    fn dwarf_goes_and_the_names_stay_for_the_run_that_must_not_keep_dwarf() {
        let names = {
            // The name section: one function name subsection, `1 entry: index 3 "explode"`.
            let mut body = vec![4, b'n', b'a', b'm', b'e', 1];
            let entry: Vec<u8> = [vec![1, 3, 7], b"explode".to_vec()].concat();
            body.push(u8::try_from(entry.len()).unwrap());
            body.extend_from_slice(&entry);
            section(0, &body)
        };
        let full = module(&[
            section(1, &[0x01, 0x60, 0x00, 0x00]),
            custom(".debug_line", &[1, 2, 3]),
            names.clone(),
            custom("producers", &[0]),
        ]);
        let stripped = strip_dwarf_sections(&full).unwrap();
        assert_eq!(
            custom_section_names(&stripped).unwrap(),
            ["name", "producers"]
        );
        assert_eq!(function_names(&stripped), [(3, "explode".to_owned())]);
        assert!(function_names(&module(&[section(1, &[0])])).is_empty());
    }

    #[test]
    fn what_is_not_a_module_is_an_error_not_a_panic() {
        assert!(strip_debug_sections(b"").is_err());
        assert!(strip_debug_sections(b"\x7fELF\x02\x01\x01\0").is_err());
        let mut cut = module(&[section(1, &[1, 2, 3, 4])]);
        cut.truncate(cut.len() - 2);
        assert!(strip_debug_sections(&cut).is_err());
    }

    #[test]
    fn function_bodies_are_located_by_module_offset_after_the_imports() {
        // One imported function (type 0), two defined ones: `nop; end` (3 bytes) and `end` (1 byte).
        let import = {
            let mut body = vec![1, 1, b'u', 1, b'f', 0, 0];
            body.insert(0, 0);
            body.remove(0);
            section(2, &body)
        };
        let code = section(10, &[2, 3, 0, 0x01, 0x0b, 1, 0x0b]);
        let m = module(&[
            section(1, &[1, 0x60, 0, 0]),
            import,
            section(3, &[2, 0, 0]),
            code,
        ]);
        let ranges = function_ranges(&m).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].index, 1, "the import is function 0");
        assert_eq!(ranges[1].index, 2);
        assert_eq!(
            ranges[0].end - ranges[0].start,
            4,
            "the size byte and a body of 3"
        );
        assert_eq!(ranges[0].end, ranges[1].start);
        assert_eq!(
            ranges[0].body,
            ranges[0].start + 1,
            "past the one-byte size"
        );
        // The code section's contents start at its function count.
        let start = code_section_start(&m).unwrap();
        assert_eq!(m[start as usize], 2);
        assert_eq!(ranges[0].start, start + 1);
    }

    #[test]
    fn the_function_map_maps_indexes_to_names() {
        let map = parse_function_map(
            "0:__wasm_call_ctors\n17:_ZN4core3fmt5write17h0123E\nnot a line\n41:explode\n",
        );
        assert_eq!(map.len(), 3);
        assert_eq!(function_name(&map, 41), Some("explode"));
        assert_eq!(function_name(&map, 17), Some("_ZN4core3fmt5write17h0123E"));
        assert_eq!(function_name(&map, 3), None);
    }
}
