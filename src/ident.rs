//! Shared identifier-syntax validation used by manifest parsing,
//! module-path derivation, and the lexer.
//!
//! `docs/spec.md` §2 defines an identifier as an ASCII letter or underscore
//! followed by ASCII letters, decimal digits, or underscores. Keeping this
//! predicate shared prevents manifest aliases, file-backed module paths, and
//! source identifiers from diverging.

/// Whether `s` is a valid Elamite identifier under the minimal rule above.
#[must_use]
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_identifiers() {
        assert!(is_valid_identifier("root"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("codec_json2"));
    }

    #[test]
    fn rejects_invalid_identifiers() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("2codec"));
        assert!(!is_valid_identifier("code-c"));
        assert!(!is_valid_identifier("root.models"));
        assert!(!is_valid_identifier(" root"));
    }
}
