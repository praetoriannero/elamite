use std::path::PathBuf;

use elamite::diagnostics::Category;
use elamite::lexer::{Token, TokenKind, lex};
use elamite::source::SourceManager;
use proptest::prelude::*;

fn lex_text(source: &str) -> elamite::lexer::LexOutput {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from("test.elx"), source.to_string());
    lex(file, source)
}

fn render(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|token| {
            format!(
                "{:?} @ {}..{}",
                token.kind, token.span.start, token.span.end
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn count(tokens: &[Token], predicate: impl Fn(&TokenKind) -> bool) -> usize {
    tokens.iter().filter(|token| predicate(&token.kind)).count()
}

#[test]
fn malformed_formatted_interpolation_recovers_across_unicode_escape_text() {
    let output = lex_text("''f\"{'\\¡");
    assert!(matches!(
        output.tokens.last().map(|token| &token.kind),
        Some(TokenKind::Eof)
    ));
}

#[test]
fn snapshots_nested_blocks_and_eof_dedents() {
    let output = lex_text(
        "if true:\n\
             \x20\x20\x20\x20println(\"yes\")\n\
         else:\n\
             \x20\x20\x20\x20pass\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    insta::assert_snapshot!(render(&output.tokens));
}

#[test]
fn snapshots_statement_continuation() {
    let output = lex_text(
        "let total = 1\n\
             \x20\x20\x20\x20+ 2\n\
         let next = 3\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Newline)),
        2
    );
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Indent)),
        0
    );
    insta::assert_snapshot!(render(&output.tokens));
}

#[test]
fn snapshots_grouped_multiline_expression() {
    let output = lex_text("let values = [\n        1,\n  2,\n]\nlet done = true\n");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Newline)),
        2
    );
    insta::assert_snapshot!(render(&output.tokens));
}

#[test]
fn preserves_documentation_and_discards_ordinary_comments() {
    let output = lex_text(
        "// ordinary\n\
         /// Documentation\n\
         struct Example:\n\
             \x20\x20\x20\x20// body comment\n\
             \x20\x20\x20\x20pass\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(
            kind,
            TokenKind::DocComment(_)
        )),
        1
    );
    insta::assert_snapshot!(render(&output.tokens));
}

#[test]
fn recognizes_and_decodes_literals() {
    let output = lex_text(
        "let mask = 0xffu8\n\
         let count = 1_024i32\n\
         let ratio = 1.25e+2f64\n\
         let text = \"line\\nemoji: \\u{1f642}\"\n\
         let scalar = '\\u{3bb}'\n\
         let formatted = f\"{{value}} = {count + 1}\"\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    insta::assert_snapshot!(render(&output.tokens));
}

#[test]
fn recognizes_every_reserved_keyword() {
    let source = "as break continue defer else enum false fn for if impl import in \
                  let match mod null pass pub return root self Self struct super trait true \
                  type unsafe var while\n";
    let output = lex_text(source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Keyword(_))),
        31
    );
    assert_eq!(
        count(&output.tokens, |kind| matches!(
            kind,
            TokenKind::Identifier(_)
        )),
        0
    );
}

#[test]
fn removed_and_contextual_words_are_ordinary_identifiers() {
    // `dyn` was removed with the trait-object syntax change, and `std` names
    // the standard-library package as an ordinary name. `extern` was removed
    // when C interop moved to item attributes.
    let output = lex_text("dyn std extern\n");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Keyword(_))),
        0
    );
    assert_eq!(
        count(&output.tokens, |kind| matches!(
            kind,
            TokenKind::Identifier(_)
        )),
        3
    );
}

#[test]
fn recognizes_surface_punctuation_and_operators() {
    let output = lex_text(
        "( ) [ ] { } , ; : . .. ... @ ? -> = + - * / % & | ^ ~ ! << >> == != <= >= < > && || \
         += -= *= /= %= &= |= ^= <<= >>=\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    insta::assert_snapshot!(render(&output.tokens));
}

#[test]
fn body_colon_ends_a_continued_header() {
    let output = lex_text(
        "if enabled\n\
             \x20\x20\x20\x20&& ready:\n\
             \x20\x20\x20\x20pass\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Indent)),
        1
    );
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Dedent)),
        1
    );
}

#[test]
fn reports_indentation_failures_and_keeps_lexing() {
    let output = lex_text(
        "if true:\n\
         \tpass\n\
         \x20\x20let bad = 1\n\
         let recovered = 2\n",
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::LexicalIndentation)
    );
    assert!(
        output
            .tokens
            .iter()
            .any(|token| matches!(&token.kind, TokenKind::Identifier(name) if name == "recovered"))
    );
}

#[test]
fn reports_literal_failures_and_keeps_lexing() {
    let output = lex_text(
        "let bad = \"unterminated\n\
         let also_bad = 1__0\n\
         let escape = \"\\q\"\n\
         let format = f\"bad } brace\"\n\
         let recovered = 2\n",
    );
    assert!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::LexicalLiteral)
            .count()
            >= 4
    );
    assert!(
        output
            .tokens
            .iter()
            .any(|token| matches!(&token.kind, TokenKind::Identifier(name) if name == "recovered"))
    );
}

#[test]
fn reports_invalid_prefixed_digits_and_missing_body() {
    let output = lex_text("let binary = 0b102\nif true:");
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::LexicalLiteral)
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::LexicalIndentation)
    );
}

#[test]
fn reports_mismatched_and_unclosed_delimiters() {
    let output = lex_text("let first = (1]\nlet second = [2\nlet format = f\"{(3]}\"\n");
    assert!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::LexicalDelimiter)
            .count()
            >= 2
    );
}

#[test]
fn rejects_non_ascii_identifier_characters() {
    let output = lex_text("let café = 1\nlet recovered = 2\n");
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::LexicalCharacter)
    );
    assert!(
        output
            .tokens
            .iter()
            .any(|token| matches!(&token.kind, TokenKind::Identifier(name) if name == "recovered"))
    );
}

#[test]
fn accepts_crlf_line_endings() {
    let output = lex_text("if true:\r\n    pass\r\n");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Indent)),
        1
    );
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Dedent)),
        1
    );
}

#[test]
fn unsupported_unicode_escape_recovers_at_a_character_boundary() {
    let output = lex_text("'\\¡");
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::LexicalLiteral)
    );
    assert!(matches!(
        output.tokens.first().map(|token| &token.kind),
        Some(TokenKind::CharacterLiteral(_))
    ));
}

#[test]
fn authoritative_demo_lexes_without_errors() {
    let source = include_str!("../examples/spec_demo.elx");
    let output = lex_text(source);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert!(
        output
            .tokens
            .iter()
            .all(|token| token.span.start <= token.span.end
                && token.span.end as usize <= source.len())
    );
    assert_eq!(
        count(&output.tokens, |kind| matches!(kind, TokenKind::Indent)),
        count(&output.tokens, |kind| matches!(kind, TokenKind::Dedent))
    );
}

proptest! {
    #[test]
    fn accepted_nested_blocks_balance_layout_tokens(depth in 0usize..12) {
        let mut source = String::new();
        for level in 0..depth {
            source.push_str(&" ".repeat(level * 4));
            source.push_str("if true:\n");
        }
        source.push_str(&" ".repeat(depth * 4));
        source.push_str("pass\n");

        let output = lex_text(&source);
        prop_assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let indents = count(&output.tokens, |kind| matches!(kind, TokenKind::Indent));
        let dedents = count(&output.tokens, |kind| matches!(kind, TokenKind::Dedent));
        prop_assert_eq!(indents, depth);
        prop_assert_eq!(dedents, depth);
    }
}
