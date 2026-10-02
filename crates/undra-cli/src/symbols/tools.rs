//! Finding the tools that strip and symbolicate: on `PATH`, in the Android NDK, or through `xcrun`.

use std::path::{Path, PathBuf};

use crate::sys::{Os, Sys};
use crate::toolchain::Toolchain;

/// The directory of the NDK's LLVM tools (`<ndk>/toolchains/llvm/prebuilt/<host>/bin`): the NDK
/// has one `prebuilt` directory per host it runs on, and a machine has the one it unpacked.
#[must_use]
pub fn ndk_bin(sys: &dyn Sys, ndk: &Path) -> Option<PathBuf> {
    let prebuilt = ndk.join("toolchains/llvm/prebuilt");
    let mut hosts = sys.list_dir(&prebuilt);
    hosts.sort();
    hosts
        .into_iter()
        .map(|host| prebuilt.join(host).join("bin"))
        .find(|bin| sys.is_dir(bin))
}

/// The tool `xcrun --find <name>` reports (macOS): the one of the active Xcode.
#[must_use]
pub fn xcrun_find(sys: &dyn Sys, toolchain: &Toolchain, name: &str) -> Option<PathBuf> {
    if sys.os() != Os::Macos {
        return None;
    }
    let xcrun = toolchain.which(sys, "xcrun")?;
    sys.run(&xcrun, &["--find", name], &toolchain.env_pairs())
        .filter(|out| out.success)
        .map(|out| PathBuf::from(out.stdout.trim()))
        .filter(|path| !path.as_os_str().is_empty() && sys.is_file(path))
}

/// The first of `names` that runs on this machine: on `PATH`, then in the NDK's LLVM tools, then
/// (on a Mac) the one `xcrun` finds in the active Xcode.
#[must_use]
pub fn find(sys: &dyn Sys, toolchain: &Toolchain, names: &[&str]) -> Option<PathBuf> {
    for name in names {
        if let Some(found) = toolchain.which(sys, name) {
            return Some(found);
        }
        if let Some(bin) = toolchain
            .android_ndk
            .as_deref()
            .and_then(|ndk| ndk_bin(sys, ndk))
        {
            let candidate = bin.join(name);
            if sys.is_file(&candidate) {
                return Some(candidate);
            }
        }
        if let Some(found) = xcrun_find(sys, toolchain, name) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    #[test]
    fn the_ndk_tools_are_found_below_the_prebuilt_directory_of_this_host() {
        let sys = FakeSys::linux()
            .with_dir("/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin")
            .with_file("/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip");
        let toolchain = Toolchain {
            android_ndk: Some(PathBuf::from("/ndk")),
            ..Toolchain::default()
        };
        assert_eq!(
            ndk_bin(&sys, Path::new("/ndk")),
            Some(PathBuf::from(
                "/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
            ))
        );
        assert_eq!(
            find(&sys, &toolchain, &["llvm-strip"]),
            Some(PathBuf::from(
                "/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip"
            ))
        );
        assert_eq!(find(&sys, &toolchain, &["llvm-nothing"]), None);
    }

    #[test]
    fn a_tool_on_path_wins_over_the_ndk() {
        let sys = FakeSys::linux()
            .with_tool("llvm-symbolizer", "/usr/bin/llvm-symbolizer")
            .with_dir("/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin")
            .with_file("/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-symbolizer");
        let toolchain = Toolchain {
            android_ndk: Some(PathBuf::from("/ndk")),
            ..Toolchain::default()
        };
        assert_eq!(
            find(&sys, &toolchain, &["llvm-symbolizer"]),
            Some(PathBuf::from("/usr/bin/llvm-symbolizer"))
        );
    }
}
