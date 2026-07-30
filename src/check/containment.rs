//! Pure graph analysis for recursively contained nominal values.

use std::collections::{BTreeMap, BTreeSet};

use crate::resolution::DeclarationId;

pub(super) fn detect_cycle(
    node: DeclarationId,
    edges: &BTreeMap<DeclarationId, BTreeSet<DeclarationId>>,
    state: &mut BTreeMap<DeclarationId, u8>,
    flagged: &mut BTreeSet<DeclarationId>,
) {
    state.insert(node, 1);
    if let Some(targets) = edges.get(&node) {
        for &target in targets {
            match state.get(&target).copied().unwrap_or(0) {
                0 => detect_cycle(target, edges, state, flagged),
                1 => {
                    flagged.insert(node);
                    flagged.insert(target);
                }
                _ => {}
            }
        }
    }
    state.insert(node, 2);
}
