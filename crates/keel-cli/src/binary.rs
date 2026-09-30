//! Looking inside the artifacts the build produced.
//!
//! Two checks catch the mistakes that otherwise show up as a blank screen on a device:
//!
//! * a **wasm module** must export the `keel_*` functions of docs/SPEC.md 7 and `_initialize`
//!   (which runs the `#[keel::api]` registrations), or the TypeScript runtime cannot start it;
//! * an **Android library** for a 64-bit ABI must have its `LOAD` segments aligned to 16 KB,
//!   or Google Play refuses the app and Android 15+ devices with 16 KB pages cannot load it.

/// The exports a Keel wasm core must have (SPEC 7; `_initialize` is checked separately).
pub const REQUIRED_WASM_EXPORTS: &[&str] = &[
    "memory",
    "keel_alloc",
    "keel_free",
    "keel_abi_version",
    "keel_schema_hash",
    "keel_schema_json",
    "keel_init",
    "keel_call",
    "keel_call_sync",
    "keel_poll",
    "keel_buf_free",
];

/// Reads a LEB128 unsigned integer, returning it and the bytes used.
fn leb_u32(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut result = 0_u32;
    let mut shift = 0;
    for (i, byte) in bytes.iter().enumerate().take(5) {
        result |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

/// The names of the exports of a wasm module, read from its export section.
///
/// # Errors
///
/// A description of why the bytes are not a wasm module this function can read.
pub fn wasm_exports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err("it is not a wasm module (no \\0asm header)".to_owned());
    }
    let mut pos = 8;
    while pos < bytes.len() {
        let id = bytes[pos];
        pos += 1;
        let (size, used) = leb_u32(&bytes[pos..]).ok_or("a section size is cut off")?;
        pos += used;
        let end = pos.checked_add(size as usize).filter(|e| *e <= bytes.len()).ok_or("a section runs past the end")?;
        if id == 7 {
            let section = &bytes[pos..end];
            let (count, mut at) = leb_u32(section).ok_or("the export count is cut off")?;
            let mut names = Vec::new();
            for _ in 0..count {
                let (len, used) = leb_u32(&section[at..]).ok_or("an export name is cut off")?;
                at += used;
                let name_end = at.checked_add(len as usize).filter(|e| *e <= section.len()).ok_or("an export name runs past the section")?;
                names.push(String::from_utf8_lossy(&section[at..name_end]).into_owned());
                at = name_end;
                at += 1; // export kind
                let (_, used) = leb_u32(section.get(at..).unwrap_or_default()).ok_or("an export index is cut off")?;
                at += used;
            }
            return Ok(names);
        }
        pos = end;
    }
    Ok(Vec::new())
}

/// What is wrong with the exports of a wasm core, in words; empty when it is fine.
#[must_use]
pub fn wasm_problems(exports: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let missing: Vec<&str> = REQUIRED_WASM_EXPORTS
        .iter()
        .copied()
        .filter(|name| !exports.iter().any(|e| e == name))
        .collect();
    if !missing.is_empty() {
        problems.push(format!(
            "it does not export {}: the TypeScript runtime cannot start a core without them (was keel-ffi linked?)",
            missing.join(", ")
        ));
    }
    if !exports.iter().any(|e| e == "_initialize") {
        problems.push(
            "it does not export `_initialize`, so the `#[keel::api]` registrations never run and the schema would be empty"
                .to_owned(),
        );
    }
    problems
}

/// The smallest `p_align` among the `PT_LOAD` segments of an ELF file, or `None` when the bytes
/// are not a little-endian ELF this function reads.
#[must_use]
pub fn elf_min_load_alignment(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[5] != 1 {
        return None;
    }
    let is64 = match bytes[4] {
        1 => false,
        2 => true,
        _ => return None,
    };
    let u16_at = |o: usize| bytes.get(o..o + 2).map(|b| u64::from(u16::from_le_bytes([b[0], b[1]])));
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])));
    let u64_at = |o: usize| {
        bytes.get(o..o + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    };
    let (phoff, phentsize, phnum) = if is64 {
        (u64_at(0x20)?, u16_at(0x36)?, u16_at(0x38)?)
    } else {
        (u32_at(0x1c)?, u16_at(0x2a)?, u16_at(0x2c)?)
    };
    let mut min: Option<u64> = None;
    for i in 0..phnum {
        let base = usize::try_from(phoff + i * phentsize).ok()?;
        if u32_at(base)? != 1 {
            continue; // not PT_LOAD
        }
        let align = if is64 { u64_at(base + 0x30)? } else { u32_at(base + 0x1c)? };
        min = Some(min.map_or(align, |m| m.min(align)));
    }
    min
}

/// Whether an ELF file is 64-bit.
#[must_use]
pub fn elf_is_64(bytes: &[u8]) -> bool {
    bytes.len() > 4 && &bytes[..4] == b"\x7fELF" && bytes[4] == 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal wasm module with the given exports (all functions with index 0).
    fn module(exports: &[&str]) -> Vec<u8> {
        let mut section = vec![u8::try_from(exports.len()).unwrap()];
        for name in exports {
            section.push(u8::try_from(name.len()).unwrap());
            section.extend_from_slice(name.as_bytes());
            section.extend_from_slice(&[0, 0]); // kind func, index 0
        }
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        bytes.extend_from_slice(&[1, 1, 0]); // a (bogus, skipped) type section of size 1
        bytes.push(7);
        bytes.push(u8::try_from(section.len()).unwrap());
        bytes.extend_from_slice(&section);
        bytes
    }

    #[test]
    fn reads_export_names() {
        let names = wasm_exports(&module(&["memory", "keel_init", "_initialize"])).unwrap();
        assert_eq!(names, ["memory", "keel_init", "_initialize"]);
    }

    #[test]
    fn malformed_modules_are_errors_not_panics() {
        assert!(wasm_exports(b"").is_err());
        assert!(wasm_exports(b"\x7fELF....").is_err());
        let mut truncated = module(&["keel_init"]);
        truncated.truncate(truncated.len() - 3);
        assert!(wasm_exports(&truncated).is_err());
        // No export section at all is fine: no exports.
        assert_eq!(wasm_exports(b"\0asm\x01\0\0\0").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn problems_name_what_is_missing() {
        let all: Vec<String> = REQUIRED_WASM_EXPORTS.iter().map(|s| (*s).to_owned()).chain(["_initialize".to_owned()]).collect();
        assert!(wasm_problems(&all).is_empty());
        let mut without = all.clone();
        without.retain(|e| e != "keel_init" && e != "_initialize");
        let problems = wasm_problems(&without);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("keel_init"), "{problems:?}");
        assert!(problems[1].contains("_initialize"), "{problems:?}");
    }

    /// An ELF64 header with two PT_LOAD segments and one PT_NOTE.
    fn elf64(aligns: [u64; 2]) -> Vec<u8> {
        let mut b = vec![0_u8; 64 + 3 * 56];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[0x20..0x28].copy_from_slice(&64_u64.to_le_bytes());
        b[0x36..0x38].copy_from_slice(&56_u16.to_le_bytes());
        b[0x38..0x3a].copy_from_slice(&3_u16.to_le_bytes());
        for (i, (kind, align)) in [(1_u32, aligns[0]), (4, 8), (1, aligns[1])].into_iter().enumerate() {
            let base = 64 + i * 56;
            b[base..base + 4].copy_from_slice(&kind.to_le_bytes());
            b[base + 0x30..base + 0x38].copy_from_slice(&align.to_le_bytes());
        }
        b
    }

    #[test]
    fn finds_the_load_alignment() {
        assert_eq!(elf_min_load_alignment(&elf64([0x4000, 0x10000])), Some(0x4000));
        assert_eq!(elf_min_load_alignment(&elf64([0x1000, 0x4000])), Some(0x1000));
        assert!(elf_is_64(&elf64([0x4000, 0x4000])));
        assert_eq!(elf_min_load_alignment(b"not elf"), None);
        assert_eq!(elf_min_load_alignment(&[0x7f, b'E', b'L', b'F', 2, 2]), None, "big endian is not read");
    }
}
