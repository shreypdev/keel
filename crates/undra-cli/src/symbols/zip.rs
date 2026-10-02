//! A minimal ZIP writer for `native-debug-symbols.zip` (the Play Console layout), used when the
//! system has no `zip` tool.
//!
//! Entries are stored, not compressed, with a fixed timestamp: the same files always give the
//! same bytes, and any unzip reads it. (With `zip` installed the CLI uses it instead, which
//! compresses; the layout is the same.)

/// CRC-32 (IEEE 802.3), as ZIP wants it.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut table = [0_u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    }
    static TABLE: [u32; 256] = table();
    !bytes.iter().fold(!0_u32, |crc, byte| {
        TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8)
    })
}

/// 1980-01-01, the earliest time a ZIP entry can have (DOS date; the time is midnight).
const DOS_DATE: u16 = (1 << 5) | 1;

/// The bytes of a ZIP archive holding `files` (name, contents), stored in the order given.
///
/// # Errors
///
/// A sentence when a file or the archive is too large for the 32-bit format (4 GiB).
pub fn archive(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let too_large = || "a ZIP archive of more than 4 GiB is not supported".to_owned();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let size = u32::try_from(data.len()).map_err(|_| too_large())?;
        let offset = u32::try_from(out.len()).map_err(|_| too_large())?;
        let name_len = u16::try_from(name.len()).map_err(|_| "a file name is too long")?;
        let crc = crc32(data);
        // Local file header.
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800_u16.to_le_bytes()); // flags: UTF-8 names
        out.extend_from_slice(&0_u16.to_le_bytes()); // stored
        out.extend_from_slice(&0_u16.to_le_bytes()); // time
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes()); // extra
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Central directory header.
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&0x031e_u16.to_le_bytes()); // made by: Unix, 3.0
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&0x0800_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&[0; 4]); // extra, comment
        central.extend_from_slice(&[0; 4]); // disk, internal attributes
        central.extend_from_slice(&(0o100_644_u32 << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = u32::try_from(out.len()).map_err(|_| too_large())?;
    let central_size = u32::try_from(central.len()).map_err(|_| too_large())?;
    let count = u16::try_from(files.len()).map_err(|_| "too many files for a ZIP archive")?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes()); // comment
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn the_archive_has_the_layout_unzip_expects() {
        let zip = archive(&[
            ("arm64-v8a/libacme.so".to_owned(), b"first".to_vec()),
            ("x86_64/libacme.so".to_owned(), vec![7; 1000]),
        ])
        .unwrap();
        // The end-of-central-directory record is the last 22 bytes and counts both entries.
        let end = &zip[zip.len() - 22..];
        assert_eq!(&end[..4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([end[10], end[11]]), 2);
        // The same input, the same bytes.
        assert_eq!(
            zip,
            archive(&[
                ("arm64-v8a/libacme.so".to_owned(), b"first".to_vec()),
                ("x86_64/libacme.so".to_owned(), vec![7; 1000]),
            ])
            .unwrap()
        );
        assert!(zip.windows(5).any(|w| w == b"first"));
    }
}
