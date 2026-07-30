//! Behavior-neutral expansion boundary.
//!
//! Milestone 20 makes the parsed-to-resolved phase edge explicit. User-defined
//! expansion remains disabled until **Macro expansion foundations**, so the current phase retains
//! parsed units exactly and contributes no diagnostics.

use crate::parsed::{ParsedPackage, ParsedPackageOutput};

#[derive(Debug)]
pub struct ExpandedPackage {
    pub units: Vec<crate::parsed::ParsedUnit>,
}

pub struct ExpansionOutput {
    pub package: ExpandedPackage,
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

#[must_use]
pub fn expand(parsed: ParsedPackageOutput) -> ExpansionOutput {
    let ParsedPackageOutput {
        package: ParsedPackage { units },
        diagnostics,
    } = parsed;
    ExpansionOutput {
        package: ExpandedPackage { units },
        diagnostics,
    }
}
