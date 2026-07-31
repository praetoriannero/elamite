//! Lossless token-tree representation for macro expansion.
//!
//! Token trees group balanced delimiters without assigning Elamite syntax
//! meaning to their contents. Layout tokens remain ordinary leaves, and each
//! physical leaf retains the lexer kind, exact source slice, and stable origin.
//! Delimiter diagnostics remain owned by the lexer; this builder recovers
//! structurally and never discards a token after malformed input.

use crate::expansion::provenance::{
    ExpansionId, GeneratedSource, OriginId, OriginRange, ProvenanceTable,
};
use crate::source::{FileId, Span};
use crate::syntax::{Token, TokenKind};

/// One of the three balanced source delimiter kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelimiterKind {
    Parenthesis,
    Bracket,
    Brace,
}

impl DelimiterKind {
    fn opening(kind: &TokenKind) -> Option<Self> {
        match kind {
            TokenKind::LParen => Some(Self::Parenthesis),
            TokenKind::LBracket => Some(Self::Bracket),
            TokenKind::LBrace => Some(Self::Brace),
            _ => None,
        }
    }

    fn closing(kind: &TokenKind) -> Option<Self> {
        match kind {
            TokenKind::RParen => Some(Self::Parenthesis),
            TokenKind::RBracket => Some(Self::Bracket),
            TokenKind::RBrace => Some(Self::Brace),
            _ => None,
        }
    }
}

/// A physical or generated token represented at the expansion boundary.
///
/// Physical layout and EOF tokens may have an empty source spelling. Generated
/// tokens carry emitted source text but deliberately have no fabricated
/// physical span; their origin identifies an expansion-local output location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTreeToken {
    pub kind: TokenKind,
    pub source: String,
    pub origin: OriginId,
}

impl TokenTreeToken {
    fn physical(
        token: &Token,
        source: &str,
        file: FileId,
        provenance: &mut ProvenanceTable,
    ) -> Self {
        let text = if token.span.file == file {
            source
                .get(token.span.start as usize..token.span.end as usize)
                .unwrap_or_default()
        } else {
            ""
        };
        Self {
            kind: token.kind.clone(),
            source: text.to_string(),
            origin: provenance.register_physical(token.span),
        }
    }

    /// Constructs a generated token with an expansion-local location.
    ///
    /// No physical [`Span`] is fabricated. `source` is the emitted spelling,
    /// while `generated_from` retains the exact definition-side literal or
    /// invocation-side capture that produced it.
    pub fn generated(
        kind: TokenKind,
        source: String,
        expansion: ExpansionId,
        generated_from: GeneratedSource,
        provenance: &mut ProvenanceTable,
    ) -> Self {
        Self {
            kind,
            source,
            origin: provenance.register_generated(expansion, generated_from),
        }
    }
}

/// A balanced or recoverably unclosed delimited token tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelimitedTokenTree {
    pub delimiter: DelimiterKind,
    pub origin: OriginRange,
    pub open: TokenTreeToken,
    pub children: Vec<TokenTree>,
    pub close: Option<TokenTreeToken>,
}

/// One source token or one nested delimited group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenTree {
    Token(TokenTreeToken),
    Delimited(DelimitedTokenTree),
}

impl TokenTree {
    #[must_use]
    pub fn origin(&self) -> OriginRange {
        match self {
            Self::Token(token) => OriginRange::new(token.origin, token.origin),
            Self::Delimited(group) => group.origin,
        }
    }

    fn append_flattened<'a>(&'a self, output: &mut Vec<&'a TokenTreeToken>) {
        match self {
            Self::Token(token) => output.push(token),
            Self::Delimited(group) => {
                output.push(&group.open);
                for child in &group.children {
                    child.append_flattened(output);
                }
                if let Some(close) = &group.close {
                    output.push(close);
                }
            }
        }
    }

    fn dump_into(&self, depth: usize, output: &mut String) {
        let indent = "  ".repeat(depth);
        match self {
            Self::Token(token) => {
                output.push_str(&format!(
                    "{indent}{:?} {:?} @ {:?}\n",
                    token.kind, token.source, token.origin
                ));
            }
            Self::Delimited(group) => {
                let closure = if group.close.is_some() {
                    "closed"
                } else {
                    "unclosed"
                };
                output.push_str(&format!(
                    "{indent}{:?} {closure} @ {:?}..{:?}\n",
                    group.delimiter, group.origin.first, group.origin.last
                ));
                output.push_str(&format!(
                    "{indent}  open {:?} @ {:?}\n",
                    group.open.source, group.open.origin
                ));
                for child in &group.children {
                    child.dump_into(depth + 1, output);
                }
                if let Some(close) = &group.close {
                    output.push_str(&format!(
                        "{indent}  close {:?} @ {:?}\n",
                        close.source, close.origin
                    ));
                }
            }
        }
    }
}

/// The token-tree view of one complete parsed source unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTreeUnit {
    pub file: FileId,
    pub span: Span,
    /// Complete physical source, including whitespace and comments discarded
    /// by ordinary lexing.
    pub source: String,
    pub trees: Vec<TokenTree>,
    pub eof: Option<TokenTreeToken>,
}

impl TokenTreeUnit {
    /// Returns every represented token in original source order.
    ///
    /// Opening and closing delimiters are included, followed by EOF when the
    /// lexer supplied it. This is primarily an equivalence and recovery check:
    /// flattening a tree built from lexer output must reproduce that output.
    #[must_use]
    pub fn flattened(&self) -> Vec<&TokenTreeToken> {
        let mut output = flatten_token_trees(&self.trees);
        if let Some(eof) = &self.eof {
            output.push(eof);
        }
        output
    }

    /// Produces a deterministic structural representation for focused tests
    /// and future expansion inspection.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = format!("TokenTreeUnit @ {}..{}\n", self.span.start, self.span.end);
        for tree in &self.trees {
            tree.dump_into(1, &mut output);
        }
        if let Some(eof) = &self.eof {
            output.push_str(&format!("  Eof {:?} @ {:?}\n", eof.source, eof.origin));
        }
        output
    }
}

/// Returns every token in `trees`, including group delimiters, in source order.
#[must_use]
pub fn flatten_token_trees(trees: &[TokenTree]) -> Vec<&TokenTreeToken> {
    let mut output = Vec::new();
    for tree in trees {
        tree.append_flattened(&mut output);
    }
    output
}

struct OpenGroup {
    delimiter: DelimiterKind,
    open: TokenTreeToken,
    children: Vec<TokenTree>,
}

fn group_origin(
    open: &TokenTreeToken,
    children: &[TokenTree],
    close: Option<&TokenTreeToken>,
) -> OriginRange {
    let last = close.map_or_else(
        || {
            children
                .last()
                .map_or(open.origin, |child| child.origin().last)
        },
        |close| close.origin,
    );
    OriginRange::new(open.origin, last)
}

fn append_tree(tree: TokenTree, stack: &mut [OpenGroup], roots: &mut Vec<TokenTree>) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(tree);
    } else {
        roots.push(tree);
    }
}

/// Builds a lossless nested view over one lexer's token stream.
///
/// A closer nests only when it matches the currently open delimiter. A
/// mismatched or unmatched closer remains a leaf, while an opener left at EOF
/// becomes an unclosed group. The lexer already owns user-facing delimiter
/// diagnostics, so recovery here adds no duplicate diagnostics.
#[must_use]
pub fn build_token_trees(
    span: Span,
    source: String,
    tokens: &[Token],
    provenance: &mut ProvenanceTable,
) -> TokenTreeUnit {
    let file = span.file;
    let mut roots = Vec::new();
    let mut stack = Vec::<OpenGroup>::new();
    let mut eof = None;

    for token in tokens {
        let represented = TokenTreeToken::physical(token, &source, file, provenance);
        if matches!(token.kind, TokenKind::Eof) {
            eof = Some(represented);
            continue;
        }

        if let Some(delimiter) = DelimiterKind::opening(&token.kind) {
            stack.push(OpenGroup {
                delimiter,
                open: represented,
                children: Vec::new(),
            });
            continue;
        }

        if let Some(delimiter) = DelimiterKind::closing(&token.kind) {
            if stack.last().is_some_and(|open| open.delimiter == delimiter) {
                let open = stack.pop().expect("matching group is present");
                let group = DelimitedTokenTree {
                    delimiter,
                    origin: group_origin(&open.open, &open.children, Some(&represented)),
                    open: open.open,
                    children: open.children,
                    close: Some(represented),
                };
                append_tree(TokenTree::Delimited(group), &mut stack, &mut roots);
            } else {
                append_tree(TokenTree::Token(represented), &mut stack, &mut roots);
            }
            continue;
        }

        append_tree(TokenTree::Token(represented), &mut stack, &mut roots);
    }

    while let Some(open) = stack.pop() {
        let group = DelimitedTokenTree {
            delimiter: open.delimiter,
            origin: group_origin(&open.open, &open.children, None),
            open: open.open,
            children: open.children,
            close: None,
        };
        append_tree(TokenTree::Delimited(group), &mut stack, &mut roots);
    }

    TokenTreeUnit {
        file,
        span,
        source,
        trees: roots,
        eof,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::lexer::lex;
    use crate::source::SourceManager;

    use super::*;

    fn build(source: &str) -> (crate::lexer::LexOutput, TokenTreeUnit, ProvenanceTable) {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("tokens.elx"), source.to_string());
        let lexed = lex(file, source);
        let span = Span::new(file, 0, source.len() as u32);
        let mut provenance = ProvenanceTable::new();
        let trees = build_token_trees(span, source.to_string(), &lexed.tokens, &mut provenance);
        (lexed, trees, provenance)
    }

    fn group_count(trees: &[TokenTree]) -> usize {
        trees
            .iter()
            .map(|tree| match tree {
                TokenTree::Token(_) => 0,
                TokenTree::Delimited(group) => 1 + group_count(&group.children),
            })
            .sum()
    }

    #[test]
    fn nests_delimiters_without_parsing_their_contents() {
        let (lexed, trees, _) = build("let value = call((one, [two, {three: four}]))\n");
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        assert_eq!(group_count(&trees.trees), 4);
        assert!(trees.dump().contains("Brace closed"));
        assert!(trees.dump().contains("Identifier(\"three\")"));
    }

    #[test]
    fn flattening_reproduces_tokens_and_exact_source_slices() {
        let source = "/// Døc\nfn value() -> ():\n    println(\"λ\")\n";
        let (lexed, trees, provenance) = build(source);
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        assert_eq!(trees.source, source);

        let flattened = trees.flattened();
        assert_eq!(flattened.len(), lexed.tokens.len());
        for (represented, original) in flattened.into_iter().zip(&lexed.tokens) {
            assert_eq!(&represented.kind, &original.kind);
            assert_eq!(
                provenance.physical_span(represented.origin),
                Some(original.span)
            );
            assert_eq!(
                represented.source,
                source
                    .get(original.span.start as usize..original.span.end as usize)
                    .unwrap_or_default()
            );
        }
    }

    #[test]
    fn layout_tokens_remain_ordered_leaves() {
        let (lexed, trees, _) = build("if true:\n    println(\"yes\")\n");
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        let kinds = trees
            .flattened()
            .into_iter()
            .map(|token| &token.kind)
            .collect::<Vec<_>>();
        assert!(kinds.iter().any(|kind| matches!(kind, TokenKind::Indent)));
        assert!(kinds.iter().any(|kind| matches!(kind, TokenKind::Dedent)));
        assert!(kinds.iter().any(|kind| matches!(kind, TokenKind::Newline)));
    }

    #[test]
    fn malformed_delimiters_recover_without_losing_tokens() {
        let (lexed, trees, _) = build("let value = (one]\n");
        assert!(!lexed.diagnostics.is_empty());
        assert_eq!(trees.flattened().len(), lexed.tokens.len());

        let group = trees
            .trees
            .iter()
            .find_map(|tree| match tree {
                TokenTree::Delimited(group) => Some(group),
                TokenTree::Token(_) => None,
            })
            .expect("the unclosed parenthesis remains represented");
        assert_eq!(group.delimiter, DelimiterKind::Parenthesis);
        assert!(group.close.is_none());
        assert!(group.children.iter().any(|child| {
            matches!(
                child,
                TokenTree::Token(token) if matches!(token.kind, TokenKind::RBracket)
            )
        }));
    }

    #[test]
    fn generated_tokens_have_expansion_origins_without_physical_spans() {
        let (lexed, trees, mut provenance) = build("definition invocation\n");
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        let flattened = trees.flattened();
        let definition = flattened[0].origin;
        let invocation = flattened[1].origin;
        let expansion = provenance.register_expansion(invocation, definition);

        let generated = TokenTreeToken::generated(
            TokenKind::Identifier("temporary".to_string()),
            "temporary".to_string(),
            expansion,
            GeneratedSource::Definition(definition),
            &mut provenance,
        );

        assert_eq!(provenance.physical_span(generated.origin), None);
        assert_eq!(
            provenance.trace(generated.origin).source_chain,
            [generated.origin, definition]
        );
        assert_eq!(
            provenance.trace(generated.origin).expansions,
            [crate::expansion::provenance::ExpansionTraceFrame {
                expansion,
                invocation,
                definition,
            }]
        );
    }
}
