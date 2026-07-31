use std::path::PathBuf;

use elamite::expansion::fragment::parse_fragment;
use elamite::expansion::provenance::{GeneratedSource, ProvenanceTable};
use elamite::expansion::token_tree::{
    DelimiterKind, TokenTree, TokenTreeToken, TokenTreeUnit, build_token_trees,
};
use elamite::lexer::lex;
use elamite::parser::{FragmentKind, ParseOutput, SyntaxKind};
use elamite::source::{SourceManager, Span};
use elamite::syntax::TokenKind;

fn build(source: &str) -> (TokenTreeUnit, ProvenanceTable) {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from("fragment.elx"), source.to_string());
    let lexed = lex(file, source);
    assert!(
        lexed.diagnostics.is_empty(),
        "unexpected lexer diagnostics for {source:?}: {:?}",
        lexed.diagnostics
    );
    let mut provenance = ProvenanceTable::new();
    let unit = build_token_trees(
        Span::new(file, 0, source.len() as u32),
        source.to_string(),
        &lexed.tokens,
        &mut provenance,
    );
    (unit, provenance)
}

fn parse_source(kind: FragmentKind, source: &str) -> ParseOutput {
    if matches!(
        kind,
        FragmentKind::Expression | FragmentKind::Pattern | FragmentKind::Type
    ) {
        let wrapped = format!("({source})");
        let (unit, provenance) = build(&wrapped);
        let group = match &unit.trees[0] {
            TokenTree::Delimited(group) if group.delimiter == DelimiterKind::Parenthesis => group,
            tree => panic!("expected parenthesized fragment, found {tree:?}"),
        };
        let boundary = group
            .close
            .as_ref()
            .expect("the test wrapper is balanced")
            .origin;
        parse_fragment(&group.children, boundary, kind, &provenance)
            .expect("physical token trees can be parsed")
    } else {
        let (unit, provenance) = build(source);
        let boundary = unit.eof.as_ref().expect("the lexer emits EOF").origin;
        parse_fragment(&unit.trees, boundary, kind, &provenance)
            .expect("physical token trees can be parsed")
    }
}

#[test]
fn parses_each_complete_fragment_role_from_token_trees() {
    let cases = [
        (
            FragmentKind::Expression,
            "left + right * 2",
            SyntaxKind::BinaryExpression,
        ),
        (
            FragmentKind::Statement,
            "if ready:\n    return value\n",
            SyntaxKind::IfStatement,
        ),
        (
            FragmentKind::Pattern,
            "Point { x, .. }",
            SyntaxKind::Pattern,
        ),
        (
            FragmentKind::Type,
            "&fn((i32, i32)) -> Result[i32, Error]",
            SyntaxKind::Type,
        ),
        (
            FragmentKind::Item,
            "pub fn identity(value: i32) -> i32:\n    return value\n",
            SyntaxKind::Function,
        ),
    ];

    for (kind, source, expected) in cases {
        let output = parse_source(kind, source);
        assert!(
            output.diagnostics.is_empty(),
            "{kind:?} {source:?}: {:?}",
            output.diagnostics
        );
        assert_eq!(output.tree.kind, expected, "{kind:?} {source:?}");
    }
}

#[test]
fn every_fragment_role_rejects_trailing_tokens() {
    let cases = [
        (FragmentKind::Expression, "left right"),
        (FragmentKind::Statement, "pass\npass\n"),
        (FragmentKind::Pattern, "Some(value) other"),
        (FragmentKind::Type, "Result[i32] Error"),
        (FragmentKind::Item, "type First = i32\ntype Second = i32\n"),
    ];

    for (kind, source) in cases {
        let output = parse_source(kind, source);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unexpected trailing tokens")),
            "{kind:?} {source:?}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn every_fragment_role_recovers_from_malformed_input() {
    let cases = [
        (FragmentKind::Expression, "left +"),
        (FragmentKind::Statement, "let value =\n"),
        (FragmentKind::Pattern, "Point { field: }"),
        (FragmentKind::Type, "fn(i32)"),
        (FragmentKind::Item, "fn missing() ->\n"),
    ];

    for (kind, source) in cases {
        let output = parse_source(kind, source);
        assert!(
            !output.diagnostics.is_empty(),
            "{kind:?} unexpectedly accepted {source:?}"
        );
    }
}

#[test]
fn required_value_fragments_reject_empty_input() {
    for kind in [
        FragmentKind::Expression,
        FragmentKind::Pattern,
        FragmentKind::Type,
    ] {
        let output = parse_source(kind, "");
        assert!(!output.diagnostics.is_empty(), "{kind:?}");
    }
}

#[test]
fn generated_origins_are_not_projected_onto_physical_parser_spans() {
    let (unit, mut provenance) = build("(definition invocation)");
    let group = match &unit.trees[0] {
        TokenTree::Delimited(group) => group,
        tree => panic!("expected parenthesized input, found {tree:?}"),
    };
    let definition = group.children[0].origin().first;
    let invocation = group.children[1].origin().first;
    let boundary = group.close.as_ref().expect("balanced input").origin;
    let expansion = provenance.register_expansion(invocation, definition);
    let generated = TokenTreeToken::generated(
        TokenKind::Identifier("temporary".to_string()),
        "temporary".to_string(),
        expansion,
        GeneratedSource::Definition(definition),
        &mut provenance,
    );
    let generated_origin = generated.origin;

    let error = parse_fragment(
        &[TokenTree::Token(generated)],
        boundary,
        FragmentKind::Expression,
        &provenance,
    )
    .expect_err("generated origins must remain distinct from physical spans");

    assert_eq!(error.origin, generated_origin);
}
