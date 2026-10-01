//! Internal service tokens (runtime, notify, cluster): constant-time check.

use subtle::ConstantTimeEq;

/// `true` when `supplied` (trimmed) equals a non-empty `expected`, compared
/// in constant time. Only the length can leak, never the content.
pub fn matches(supplied: &str, expected: &str) -> bool {
    !expected.is_empty() && bool::from(supplied.trim().as_bytes().ct_eq(expected.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn compares_trimmed_and_fails_closed() {
        assert!(matches(" tok ", "tok"));
        assert!(!matches("tok", "tak"));
        assert!(!matches("to", "tok"));
        assert!(!matches("", ""));
        assert!(!matches("tok", ""));
    }
}
