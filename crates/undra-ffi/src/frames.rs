//! The frames of a panic report and the identity of the image they belong to (ADR-046 decision 4.1).
//!
//! `undra-runtime` builds the report of a contained panic and may not use `unsafe`; what needs it
//! is reading the stack and the loaded image, so this crate installs a
//! [`FrameSource`](undra_runtime::FrameSource) when the core starts:
//!
//! * [`capture`](undra_runtime::FrameSource::capture) walks the stack with the platform's unwinder
//!   (`_Unwind_Backtrace`, the one `std::backtrace` uses, so the symbol is always linked) and
//!   keeps the frames inside the core's own image, each as an offset from the image's load
//!   address: an address that does not depend on where the loader put the image, which a
//!   symbolicator resolves against the symbol files of `undra build --release`. The address points
//!   into the call instruction (the return address less one), as symbolizers expect.
//! * [`image_id`](undra_runtime::FrameSource::image_id) reads the image's identity from its own
//!   headers in memory: the Mach-O `LC_UUID` on Apple platforms, the GNU build id (`PT_NOTE`, type
//!   3) of an ELF image on Android and Linux (`undra build` links Android libraries with
//!   `--build-id`). Crash reporters key their symbol files by this id.
//!
//! "The core's image" is the one that contains this function: the app executable on iOS (the core
//! is prelinked into it), `lib<namespace>.so` on Android, the dynamic library on the host.

use core::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

use undra_runtime::FrameSource;

/// `struct _Unwind_Context` is opaque.
type UnwindContext = c_void;

/// `_Unwind_Reason_Code`: `_URC_NO_REASON` (continue) is 0, `_URC_END_OF_STACK` 5.
type TraceFn = extern "C" fn(*mut UnwindContext, *mut c_void) -> c_int;

/// `Dl_info` of `dladdr`.
#[repr(C)]
struct DlInfo {
    dli_fname: *const c_char,
    dli_fbase: *mut c_void,
    dli_sname: *const c_char,
    dli_saddr: *mut c_void,
}

// SAFETY: these are the platform unwinder's and loader's own entry points, declared with the
// signatures of `<unwind.h>` and `<dlfcn.h>`; both are in libunwind/libgcc_s and libc/libdl, which
// every Rust program on these targets links (`std::backtrace` uses the same ones).
unsafe extern "C" {
    fn _Unwind_Backtrace(trace: TraceFn, arg: *mut c_void) -> c_int;
    fn _Unwind_GetIPInfo(context: *mut UnwindContext, ip_before_insn: *mut c_int) -> usize;
    fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
}

/// The base address of the image containing `address`, if the loader knows it.
fn image_base(address: usize) -> Option<usize> {
    let mut info = DlInfo {
        dli_fname: core::ptr::null(),
        dli_fbase: core::ptr::null_mut(),
        dli_sname: core::ptr::null(),
        dli_saddr: core::ptr::null_mut(),
    };
    // SAFETY: `info` is a valid `Dl_info` for `dladdr` to fill; `address` is only looked up, never
    // dereferenced.
    let found = unsafe { dladdr(address as *const c_void, &mut info) };
    (found != 0 && !info.dli_fbase.is_null()).then_some(info.dli_fbase as usize)
}

/// The base of the image this code is in.
fn core_base() -> Option<usize> {
    static BASE: OnceLock<Option<usize>> = OnceLock::new();
    *BASE.get_or_init(|| image_base(core_base as fn() -> Option<usize> as usize))
}

struct Walk {
    base: usize,
    max: usize,
    out: Vec<u64>,
}

extern "C" fn visit(context: *mut UnwindContext, arg: *mut c_void) -> c_int {
    // SAFETY: `arg` is the `&mut Walk` that `capture` passed to `_Unwind_Backtrace`, which calls
    // this function synchronously, so the reference is live and unaliased.
    let walk = unsafe { &mut *arg.cast::<Walk>() };
    let mut before = 0;
    // SAFETY: `context` is the unwinder's context for this frame, valid for the call.
    let ip = unsafe { _Unwind_GetIPInfo(context, &mut before) };
    if ip == 0 {
        return 0;
    }
    // A return address points after the call: step back into the call instruction.
    let ip = if before == 0 { ip - 1 } else { ip };
    if image_base(ip) == Some(walk.base) {
        // (`dladdr` gives the base of the image containing `ip`, so `ip >= base`; checked anyway:
        // nothing in this callback may panic, an unwind out of it would abort.)
        if let Some(offset) = ip.checked_sub(walk.base) {
            walk.out.push(offset as u64);
        }
    }
    if walk.out.len() >= walk.max { 5 } else { 0 }
}

/// Reads the stack and the image of this process for `undra-runtime`'s panic reports.
struct Source;

impl FrameSource for Source {
    fn capture(&self, max: usize) -> Vec<u64> {
        let Some(base) = core_base() else {
            return Vec::new();
        };
        let mut walk = Walk {
            base,
            max,
            out: Vec::with_capacity(max.min(64)),
        };
        // SAFETY: `visit` only reads the unwinder context it is given and writes through `arg`,
        // which points at `walk` for exactly the duration of this call.
        unsafe { _Unwind_Backtrace(visit, (&raw mut walk).cast()) };
        walk.out
    }

    fn image_id(&self) -> String {
        core_base().map_or_else(String::new, |base| {
            // SAFETY: `base` is the load address of the image this code is running from: its
            // headers are mapped and readable for as long as the image is loaded.
            unsafe { read_image_id(base) }
        })
    }
}

static SOURCE: Source = Source;

/// Installs the source for this image's runtime (once per process; later calls do nothing).
pub(crate) fn install() {
    undra_runtime::install_frame_source(&SOURCE);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The smallest page any supported target maps: the page at an image's base is mapped whole.
const FIRST_PAGE: usize = 4096;

/// The identity of the image whose headers start at `base`: lowercase hex of the Mach-O UUID or
/// the ELF GNU build id, empty when the image has neither or its headers are not shaped as a
/// loader would have accepted them.
///
/// Reads nothing outside what the loader maps for an image it accepted: the page at `base`, then
/// only bytes that the headers read there place inside it (Mach-O: the `sizeofcmds` bytes of load
/// commands after the header, which dyld refuses to load unless they lie in the first segment;
/// ELF: the program headers, read only when they are inside that first page, and a note only when
/// it lies inside a `PT_LOAD` segment).
///
/// # Safety
///
/// `base` must be the address of the first byte of a loaded image (what `dladdr` reports as
/// `dli_fbase`): its first [`FIRST_PAGE`] bytes are mapped and readable, and so is everything the
/// loader mapped for it (a Mach-O header with its load commands; an ELF image's `PT_LOAD`
/// segments, placed relative to the segment that holds the ELF header).
unsafe fn read_image_id(base: usize) -> String {
    // SAFETY: the caller guarantees the first page is mapped; four bytes are inside it.
    let magic = unsafe { (base as *const [u8; 4]).read_unaligned() };
    match magic {
        // Mach-O 64-bit, little endian (`MH_MAGIC_64` 0xfeedfacf).
        [0xcf, 0xfa, 0xed, 0xfe] => {
            // SAFETY: as this function's, for a Mach-O image.
            unsafe { macho_uuid(base) }
        }
        [0x7f, b'E', b'L', b'F'] => {
            // SAFETY: as this function's, for an ELF image.
            unsafe { elf_build_id(base) }
        }
        _ => String::new(),
    }
}

/// Reads `T` at `base + offset`.
///
/// # Safety
///
/// The `size_of::<T>()` bytes there must be mapped and readable.
unsafe fn at<T: Copy>(base: usize, offset: usize) -> T {
    // SAFETY: the caller guarantees the bytes are mapped; the read is unaligned.
    unsafe { ((base + offset) as *const T).read_unaligned() }
}

/// `LC_UUID` (0x1b) of the 64-bit Mach-O image at `base`.
///
/// # Safety
///
/// As [`read_image_id`], for a Mach-O image.
unsafe fn macho_uuid(base: usize) -> String {
    const HEADER: usize = 32;
    const LC_UUID: u32 = 0x1b;
    // SAFETY: `ncmds` (16) and `sizeofcmds` (20) are inside the 32-byte header, in the first page.
    let (ncmds, sizeofcmds): (u32, u32) = unsafe { (at(base, 16), at(base, 20)) };
    // The load commands are the `sizeofcmds` bytes after the header: dyld maps them with the first
    // segment (it refuses an image whose commands do not fit there). Nothing past them is read.
    let end = HEADER.saturating_add(sizeofcmds as usize);
    let mut offset = HEADER;
    for _ in 0..ncmds.min(512) {
        if offset.saturating_add(8) > end {
            break;
        }
        // SAFETY: `cmd` and `cmdsize` are the first 8 bytes of a load command inside `end`.
        let (cmd, size): (u32, u32) = unsafe { (at(base, offset), at(base, offset + 4)) };
        let size = size as usize;
        if size < 8 || offset.saturating_add(size) > end {
            break;
        }
        if cmd == LC_UUID && size >= 24 {
            // SAFETY: the command is inside `end` and at least 24 bytes: `cmd`, `cmdsize`, the UUID.
            let uuid: [u8; 16] = unsafe { at(base, offset + 8) };
            return hex(&uuid);
        }
        offset += size;
    }
    String::new()
}

/// The GNU build id (`NT_GNU_BUILD_ID`) of the ELF image at `base`, 32- or 64-bit.
///
/// # Safety
///
/// As [`read_image_id`], for an ELF image.
unsafe fn elf_build_id(base: usize) -> String {
    const PT_LOAD: u32 = 1;
    const PT_NOTE: u32 = 4;
    const NT_GNU_BUILD_ID: u32 = 3;
    // SAFETY: `e_ident` is in the first page; byte 4 is the class (1 = 32-bit, 2 = 64-bit).
    let class: u8 = unsafe { at(base, 4) };
    let wide = class == 2;
    let (phoff, phentsize, phnum) = if wide {
        // SAFETY: the ELF64 header fields `e_phoff` (0x20), `e_phentsize` (0x36) and `e_phnum`
        // (0x38), in the first page.
        unsafe {
            (
                at::<u64>(base, 0x20) as usize,
                at::<u16>(base, 0x36),
                at::<u16>(base, 0x38),
            )
        }
    } else {
        // SAFETY: the ELF32 header fields at 0x1c, 0x2a and 0x2c, in the first page.
        unsafe {
            (
                at::<u32>(base, 0x1c) as usize,
                at::<u16>(base, 0x2a),
                at::<u16>(base, 0x2c),
            )
        }
    };
    let (phentsize, phnum) = (usize::from(phentsize), usize::from(phnum).min(64));
    // The program headers are read only when the first page holds them (every linker puts them
    // right after the ELF header); an image that keeps them elsewhere gets no id, not a wild read.
    let entry = if wide { 56 } else { 32 };
    if phentsize < entry
        || phoff
            .checked_add(phnum * phentsize)
            .is_none_or(|table_end| table_end > FIRST_PAGE)
    {
        return String::new();
    }
    // (type, p_offset, p_vaddr, p_filesz) of program header `i`.
    let header = |i: usize| -> (u32, usize, usize, usize) {
        let h = phoff + i * phentsize;
        // SAFETY: the whole table is inside the first page (checked above); the offsets are those
        // of `Elf64_Phdr` (type 0, offset 8, vaddr 16, filesz 32) or `Elf32_Phdr` (0, 4, 8, 16).
        unsafe {
            if wide {
                (
                    at(base, h),
                    at::<u64>(base, h + 8) as usize,
                    at::<u64>(base, h + 16) as usize,
                    at::<u64>(base, h + 32) as usize,
                )
            } else {
                (
                    at(base, h),
                    at::<u32>(base, h + 4) as usize,
                    at::<u32>(base, h + 8) as usize,
                    at::<u32>(base, h + 16) as usize,
                )
            }
        }
    };
    let headers: Vec<_> = (0..phnum).map(header).collect();
    let loads = || headers.iter().filter(|h| h.0 == PT_LOAD);
    // `base` is the ELF header, file offset 0: the start of the `PT_LOAD` segment whose file offset
    // is 0. A segment at virtual address `v` is at `base + v - first` (the load bias is `base -
    // first`; `first` is 0 for a shared object or a PIE, 0x400000 or so for a fixed executable).
    let Some(first) = loads().find(|h| h.1 == 0).map(|h| h.2) else {
        return String::new();
    };
    let mapped = |from: usize, to: usize| {
        loads().any(|&(_, _, vaddr, filesz)| {
            from >= vaddr && vaddr.checked_add(filesz).is_some_and(|end| to <= end)
        })
    };
    for &(_, _, vaddr, size) in headers.iter().filter(|h| h.0 == PT_NOTE) {
        let Some(end) = vaddr.checked_add(size) else {
            continue;
        };
        if vaddr < first || !mapped(vaddr, end) {
            continue;
        }
        let mut note = vaddr;
        while note.saturating_add(12) <= end {
            // SAFETY: a note is `namesz`, `descsz`, `type`, then the name and the descriptor, each
            // padded to 4 bytes; these 12 bytes are inside the note segment, inside a `PT_LOAD`.
            let (namesz, descsz, kind): (u32, u32, u32) = unsafe {
                let at_note = base + (note - first);
                (at(at_note, 0), at(at_note, 4), at(at_note, 8))
            };
            let name = note + 12;
            let Some(desc) = name.checked_add((namesz as usize).next_multiple_of(4)) else {
                break;
            };
            if desc.saturating_add(descsz as usize) > end {
                break;
            }
            if kind == NT_GNU_BUILD_ID && namesz == 4 {
                // SAFETY: `descsz` bytes at `desc` are inside the note segment checked above.
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        (base + (desc - first)) as *const u8,
                        descsz as usize,
                    )
                };
                return hex(bytes);
            }
            note = desc.saturating_add((descsz as usize).next_multiple_of(4));
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stack_is_captured_as_offsets_into_this_image() {
        #[inline(never)]
        fn deep(max: usize) -> Vec<u64> {
            Source.capture(max)
        }
        let frames = deep(64);
        assert!(frames.len() >= 3, "{frames:?}");
        assert!(core_base().is_some(), "this image has a base");
        // An offset into an image is far smaller than an address in a 64-bit address space.
        assert!(frames.iter().all(|a| *a < 1 << 36), "{frames:?}");
        assert_eq!(deep(2).len(), 2, "the walk stops at the limit");
    }

    /// A zeroed image of `len` bytes (the tests write headers into it), 8-byte aligned.
    fn image(len: usize) -> Vec<u64> {
        vec![0; len.div_ceil(8)]
    }

    fn put(image: &mut [u64], offset: usize, bytes: &[u8]) {
        // SAFETY: (a test) `image` is a live, exclusively borrowed buffer; `bytes` fits in it.
        let all = unsafe {
            core::slice::from_raw_parts_mut(image.as_mut_ptr().cast::<u8>(), image.len() * 8)
        };
        all[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn id_of(image: &[u64]) -> String {
        // SAFETY: (a test) the buffer is at least a page and holds every segment its headers place.
        unsafe { read_image_id(image.as_ptr() as usize) }
    }

    const BUILD_ID: [u8; 20] = [
        0xde, 0xad, 0xbe, 0xef, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    ];

    /// An ELF64 image: header, program headers at 64, a GNU build-id note at file offset 0x200.
    /// `phdrs` are (type, offset, vaddr, filesz).
    fn elf64(len: usize, phoff: usize, phdrs: &[(u32, u64, u64, u64)]) -> Vec<u64> {
        let mut image = image(len);
        put(&mut image, 0, &[0x7f, b'E', b'L', b'F', 2, 1, 1]);
        put(&mut image, 0x20, &(phoff as u64).to_le_bytes());
        put(&mut image, 0x36, &56_u16.to_le_bytes());
        put(&mut image, 0x38, &(phdrs.len() as u16).to_le_bytes());
        for (i, &(kind, offset, vaddr, filesz)) in phdrs.iter().enumerate() {
            let h = phoff + i * 56;
            put(&mut image, h, &kind.to_le_bytes());
            put(&mut image, h + 8, &offset.to_le_bytes());
            put(&mut image, h + 16, &vaddr.to_le_bytes());
            put(&mut image, h + 32, &filesz.to_le_bytes());
        }
        let mut note = Vec::new();
        note.extend_from_slice(&4_u32.to_le_bytes());
        note.extend_from_slice(&20_u32.to_le_bytes());
        note.extend_from_slice(&3_u32.to_le_bytes());
        note.extend_from_slice(b"GNU\0");
        note.extend_from_slice(&BUILD_ID);
        put(&mut image, 0x200, &note);
        image
    }

    const NOTE_LEN: u64 = 12 + 4 + 20;

    #[test]
    fn an_elf_build_id_is_read_from_a_shared_object() {
        let so = elf64(
            FIRST_PAGE,
            64,
            &[(1, 0, 0, 0x1000), (4, 0x200, 0x200, NOTE_LEN)],
        );
        assert_eq!(id_of(&so), hex(&BUILD_ID));
    }

    #[test]
    fn an_elf_executable_linked_at_a_fixed_address_has_its_build_id_read_at_the_load_bias() {
        // A non-PIE executable (a C embedding linked with `-no-pie`): its first segment is at
        // 0x400000, so the note's address is relative to that, not to 0. The buffer is large
        // enough that reading at `base + p_vaddr` (the old arithmetic) stays inside it.
        let exe = elf64(
            0x40_1000,
            64,
            &[(1, 0, 0x40_0000, 0x1000), (4, 0x200, 0x40_0200, NOTE_LEN)],
        );
        assert_eq!(id_of(&exe), hex(&BUILD_ID));
    }

    #[test]
    fn elf_headers_that_point_outside_what_is_mapped_are_not_followed() {
        // Program headers past the first page (a page the loader may not have mapped).
        let far = elf64(
            3 * FIRST_PAGE,
            0x2000,
            &[(1, 0, 0, 0x3000), (4, 0x200, 0x200, NOTE_LEN)],
        );
        assert_eq!(
            id_of(&far),
            "",
            "the program headers are not read off the first page"
        );
        // A note outside every loaded segment.
        let unmapped = elf64(
            FIRST_PAGE,
            64,
            &[(1, 0, 0, 0x100), (4, 0x200, 0x200, NOTE_LEN)],
        );
        assert_eq!(
            id_of(&unmapped),
            "",
            "a note outside every PT_LOAD is not read"
        );
        // A note segment whose size runs off the end of the address space.
        let wrapped = elf64(
            FIRST_PAGE,
            64,
            &[(1, 0, 0, 0x1000), (4, 0x200, 0x200, u64::MAX)],
        );
        assert_eq!(id_of(&wrapped), "");
        // No segment at file offset 0: no way to tell where the image's addresses start.
        let no_first = elf64(
            FIRST_PAGE,
            64,
            &[(1, 0x1000, 0, 0x1000), (4, 0x200, 0x200, NOTE_LEN)],
        );
        assert_eq!(id_of(&no_first), "");
    }

    /// A 64-bit Mach-O header with `commands` (cmd, cmdsize, payload) and `sizeofcmds`.
    fn macho(sizeofcmds: u32, commands: &[(u32, u32, &[u8])]) -> Vec<u64> {
        let mut image = image(FIRST_PAGE);
        put(&mut image, 0, &[0xcf, 0xfa, 0xed, 0xfe]);
        put(&mut image, 16, &(commands.len() as u32).to_le_bytes());
        put(&mut image, 20, &sizeofcmds.to_le_bytes());
        let mut offset = 32;
        for &(cmd, size, payload) in commands {
            put(&mut image, offset, &cmd.to_le_bytes());
            put(&mut image, offset + 4, &size.to_le_bytes());
            put(&mut image, offset + 8, payload);
            offset += size as usize;
        }
        image
    }

    #[test]
    fn a_mach_o_uuid_is_read_from_the_load_commands_and_not_beyond_them() {
        let uuid = [7_u8; 16];
        let segment = (0x19, 72, &[][..]); // LC_SEGMENT_64, its payload left zero
        let found = macho(72 + 24, &[segment, (0x1b, 24, &uuid)]);
        assert_eq!(id_of(&found), hex(&uuid));
        // The same commands, but the header says they end before the UUID: it is not read.
        let short = macho(72, &[segment, (0x1b, 24, &uuid)]);
        assert_eq!(id_of(&short), "", "nothing past sizeofcmds is read");
        // A command whose size runs past the commands stops the walk (here it would land on a
        // UUID outside them).
        let overlong = macho(72 + 24, &[(0x19, 200, &[]), (0x1b, 24, &uuid)]);
        assert_eq!(id_of(&overlong), "");
    }

    #[test]
    fn the_image_has_an_identity_where_the_platform_gives_one() {
        let id = Source.image_id();
        if cfg!(target_vendor = "apple") {
            assert_eq!(id.len(), 32, "a Mach-O UUID: {id:?}");
        }
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id:?}");
        assert_eq!(id, Source.image_id(), "stable");
    }
}
