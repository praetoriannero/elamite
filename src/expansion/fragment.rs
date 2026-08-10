//! Complete-fragment parsing at the token-tree expansion boundary.
//!
//! The ordinary hand-written parser remains the sole owner of Elamite
//! grammar. This adapter flattens a token-tree slice without discarding group
//! delimiters, restores exact physical tokens through the provenance table,
//! and invokes the requested strict fragment entry point.
//!
//! The current downstream syntax tree stores physical [`Span`] values, so this
//! boundary deliberately rejects generated origins instead of projecting them
//! onto definition or invocation bytes. Later generated-syntax integration
//! will remove that temporary representation restriction without weakening the
//! owned package boundary established here.

use std::fmt;

use crate::config::SemanticRevision;
use crate::expansion::provenance::{OriginId, ProvenanceTable};
use crate::expansion::token_tree::{TokenTree, flatten_token_trees};
use crate::parser::{FragmentKind, ParseOutput};
use crate::source::Span;
use crate::syntax::{Token, TokenKind};

/// A token-tree fragment cannot yet be represented by the physical-span syntax
/// tree consumed by ordinary semantic passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedFragmentOrigin {
    pub origin: OriginId,
}

impl fmt::Display for GeneratedFragmentOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "generated token origin {:?} requires origin-aware expanded syntax",
            self.origin
        )
    }
}

impl std::error::Error for GeneratedFragmentOrigin {}

/// Parses exactly one fragment from a token-tree slice.
///
/// `boundary` identifies the position immediately after the fragment, such as
/// its removed outer closing delimiter or a source unit's EOF token. The
/// fragment slice itself must not contain EOF.
pub fn parse_fragment(
    trees: &[TokenTree],
    boundary: OriginId,
    kind: FragmentKind,
    provenance: &ProvenanceTable,
) -> Result<ParseOutput, GeneratedFragmentOrigin> {
    parse_fragment_for_revision(
        trees,
        boundary,
        kind,
        provenance,
        SemanticRevision::default(),
    )
}

pub fn parse_fragment_for_revision(
    trees: &[TokenTree],
    boundary: OriginId,
    kind: FragmentKind,
    provenance: &ProvenanceTable,
    revision: SemanticRevision,
) -> Result<ParseOutput, GeneratedFragmentOrigin> {
    let mut tokens = flatten_token_trees(trees)
        .into_iter()
        .map(|token| {
            provenance
                .physical_span(token.origin)
                .map(|span| Token {
                    kind: token.kind.clone(),
                    span,
                })
                .ok_or(GeneratedFragmentOrigin {
                    origin: token.origin,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let boundary_span = provenance
        .physical_span(boundary)
        .ok_or(GeneratedFragmentOrigin { origin: boundary })?;
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(boundary_span.file, boundary_span.start, boundary_span.start),
    });
    Ok(crate::parser::parse_fragment_for_revision(
        &tokens, kind, revision,
    ))
}
