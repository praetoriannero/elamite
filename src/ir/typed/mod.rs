//! Typed high-level IR and its lowering from checked syntax.

mod lower;
mod model;

pub use lower::lower_typed_ir;
pub use model::*;
