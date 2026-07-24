//! Compiler library for the Elamite programming language.

/// Returns the compiler package version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_defined() {
        assert!(!super::version().is_empty());
    }
}
