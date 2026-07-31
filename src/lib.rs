//! Compiler library for the Elamite programming language.

pub mod artifact;
pub mod backend;
pub mod check;
pub mod config;
pub mod conformance;
pub mod diagnostics;
pub mod docs;
pub mod driver;
pub mod expansion;
pub mod ident;
pub mod ir;
pub mod lexer;
pub mod manifest;
pub mod memory;
pub mod operations;
pub mod package;
pub mod parsed;
pub mod parser;
pub mod promotion;
pub mod resolution;
pub mod scaffold;
pub mod source;
pub mod standard;
pub mod syntax;
pub mod testing;
pub mod traits;
pub mod types;

/// Returns the compiler package version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns the language specification revision targeted by this compiler.
#[must_use]
pub const fn spec_revision() -> &'static str {
    "0.5.0-draft"
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_identity_is_defined() {
        assert!(!super::version().is_empty());
        assert_eq!(super::spec_revision(), "0.5.0-draft");
    }
}
