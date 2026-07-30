//! Explicit control-flow IR and lowering from typed high-level IR.

mod lower;
mod model;

pub use lower::lower_control_flow;
pub use model::*;
