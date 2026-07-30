//! Stable built-in runtime trap categories represented in control-flow IR.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapKind {
    IntegerOverflow,
    DivisionByZero,
    InvalidShift,
    IndexOutOfBounds,
    MissingMapKey,
    InvalidNumericConversion,
}

impl TrapKind {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::IntegerOverflow => "E-RUN-OVERFLOW",
            Self::DivisionByZero => "E-RUN-DIVZERO",
            Self::InvalidShift => "E-RUN-SHIFT",
            Self::IndexOutOfBounds => "E-RUN-INDEX",
            Self::MissingMapKey => "E-RUN-KEY",
            Self::InvalidNumericConversion => "E-RUN-CAST",
        }
    }
}
