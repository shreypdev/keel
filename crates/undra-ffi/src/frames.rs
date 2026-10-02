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
        walk.out.push((ip - walk.base) as u64);
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

/// The identity of the image whose headers start at `base`: lowercase hex of the Mach-O UUID or
/// the ELF GNU build id, empty when the image has neither.
///
/// # Safety
///
/// `base` must be the address of the first byte of a loaded image, whose headers and (for ELF)
/// program-header notes are mapped and readable.
unsafe fn read_image_id(base: usize) -> String {
    // SAFETY: the caller guarantees the first four bytes are mapped.
    let magic = unsafe { (base as *const [u8; 4]).read_unaligned() };
    match magic {
        // Mach-O 64-bit, little endian (`MH_MAGIC_64` 0xfeedfacf).
        [0xcf, 0xfa, 0xed, 0xfe] => {
            // SAFETY: a Mach-O header and its load commands are mapped with the image.
            unsafe { macho_uuid(base) }
        }
        [0x7f, b'E', b'L', b'F'] => {
            // SAFETY: an ELF header and its program headers are in the first mapped segment.
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
    // SAFETY: `ncmds` is at offset 16 of `mach_header_64`; the header is mapped.
    let ncmds: u32 = unsafe { at(base, 16) };
    let mut offset = HEADER;
    for _ in 0..ncmds.min(512) {
        // SAFETY: each load command starts with `cmd` and `cmdsize`; the commands are mapped.
        let (cmd, size): (u32, u32) = unsafe { (at(base, offset), at(base, offset + 4)) };
        if cmd == LC_UUID && size >= 24 {
            // SAFETY: the command is 24 bytes: `cmd`, `cmdsize`, 16 bytes of UUID.
            let uuid: [u8; 16] = unsafe { at(base, offset + 8) };
            return hex(&uuid);
        }
        if size < 8 {
            break;
        }
        offset += size as usize;
    }
    String::new()
}

/// The GNU build id (`NT_GNU_BUILD_ID`) of the ELF image at `base`, 32- or 64-bit.
///
/// # Safety
///
/// As [`read_image_id`], for an ELF image.
unsafe fn elf_build_id(base: usize) -> String {
    const PT_NOTE: u32 = 4;
    const NT_GNU_BUILD_ID: u32 = 3;
    // SAFETY: `e_ident` is mapped; byte 4 is the class (1 = 32-bit, 2 = 64-bit).
    let class: u8 = unsafe { at(base, 4) };
    let (phoff, phentsize, phnum) = if class == 2 {
        // SAFETY: the ELF64 header fields `e_phoff` (0x20), `e_phentsize` (0x36) and `e_phnum` (0x38).
        unsafe {
            (
                at::<u64>(base, 0x20) as usize,
                at::<u16>(base, 0x36),
                at::<u16>(base, 0x38),
            )
        }
    } else {
        // SAFETY: the ELF32 header fields at 0x1c, 0x2a and 0x2c.
        unsafe {
            (
                at::<u32>(base, 0x1c) as usize,
                at::<u16>(base, 0x2a),
                at::<u16>(base, 0x2c),
            )
        }
    };
    for i in 0..usize::from(phnum).min(64) {
        let header = phoff + i * usize::from(phentsize);
        // SAFETY: program headers are in the first mapped segment of the image.
        let kind: u32 = unsafe { at(base, header) };
        if kind != PT_NOTE {
            continue;
        }
        // The note segment's address and size, relative to the load bias (the base of a shared
        // object, whose first segment is at virtual address 0).
        let (vaddr, size) = if class == 2 {
            // SAFETY: as above: the ELF64 `p_vaddr` (16) and `p_filesz` (32) of the program header.
            unsafe {
                (
                    at::<u64>(base, header + 16) as usize,
                    at::<u64>(base, header + 32) as usize,
                )
            }
        } else {
            // SAFETY: as above: the ELF32 `p_vaddr` (8) and `p_filesz` (16) of the program header.
            unsafe {
                (
                    at::<u32>(base, header + 8) as usize,
                    at::<u32>(base, header + 16) as usize,
                )
            }
        };
        let mut note = vaddr;
        let end = vaddr.saturating_add(size);
        while note + 12 <= end {
            // SAFETY: a note is `namesz`, `descsz`, `type`, then the name and the descriptor, each
            // padded to 4 bytes, inside the segment the program header describes.
            let (namesz, descsz, kind): (u32, u32, u32) =
                unsafe { (at(base, note), at(base, note + 4), at(base, note + 8)) };
            let name = note + 12;
            let desc = name + (namesz as usize).next_multiple_of(4);
            if desc + descsz as usize > end {
                break;
            }
            if kind == NT_GNU_BUILD_ID && namesz == 4 {
                // SAFETY: `descsz` bytes at `desc` are inside the note segment checked above.
                let bytes = unsafe {
                    core::slice::from_raw_parts((base + desc) as *const u8, descsz as usize)
                };
                return hex(bytes);
            }
            note = desc + (descsz as usize).next_multiple_of(4);
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
