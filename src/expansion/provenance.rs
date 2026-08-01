//! Stable identities and origin chains for physical and generated tokens.
//!
//! A [`Span`] always denotes bytes in a physical source file. Generated output
//! therefore receives an expansion-local location instead of borrowing or
//! fabricating physical offsets. The append-only tables here connect that
//! location to the invocation, definition, and exact token that produced it.

use crate::source::Span;

/// Stable identity of one macro expansion within a compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpansionId(u32);

impl ExpansionId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Stable identity of one physical or generated token origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OriginId(u32);

impl OriginId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Inclusive origin range for a token-tree node.
///
/// This remains meaningful for generated trees whose endpoints have no
/// physical byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OriginRange {
    pub first: OriginId,
    pub last: OriginId,
}

impl OriginRange {
    #[must_use]
    pub fn new(first: OriginId, last: OriginId) -> Self {
        Self { first, last }
    }
}

/// Whether an emitted token was copied from a definition-side literal token or
/// substituted from invocation-side input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedSource {
    Definition(OriginId),
    Invocation(OriginId),
}

impl GeneratedSource {
    #[must_use]
    pub fn origin(self) -> OriginId {
        match self {
            Self::Definition(origin) | Self::Invocation(origin) => origin,
        }
    }
}

/// Location and immediate source of one represented token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Physical {
        span: Span,
    },
    Generated {
        expansion: ExpansionId,
        /// Zero-based output position within `expansion`.
        output_index: u32,
        source: GeneratedSource,
    },
}

/// One macro invocation and the definition selected for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expansion {
    pub id: ExpansionId,
    pub invocation: OriginId,
    pub definition: OriginId,
    /// Expansion that emitted the invocation token, if any.
    pub parent: Option<ExpansionId>,
}

/// One frame in an inner-to-outer expansion backtrace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpansionTraceFrame {
    pub expansion: ExpansionId,
    pub invocation: OriginId,
    pub definition: OriginId,
}

/// Complete deterministic trace for one token origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginTrace {
    pub origin: OriginId,
    /// The token itself followed by each token it was copied or substituted
    /// from, ending at a physical token.
    pub source_chain: Vec<OriginId>,
    /// The expansion that emitted the token followed by enclosing expansions.
    pub expansions: Vec<ExpansionTraceFrame>,
}

/// Append-only provenance database for one expanded package graph.
///
/// Calling the registration methods in deterministic package/module/token
/// order produces deterministic identities. All references point to entries
/// that already exist, preventing cycles in origin chains by construction.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProvenanceTable {
    origins: Vec<Origin>,
    expansions: Vec<Expansion>,
    next_output_indices: Vec<u32>,
}

impl ProvenanceTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one physical token occurrence.
    pub fn register_physical(&mut self, span: Span) -> OriginId {
        let id =
            OriginId(u32::try_from(self.origins.len()).expect("more than u32::MAX token origins"));
        self.origins.push(Origin::Physical { span });
        id
    }

    /// Registers one expansion. Invocation and definition origins must already
    /// exist; a generated invocation automatically establishes the parent.
    pub fn register_expansion(
        &mut self,
        invocation: OriginId,
        definition: OriginId,
    ) -> ExpansionId {
        self.assert_origin(invocation);
        self.assert_origin(definition);
        let parent = match self.origin(invocation) {
            Origin::Physical { .. } => None,
            Origin::Generated { expansion, .. } => Some(*expansion),
        };
        let id = ExpansionId(
            u32::try_from(self.expansions.len()).expect("more than u32::MAX expansions"),
        );
        self.expansions.push(Expansion {
            id,
            invocation,
            definition,
            parent,
        });
        self.next_output_indices.push(0);
        id
    }

    /// Registers one generated token without assigning it a physical span.
    pub fn register_generated(
        &mut self,
        expansion: ExpansionId,
        source: GeneratedSource,
    ) -> OriginId {
        self.assert_expansion(expansion);
        self.assert_origin(source.origin());
        let output_index = self.next_output_indices[expansion.index()];
        self.next_output_indices[expansion.index()] = output_index
            .checked_add(1)
            .expect("more than u32::MAX outputs in one expansion");
        let id =
            OriginId(u32::try_from(self.origins.len()).expect("more than u32::MAX token origins"));
        self.origins.push(Origin::Generated {
            expansion,
            output_index,
            source,
        });
        id
    }

    #[must_use]
    pub fn origin(&self, id: OriginId) -> &Origin {
        &self.origins[id.index()]
    }

    #[must_use]
    pub fn expansion(&self, id: ExpansionId) -> &Expansion {
        &self.expansions[id.index()]
    }

    #[must_use]
    pub fn origins(&self) -> &[Origin] {
        &self.origins
    }

    #[must_use]
    pub fn expansions(&self) -> &[Expansion] {
        &self.expansions
    }

    /// Returns a span only when the origin itself is physical.
    ///
    /// This intentionally does not project generated output onto its source
    /// token: callers must use [`Self::trace`] when they need related physical
    /// definition and invocation locations.
    #[must_use]
    pub fn physical_span(&self, id: OriginId) -> Option<Span> {
        match self.origin(id) {
            Origin::Physical { span } => Some(*span),
            Origin::Generated { .. } => None,
        }
    }

    /// Returns a physical range only when both endpoints are directly physical
    /// locations in the same file.
    #[must_use]
    pub fn physical_range(&self, range: OriginRange) -> Option<Span> {
        let first = self.physical_span(range.first)?;
        let last = self.physical_span(range.last)?;
        (first.file == last.file && first.start <= last.end)
            .then(|| Span::new(first.file, first.start, last.end))
    }

    /// Builds an inner-to-outer trace without projecting generated locations
    /// into physical files.
    #[must_use]
    pub fn trace(&self, origin: OriginId) -> OriginTrace {
        self.assert_origin(origin);
        let mut source_chain = vec![origin];
        let mut source = origin;
        while let Origin::Generated {
            source: generated_source,
            ..
        } = self.origin(source)
        {
            source = generated_source.origin();
            source_chain.push(source);
        }

        let mut expansions = Vec::new();
        let mut expansion = match self.origin(origin) {
            Origin::Physical { .. } => None,
            Origin::Generated { expansion, .. } => Some(*expansion),
        };
        while let Some(id) = expansion {
            let record = self.expansion(id);
            expansions.push(ExpansionTraceFrame {
                expansion: id,
                invocation: record.invocation,
                definition: record.definition,
            });
            expansion = record.parent;
        }

        OriginTrace {
            origin,
            source_chain,
            expansions,
        }
    }

    fn assert_origin(&self, id: OriginId) {
        assert!(
            id.index() < self.origins.len(),
            "origin identity belongs to this append-only table"
        );
    }

    fn assert_expansion(&self, id: ExpansionId) {
        assert!(
            id.index() < self.expansions.len(),
            "expansion identity belongs to this append-only table"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::source::{FileId, SourceManager, Span};

    use super::*;

    fn span(file: FileId, start: u32, end: u32) -> Span {
        Span::new(file, start, end)
    }

    #[test]
    fn nested_generated_origins_trace_invocations_and_definitions() {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("macro.elx"), " ".repeat(64));
        let mut provenance = ProvenanceTable::new();
        let outer_definition = provenance.register_physical(span(file, 0, 5));
        let outer_invocation = provenance.register_physical(span(file, 20, 28));
        let inner_definition = provenance.register_physical(span(file, 40, 45));

        let outer = provenance.register_expansion(outer_invocation, outer_definition);
        let generated_inner_invocation =
            provenance.register_generated(outer, GeneratedSource::Definition(outer_definition));
        let inner = provenance.register_expansion(generated_inner_invocation, inner_definition);
        let output =
            provenance.register_generated(inner, GeneratedSource::Definition(inner_definition));

        assert_eq!(provenance.physical_span(output), None);
        assert_eq!(
            *provenance.origin(output),
            Origin::Generated {
                expansion: inner,
                output_index: 0,
                source: GeneratedSource::Definition(inner_definition),
            }
        );
        let trace = provenance.trace(output);
        assert_eq!(trace.source_chain, [output, inner_definition]);
        assert_eq!(
            trace.expansions,
            [
                ExpansionTraceFrame {
                    expansion: inner,
                    invocation: generated_inner_invocation,
                    definition: inner_definition,
                },
                ExpansionTraceFrame {
                    expansion: outer,
                    invocation: outer_invocation,
                    definition: outer_definition,
                },
            ]
        );
    }

    #[test]
    fn generated_output_indices_are_stable_within_each_expansion() {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("macro.elx"), "abc".to_string());
        let mut provenance = ProvenanceTable::new();
        let definition = provenance.register_physical(span(file, 0, 1));
        let invocation = provenance.register_physical(span(file, 2, 3));
        let expansion = provenance.register_expansion(invocation, definition);
        let first =
            provenance.register_generated(expansion, GeneratedSource::Definition(definition));
        let second =
            provenance.register_generated(expansion, GeneratedSource::Invocation(invocation));

        assert!(matches!(
            provenance.origin(first),
            Origin::Generated {
                output_index: 0,
                ..
            }
        ));
        assert!(matches!(
            provenance.origin(second),
            Origin::Generated {
                output_index: 1,
                ..
            }
        ));
    }
}
