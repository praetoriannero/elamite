//! Compiler library for the Elamite programming language.

pub mod backend;
pub mod check;
pub mod diagnostics;
pub mod driver;
pub mod ident;
pub mod ir;
pub mod lexer;
pub mod manifest;
pub mod memory;
pub mod package;
pub mod parser;
pub mod resolution;
pub mod source;
pub mod types;

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
