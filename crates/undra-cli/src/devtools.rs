//! The devtools page of `undra dev` (ADR-054): the assets compiled into this binary, the secret
//! each run puts in the page's address, and when the page is served at all.
//!
//! The page is built from `runtimes/ts/devtools` by its `build.sh` and **committed** under
//! `assets/devtools/`, so `cargo build` needs no Node; CI rebuilds it and fails when the committed
//! files differ. The runner `undra dev` generates includes the same files (written beside its
//! sources by [`write_assets`]) and serves them from the dev server's own listener.

use std::io::Read;
use std::path::Path;

use crate::cli::DevtoolsMode;
use crate::error::{CliError, Result};
use crate::fsutil::create_dir_all;

/// One file of the page. (Its content type is written in the runner's own table, which is what
/// serves it.)
pub struct Asset {
    /// The file name, below `/devtools/`.
    pub path: &'static str,
    /// Its bytes.
    pub bytes: &'static [u8],
}

/// The page: what `build.sh` wrote.
pub static ASSETS: [Asset; 3] = [
    Asset {
        path: "index.html",
        bytes: include_bytes!("../assets/devtools/index.html"),
    },
    Asset {
        path: "app.js",
        bytes: include_bytes!("../assets/devtools/app.js"),
    },
    Asset {
        path: "app.css",
        bytes: include_bytes!("../assets/devtools/app.css"),
    },
];

/// Writes the assets to `dir/devtools/` (next to the runner's `main.rs`, which `include_bytes!`s
/// them). Files that did not change are left alone, so Cargo does not rebuild for nothing.
///
/// # Errors
///
/// `C0010` when a file cannot be written.
pub fn write_assets(dir: &Path) -> Result<()> {
    for asset in &ASSETS {
        write_bytes_if_changed(&dir.join("devtools").join(asset.path), asset.bytes)?;
    }
    Ok(())
}

fn write_bytes_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|have| have == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    std::fs::write(path, bytes).map_err(|e| CliError::io("write", path, &e))
}

/// A secret for this run of `undra dev`: 128 random bits as 32 hex digits. The operating system's
/// source when there is one, otherwise the keys of the standard library's `RandomState` (which
/// the operating system seeds).
#[must_use]
pub fn new_token() -> String {
    let mut bytes = [0_u8; 16];
    let from_os = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if from_os.is_err() {
        use std::hash::{BuildHasher, Hasher};
        let state = std::collections::hash_map::RandomState::new();
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut hasher = state.build_hasher();
            hasher.write_usize(i);
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            chunk.copy_from_slice(&hasher.finish().to_le_bytes());
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `addr` (`host:port`, as `--addr` takes it) is a loopback address.
#[must_use]
pub fn is_loopback(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map_or(addr, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether the page is served: always with `on`, never with `off`, and with `auto` (the default)
/// only on a loopback address. The page can read the core's state and restore it, so reaching it
/// from the network is a decision, not a default; the token is required either way.
#[must_use]
pub fn enabled(mode: DevtoolsMode, addr: &str) -> bool {
    match mode {
        DevtoolsMode::On => true,
        DevtoolsMode::Off => false,
        DevtoolsMode::Auto => is_loopback(addr),
    }
}

/// The address of the page for a server at `ws_url`: the same host and port over `http`, with the
/// token. `0.0.0.0` (what a server on every interface prints) is replaced by the loopback address,
/// which is where the developer's own browser reaches it.
#[must_use]
pub fn page_url(ws_url: &str, token: &str) -> Option<String> {
    let socket = ws_url
        .strip_prefix("ws://")?
        .replace("0.0.0.0", "127.0.0.1");
    let socket = socket.replace("[::]", "[::1]");
    Some(format!(
        "http://{}/devtools?token={token}",
        socket.trim_end_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_32_hex_digits_and_not_the_same_twice() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn auto_serves_the_page_on_loopback_addresses_only() {
        for yes in ["127.0.0.1:7443", "localhost:1", "[::1]:7443", "127.0.0.1:0"] {
            assert!(enabled(DevtoolsMode::Auto, yes), "{yes}");
        }
        for no in [
            "0.0.0.0:7443",
            "[::]:7443",
            "192.168.1.5:7443",
            "myhost.local:7443",
            // Spellings the OS would resolve to loopback but this does not recognise: closed, not open.
            "127.1:7443",
            "[::ffff:127.0.0.1]:7443",
            ":7443",
            "7443",
            "",
        ] {
            assert!(!enabled(DevtoolsMode::Auto, no), "{no}");
            assert!(enabled(DevtoolsMode::On, no), "{no}");
        }
        assert!(!enabled(DevtoolsMode::Off, "127.0.0.1:7443"));
    }

    #[test]
    fn the_page_is_where_the_server_is() {
        assert_eq!(
            page_url("ws://127.0.0.1:7443", "abc").as_deref(),
            Some("http://127.0.0.1:7443/devtools?token=abc")
        );
        assert_eq!(
            page_url("ws://0.0.0.0:9000", "t").as_deref(),
            Some("http://127.0.0.1:9000/devtools?token=t")
        );
        assert_eq!(page_url("http://x", "t"), None);
    }

    #[test]
    fn the_embedded_page_is_complete_and_within_its_size_budget() {
        let names: Vec<&str> = ASSETS.iter().map(|a| a.path).collect();
        assert_eq!(names, ["index.html", "app.js", "app.css"]);
        assert!(ASSETS.iter().all(|a| !a.bytes.is_empty()));
        let index = std::str::from_utf8(ASSETS[0].bytes).unwrap();
        assert!(
            index.contains("__UNDRA_DEVTOOLS_TOKEN__"),
            "the server puts the token in the page"
        );
        // No file may carry the token of a build machine, an absolute path or a source map.
        for a in &ASSETS {
            let text = String::from_utf8_lossy(a.bytes);
            assert!(!text.contains("sourceMappingURL"), "{}", a.path);
            assert!(
                !text.contains("/Users/") && !text.contains("/home/"),
                "{}",
                a.path
            );
        }
    }

    #[test]
    fn the_page_is_at_most_150_kb_gzipped() {
        // `build.sh` measures the real gzip (and CI runs it); here, the raw size bounds it from above
        // loosely enough that a gzip can only be smaller: 150 KB gzipped is never less than 150 KB
        // raw / 10, and a bundle ten times the budget is a bundle someone added a framework to.
        let raw: usize = ASSETS.iter().map(|a| a.bytes.len()).sum();
        assert!(
            raw < 1_500_000,
            "the page is {raw} bytes raw: a bundle that big is not within 150 KB gzipped"
        );
    }
}
