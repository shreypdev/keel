//! The identity of an executable image: what a crash report names the image it crashed in, read
//! from the file the symbols belong to.
//!
//! * a **Mach-O** image (an app executable, a dylib, a dSYM's DWARF file) is named by its
//!   `LC_UUID`; a fat file has one per slice. The `__TEXT` segment's `vmaddr` is what an
//!   image-relative offset is added to before a symbolicator looks it up;
//! * an **ELF** image (an Android `.so`) is named by its GNU build id (`NT_GNU_BUILD_ID`);
//! * a **wasm** module is named by the SHA-256 of its bytes ([`super::sha256`]).
//!
//! Both identities are written as lowercase hex without separators, the form a `PanicReport`'s
//! `image_id` has (ADR-046).

/// One slice of a Mach-O file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachSlice {
    /// The architecture name `atos -arch` takes (`arm64`, `x86_64`), or `cpu-<hex>` for another.
    pub arch: String,
    /// The `LC_UUID`, 32 lowercase hex digits; `None` for an object file, which has none.
    pub uuid: Option<String>,
    /// The `vmaddr` of the `__TEXT` segment; `None` for an object file, which has no segments by name.
    pub text_vmaddr: Option<u64>,
}

const MH_MAGIC_64: u32 = 0xfeed_facf;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const LC_SEGMENT_64: u32 = 0x19;
const LC_UUID: u32 = 0x1b;
const CPU_ARM64: u32 = 0x0100_000c;
const CPU_X86_64: u32 = 0x0100_0007;

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    bytes
        .get(at..at + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be64(bytes: &[u8], at: usize) -> Option<u64> {
    bytes
        .get(at..at + 8)
        .map(|b| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// The slices of a Mach-O file: one for a thin 64-bit file, one per architecture of a fat file.
///
/// # Errors
///
/// A sentence saying why the bytes are not a Mach-O file this function reads (32-bit and
/// big-endian Mach-O files are not read: no Apple platform Undra builds for uses them).
pub fn macho_slices(bytes: &[u8]) -> Result<Vec<MachSlice>, String> {
    match be32(bytes, 0) {
        Some(FAT_MAGIC | FAT_MAGIC_64) => {
            let wide = be32(bytes, 0) == Some(FAT_MAGIC_64);
            let count = be32(bytes, 4).ok_or("the fat header is cut off")?;
            let stride = if wide { 32 } else { 20 };
            let mut out = Vec::new();
            for i in 0..count as usize {
                let at = 8 + i * stride;
                let (offset, size) = if wide {
                    (be64(bytes, at + 8), be64(bytes, at + 16))
                } else {
                    (
                        be32(bytes, at + 8).map(u64::from),
                        be32(bytes, at + 12).map(u64::from),
                    )
                };
                let (offset, size) = offset.zip(size).ok_or("a fat_arch entry is cut off")?;
                let start = usize::try_from(offset).map_err(|_| "a slice offset is too large")?;
                let end = start
                    .checked_add(usize::try_from(size).map_err(|_| "a slice is too large")?)
                    .filter(|end| *end <= bytes.len())
                    .ok_or("a slice runs past the end of the file")?;
                out.push(thin_slice(&bytes[start..end])?);
            }
            Ok(out)
        }
        _ => Ok(vec![thin_slice(bytes)?]),
    }
}

/// The slice a thin 64-bit little-endian Mach-O file is.
fn thin_slice(bytes: &[u8]) -> Result<MachSlice, String> {
    if le32(bytes, 0) != Some(MH_MAGIC_64) {
        return Err("it is not a 64-bit little-endian Mach-O file".to_owned());
    }
    let cpu = le32(bytes, 4).ok_or("the Mach-O header is cut off")?;
    let count = le32(bytes, 16).ok_or("the Mach-O header is cut off")?;
    let arch = match cpu {
        CPU_ARM64 => "arm64".to_owned(),
        CPU_X86_64 => "x86_64".to_owned(),
        other => format!("cpu-{other:x}"),
    };
    let mut uuid = None;
    let mut text_vmaddr = None;
    let mut at = 32;
    for _ in 0..count {
        let command = le32(bytes, at).ok_or("a load command is cut off")?;
        let size = le32(bytes, at + 4).ok_or("a load command is cut off")? as usize;
        if size < 8 {
            return Err("a load command has no size".to_owned());
        }
        match command {
            LC_UUID => {
                let raw = bytes
                    .get(at + 8..at + 24)
                    .ok_or("the LC_UUID command is cut off")?;
                uuid = Some(raw.iter().map(|b| format!("{b:02x}")).collect::<String>());
            }
            LC_SEGMENT_64 => {
                let name = bytes
                    .get(at + 8..at + 24)
                    .ok_or("a segment command is cut off")?;
                if name.starts_with(b"__TEXT\0") {
                    text_vmaddr = le64(bytes, at + 24);
                }
            }
            _ => {}
        }
        at += size;
    }
    Ok(MachSlice {
        arch,
        uuid,
        text_vmaddr,
    })
}

/// The GNU build id of an ELF file (64-bit little-endian), as lowercase hex; `None` when the file
/// is not one this function reads or carries no `NT_GNU_BUILD_ID` note.
///
/// Read from the `PT_NOTE` segments, which a stripped library keeps.
#[must_use]
pub fn elf_build_id(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
        return None;
    }
    let phoff = usize::try_from(le64(bytes, 0x20)?).ok()?;
    let phentsize = usize::from(u16::from_le_bytes([*bytes.get(0x36)?, *bytes.get(0x37)?]));
    let phnum = usize::from(u16::from_le_bytes([*bytes.get(0x38)?, *bytes.get(0x39)?]));
    for i in 0..phnum {
        let header = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if le32(bytes, header)? != 4 {
            continue; // not PT_NOTE
        }
        let offset = usize::try_from(le64(bytes, header + 8)?).ok()?;
        let size = usize::try_from(le64(bytes, header + 32)?).ok()?;
        let notes = bytes.get(offset..offset.checked_add(size)?)?;
        if let Some(id) = gnu_build_id(notes) {
            return Some(id);
        }
    }
    None
}

/// The `NT_GNU_BUILD_ID` descriptor among the notes of one `PT_NOTE` segment.
fn gnu_build_id(notes: &[u8]) -> Option<String> {
    let align4 = |n: usize| (n + 3) & !3;
    let mut at = 0;
    while at + 12 <= notes.len() {
        let name_size = le32(notes, at)? as usize;
        let desc_size = le32(notes, at + 4)? as usize;
        let kind = le32(notes, at + 8)?;
        let name_at = at + 12;
        let desc_at = name_at.checked_add(align4(name_size))?;
        let next = desc_at.checked_add(align4(desc_size))?;
        if kind == 3 && notes.get(name_at..name_at + name_size) == Some(b"GNU\0") {
            let id = notes.get(desc_at..desc_at.checked_add(desc_size)?)?;
            return Some(id.iter().map(|b| format!("{b:02x}")).collect());
        }
        at = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thin arm64 Mach-O with the given load commands.
    fn thin(commands: &[Vec<u8>]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
        b.extend_from_slice(&CPU_ARM64.to_le_bytes());
        b.extend_from_slice(&[0; 8]); // cpusubtype, filetype
        b.extend_from_slice(&(commands.len() as u32).to_le_bytes());
        b.extend_from_slice(&[0; 12]); // sizeofcmds, flags, reserved
        for c in commands {
            b.extend_from_slice(c);
        }
        b
    }

    fn uuid_command(uuid: [u8; 16]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&LC_UUID.to_le_bytes());
        c.extend_from_slice(&24_u32.to_le_bytes());
        c.extend_from_slice(&uuid);
        c
    }

    fn segment_command(name: &[u8], vmaddr: u64) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
        c.extend_from_slice(&72_u32.to_le_bytes());
        let mut padded = name.to_vec();
        padded.resize(16, 0);
        c.extend_from_slice(&padded);
        c.extend_from_slice(&vmaddr.to_le_bytes());
        c.resize(72, 0);
        c
    }

    #[test]
    fn a_thin_file_has_its_uuid_and_text_address() {
        let uuid = [
            0xde, 0xad, 0xbe, 0xef, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0xff,
        ];
        let file = thin(&[
            segment_command(b"__PAGEZERO", 0),
            segment_command(b"__TEXT", 0x1_0000_0000),
            uuid_command(uuid),
        ]);
        let slices = macho_slices(&file).unwrap();
        assert_eq!(
            slices,
            [MachSlice {
                arch: "arm64".to_owned(),
                uuid: Some("deadbeef000102030405060708090aff".to_owned()),
                text_vmaddr: Some(0x1_0000_0000),
            }]
        );
    }

    #[test]
    fn a_fat_file_has_a_slice_per_architecture() {
        let a = thin(&[uuid_command([1; 16]), segment_command(b"__TEXT", 0x4000)]);
        let mut b = thin(&[uuid_command([2; 16]), segment_command(b"__TEXT", 0x8000)]);
        b[4..8].copy_from_slice(&CPU_X86_64.to_le_bytes());
        let first = 8 + 2 * 20;
        let second = first + a.len();
        let mut file = Vec::new();
        file.extend_from_slice(&FAT_MAGIC.to_be_bytes());
        file.extend_from_slice(&2_u32.to_be_bytes());
        for (cpu, offset, size) in [(CPU_ARM64, first, a.len()), (CPU_X86_64, second, b.len())] {
            file.extend_from_slice(&cpu.to_be_bytes());
            file.extend_from_slice(&0_u32.to_be_bytes());
            file.extend_from_slice(&(offset as u32).to_be_bytes());
            file.extend_from_slice(&(size as u32).to_be_bytes());
            file.extend_from_slice(&0_u32.to_be_bytes());
        }
        file.extend_from_slice(&a);
        file.extend_from_slice(&b);
        let slices = macho_slices(&file).unwrap();
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].arch, "arm64");
        assert_eq!(slices[0].uuid.as_deref(), Some("01".repeat(16).as_str()));
        assert_eq!(slices[1].arch, "x86_64");
        assert_eq!(slices[1].text_vmaddr, Some(0x8000));
    }

    #[test]
    fn what_is_not_macho_is_an_error_not_a_panic() {
        assert!(macho_slices(b"").is_err());
        assert!(macho_slices(b"\x7fELF....").is_err());
        let mut cut = thin(&[uuid_command([3; 16])]);
        cut.truncate(40);
        assert!(macho_slices(&cut).is_err());
    }

    /// An ELF64 with one PT_NOTE segment holding `notes`.
    fn elf_with_notes(notes: &[u8]) -> Vec<u8> {
        let mut b = vec![0_u8; 64 + 56];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[0x20..0x28].copy_from_slice(&64_u64.to_le_bytes());
        b[0x36..0x38].copy_from_slice(&56_u16.to_le_bytes());
        b[0x38..0x3a].copy_from_slice(&1_u16.to_le_bytes());
        b[64..68].copy_from_slice(&4_u32.to_le_bytes());
        b[64 + 8..64 + 16].copy_from_slice(&120_u64.to_le_bytes());
        b[64 + 32..64 + 40].copy_from_slice(&(notes.len() as u64).to_le_bytes());
        b.extend_from_slice(notes);
        b
    }

    fn note(name: &[u8], kind: u32, desc: &[u8]) -> Vec<u8> {
        let mut n = Vec::new();
        n.extend_from_slice(&(name.len() as u32).to_le_bytes());
        n.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        n.extend_from_slice(&kind.to_le_bytes());
        n.extend_from_slice(name);
        n.resize(n.len().div_ceil(4) * 4, 0);
        n.extend_from_slice(desc);
        n.resize(n.len().div_ceil(4) * 4, 0);
        n
    }

    #[test]
    fn the_gnu_build_id_is_found_among_other_notes() {
        let mut notes = note(b"Android\0", 1, &[1, 2, 3, 4, 5, 6]);
        notes.extend(note(b"GNU\0", 3, &[0xab, 0xcd, 0x01, 0x23, 0x45]));
        assert_eq!(
            elf_build_id(&elf_with_notes(&notes)).as_deref(),
            Some("abcd012345")
        );
        // A GNU note of another kind, or no notes at all: no id.
        assert_eq!(
            elf_build_id(&elf_with_notes(&note(b"GNU\0", 5, &[1, 2, 3, 4]))),
            None
        );
        assert_eq!(
            elf_build_id(b"not an elf at all, not even close......"),
            None
        );
        assert_eq!(elf_build_id(&elf_with_notes(&[])), None);
    }
}
