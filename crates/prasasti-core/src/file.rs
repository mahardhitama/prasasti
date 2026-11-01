#[derive(Debug, PartialEq, Eq)]
pub struct NotUtf8;

/// Saving writes UTF-8 with LF; anything else on disk would be rewritten by it.
pub fn saves_unchanged(raw: &[u8]) -> bool {
    !raw.contains(&b'\r') && std::str::from_utf8(raw).is_ok()
}

pub fn decode(raw: &[u8]) -> Result<String, NotUtf8> {
    std::str::from_utf8(raw).map(str::to_owned).map_err(|_| NotUtf8)
}

pub fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_unchanged_cases() {
        assert!(saves_unchanged(b"a\nb"));
        assert!(!saves_unchanged(b"a\r\nb"));
        assert!(!saves_unchanged(&[0xff, 0x41]));
        assert!(saves_unchanged("\u{feff}héllo".as_bytes())); // BOM survives a round trip
    }

    #[test]
    fn decode_refuses_invalid_utf8() {
        assert!(decode(&[0xff]).is_err());
        assert_eq!(decode("héllo".as_bytes()).unwrap(), "héllo");
    }

    #[test]
    fn normalizes_newlines() {
        assert_eq!(normalize_newlines("a\r\nb\rc"), "a\nb\nc");
    }
}
