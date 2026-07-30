//! Pattern exhaustiveness and redundancy coverage model.

use std::collections::BTreeSet;

use crate::resolution::VariantId;

#[derive(Debug, Clone)]
pub(super) enum Coverage {
    None,
    Catchall,
    Variants(BTreeSet<VariantId>),
    Bools(BTreeSet<bool>),
    Dereferenced(Box<Coverage>),
    Other,
}

impl Coverage {
    pub(super) fn union(self, other: Coverage) -> Coverage {
        match (self, other) {
            (Coverage::None, other) => other,
            (this, Coverage::None) => this,
            (Coverage::Catchall, _) | (_, Coverage::Catchall) => Coverage::Catchall,
            (Coverage::Variants(mut a), Coverage::Variants(b)) => {
                a.extend(b);
                Coverage::Variants(a)
            }
            (Coverage::Bools(mut a), Coverage::Bools(b)) => {
                a.extend(b);
                Coverage::Bools(a)
            }
            (Coverage::Dereferenced(a), Coverage::Dereferenced(b)) => {
                Coverage::Dereferenced(Box::new(a.union(*b)))
            }
            _ => Coverage::Other,
        }
    }

    pub(super) fn is_catchall(&self) -> bool {
        match self {
            Coverage::Catchall => true,
            Coverage::Dereferenced(inner) => inner.is_catchall(),
            _ => false,
        }
    }

    pub(super) fn covers_variants(&self, variants: &BTreeSet<VariantId>) -> bool {
        match self {
            Coverage::Variants(covered) => variants.is_subset(covered),
            Coverage::Dereferenced(inner) => inner.covers_variants(variants),
            _ => false,
        }
    }

    pub(super) fn covers_bools(&self, values: &BTreeSet<bool>) -> bool {
        match self {
            Coverage::Bools(covered) => values.is_subset(covered),
            Coverage::Dereferenced(inner) => inner.covers_bools(values),
            _ => false,
        }
    }

    pub(super) fn is_covered_by(&self, previous: &Coverage) -> bool {
        match self {
            Coverage::Variants(variants) => {
                !variants.is_empty() && previous.covers_variants(variants)
            }
            Coverage::Bools(values) => !values.is_empty() && previous.covers_bools(values),
            Coverage::Dereferenced(inner) => match previous {
                Coverage::Dereferenced(previous_inner) => inner.is_covered_by(previous_inner),
                _ => false,
            },
            Coverage::Catchall => previous.is_catchall(),
            Coverage::None | Coverage::Other => false,
        }
    }
}
