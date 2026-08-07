//! Decoding what arrives on the wire.
//!
//! Both faces read percent-encoded text out of a path segment or a
//! query string, and both used to carry their own copy of the decoder.

/// One hex digit, or nothing if the byte is not one.
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode a path segment or query value.
///
/// Works on bytes throughout. Slicing the input as `&str` to read the
/// two hex digits panics whenever the byte after a `%` starts a
/// multi-byte character, because the end of that slice lands inside
/// it: `%aé` was enough to take down the request. The bytes a caller
/// sends are theirs to choose, so anything the decoder does with them
/// has to be defined for all of them.
///
/// An escape that does not parse is left as the literal `%` it was
/// written as, which is what a caller who meant a percent sign gets.
/// Decoded bytes that do not spell UTF-8 become the replacement
/// character rather than an error, because a name the store will
/// simply not match is a 404, and a decoder is the wrong place to
/// decide that.
pub(crate) fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high << 4 | low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    /// A `%` in front of a multi-byte character used to panic.
    ///
    /// The old decoder read its two hex digits by slicing the input as
    /// text, and the end of that slice fell inside the character. Both
    /// the REST router and the console decode path segments, so any
    /// caller could reach it: `GET /v1/files/%aé` was a panicked task.
    #[test]
    fn a_percent_before_a_multibyte_character_decodes() {
        for raw in ["%aé", "a%bé", "%é", "%aa%é", "é%", "%%é", "%\u{1f600}"] {
            // Arriving at an answer at all is the assertion.
            let _ = percent_decode(raw);
        }
        assert_eq!(percent_decode("%aé"), "%aé");
        assert_eq!(percent_decode("a%bé"), "a%bé");
    }

    /// What it decoded before, it still decodes.
    #[test]
    fn ordinary_escapes_are_unchanged() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%2F"), "/");
        assert_eq!(percent_decode("%2f"), "/");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode(""), "");
    }

    /// An escape that is not one stays the text it was written as.
    #[test]
    fn an_escape_that_does_not_parse_is_left_alone() {
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%a"), "%a");
        assert_eq!(percent_decode("100%"), "100%");
        // A sign written by hand in front of a hex-looking pair is the
        // one case that cannot be told apart from an escape.
        assert_eq!(percent_decode("%2Fa"), "/a");
    }

    /// Bytes that do not spell UTF-8 come back as the replacement
    /// character rather than as an error or a panic.
    #[test]
    fn bytes_that_are_not_text_survive_decoding() {
        assert_eq!(percent_decode("%ff"), "\u{fffd}");
        assert_eq!(percent_decode("%c3%a9"), "é");
        assert_eq!(percent_decode("%e2%82%ac"), "€");
    }
}
