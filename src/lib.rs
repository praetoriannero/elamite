//! Compiler library for the Elamite programming language.

pub mod artifact;
pub mod backend;
pub mod check;
pub mod conformance;
pub mod diagnostics;
pub mod docs;
pub mod driver;
pub mod ident;
pub mod ir;
pub mod lexer;
pub mod manifest;
pub mod memory;
pub mod package;
pub mod parser;
pub mod promotion;
pub mod resolution;
pub mod scaffold;
pub mod source;
pub mod standard;
pub mod traits;
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
