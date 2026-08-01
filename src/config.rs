//! Compiler-wide target and optimization configuration.
//!
//! These choices are consumed by semantic checking, lowering, native builds,
//! conformance, and future test execution. They are not owned by the C backend
//! or command-line driver.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    X86,
    X86_64,
}

impl Target {
    #[must_use]
    pub fn pointer_bits(self) -> u8 {
        match self {
            Self::X86 => 32,
            Self::X86_64 => 64,
        }
    }

    #[must_use]
    pub fn compiler_flag(self) -> &'static str {
        match self {
            Self::X86 => "-m32",
            Self::X86_64 => "-m64",
        }
    }

    #[must_use]
    pub fn host() -> Self {
        if cfg!(target_pointer_width = "32") {
            Self::X86
        } else {
            Self::X86_64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optimization {
    Debug,
    Release,
}

/// Language feature policy carried through compiler entry points.
///
/// User-defined compile-time syntax generation is stable; this empty value is
/// retained so future, unrelated experiments do not churn driver APIs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompilerFeatures {
    _reserved: (),
}

impl Optimization {
    #[must_use]
    pub fn compiler_flag(self) -> &'static str {
        match self {
            Self::Debug => "-O0",
            Self::Release => "-O3",
        }
    }
}
