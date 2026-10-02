//! Lower-case hex, the encoding of every payload in a recording.

/// `bytes` as lower-case hex.
pub(crate) fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    out
}

/// Hex (either case) back to bytes; `None` for an odd length or a non-hex digit.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }
    let digit = |d: u8| char::from(d).to_digit(16);
    digits
        .chunks(2)
        .map(|pair| u8::try_from(digit(pair[0])? * 16 + digit(pair[1])?).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_bad_input() {
        assert_eq!(encode(&[0, 1, 0xab, 0xff]), "0001abff");
        assert_eq!(decode("0001ABff"), Some(vec![0, 1, 0xab, 0xff]));
        assert_eq!(decode(""), Some(vec![]));
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
    }
}
