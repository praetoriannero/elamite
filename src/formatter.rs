//! Deterministic source formatting for Elamite files.
//!
//! Formatting is deliberately syntax-only: it validates lexing and parsing,
//! preserves every significant token and comment, and never runs resolution or
//! type checking.

use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::lex;
use crate::parser::parse;
use crate::source::FileId;
use crate::syntax::{FormattedSegmentKind, Keyword, Token, TokenKind};

pub const DEFAULT_LINE_LENGTH: usize = 100;
pub const INDENT_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub line_length: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            line_length: DEFAULT_LINE_LENGTH,
        }
    }
}

#[derive(Clone, Copy)]
struct Piece<'a> {
    token: &'a Token,
    text: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    Parenthesis,
    Bracket,
    Brace,
}

impl Delimiter {
    fn open(kind: &TokenKind) -> Option<Self> {
        match kind {
            TokenKind::LParen => Some(Self::Parenthesis),
            TokenKind::LBracket => Some(Self::Bracket),
            TokenKind::LBrace => Some(Self::Brace),
            _ => None,
        }
    }

    fn close(kind: &TokenKind) -> Option<Self> {
        match kind {
            TokenKind::RParen => Some(Self::Parenthesis),
            TokenKind::RBracket => Some(Self::Bracket),
            TokenKind::RBrace => Some(Self::Brace),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OpenDelimiter {
    kind: Delimiter,
    indent: usize,
}

/// Formats one complete, valid Elamite source file.
///
/// Lexical or syntax diagnostics are returned without producing replacement
/// text, allowing callers to guarantee that invalid files remain untouched.
pub fn format_source(
    file: FileId,
    source: &str,
    options: FormatOptions,
) -> Result<String, Vec<Diagnostic>> {
    if options.line_length == 0 {
        return Err(vec![Diagnostic::new(
            Category::Formatting,
            "format line length must be greater than zero",
        )]);
    }

    let original = lex(file, source);
    if !original.diagnostics.is_empty() {
        return Err(original.diagnostics);
    }
    let parsed = parse(&original.tokens);
    if !parsed.diagnostics.is_empty() {
        return Err(parsed.diagnostics);
    }
    if source.is_empty() {
        return Ok(String::new());
    }

    let mut output_lines = Vec::new();
    let mut delimiters = Vec::<OpenDelimiter>::new();
    let mut line_start = 0usize;
    for raw_line in source.split_inclusive('\n') {
        let raw_without_newline = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let content = raw_without_newline
            .strip_suffix('\r')
            .unwrap_or(raw_without_newline);
        let line_end = line_start + content.len();
        let pieces = line_pieces(&original.tokens, source, line_start, line_end);
        let original_indent = content
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        let first_kind = pieces.first().map(|piece| &piece.token.kind);
        let indent = match delimiters.last() {
            Some(open) if first_kind.and_then(Delimiter::close) == Some(open.kind) => open.indent,
            Some(open) => open.indent + INDENT_WIDTH,
            None => original_indent,
        };

        let comment = trailing_comment(content, line_start, &pieces);
        if pieces.is_empty() {
            if let Some(comment) = comment {
                let comment_indent = delimiters
                    .last()
                    .map_or(original_indent, |open| open.indent + INDENT_WIDTH);
                output_lines.push(format!("{}{comment}", " ".repeat(comment_indent)));
            } else {
                output_lines.push(String::new());
            }
        } else if matches!(pieces[0].token.kind, TokenKind::DocComment(_)) {
            output_lines.push(format!(
                "{}{}",
                " ".repeat(indent),
                pieces[0].text.trim_end()
            ));
        } else {
            let mut lines = layout_pieces(&pieces, indent, options.line_length);
            if let Some(comment) = comment
                && let Some(last) = lines.last_mut()
            {
                last.push_str("  ");
                last.push_str(comment);
            }
            output_lines.extend(lines);
        }

        update_delimiters(&pieces, indent, &mut delimiters);
        line_start += raw_line.len();
    }

    let mut formatted = output_lines.join("\n");
    while formatted.ends_with("\n\n\n") {
        formatted.pop();
    }
    if !formatted.ends_with('\n') {
        formatted.push('\n');
    }

    let reformatted = lex(file, &formatted);
    if !reformatted.diagnostics.is_empty() {
        return Err(vec![Diagnostic::new(
            Category::Formatting,
            "formatter produced lexically invalid output",
        )]);
    }
    let reparsed = parse(&reformatted.tokens);
    if !reparsed.diagnostics.is_empty() {
        return Err(vec![Diagnostic::new(
            Category::Formatting,
            "formatter produced syntactically invalid output",
        )]);
    }
    if original.tokens.len() != reformatted.tokens.len()
        || !original
            .tokens
            .iter()
            .zip(&reformatted.tokens)
            .all(|(before, after)| equivalent_kind(&before.kind, &after.kind))
    {
        return Err(vec![Diagnostic::new(
            Category::Formatting,
            "formatter changed the source token stream",
        )]);
    }

    Ok(formatted)
}

fn equivalent_kind(left: &TokenKind, right: &TokenKind) -> bool {
    match (left, right) {
        (TokenKind::FormattedString(left), TokenKind::FormattedString(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| match (&left.kind, &right.kind) {
                        (FormattedSegmentKind::Text(left), FormattedSegmentKind::Text(right)) => {
                            left == right
                        }
                        (
                            FormattedSegmentKind::Expression {
                                source: left_source,
                                tokens: left_tokens,
                            },
                            FormattedSegmentKind::Expression {
                                source: right_source,
                                tokens: right_tokens,
                            },
                        ) => {
                            left_source == right_source
                                && left_tokens.len() == right_tokens.len()
                                && left_tokens
                                    .iter()
                                    .zip(right_tokens)
                                    .all(|(left, right)| equivalent_kind(&left.kind, &right.kind))
                        }
                        _ => false,
                    })
        }
        _ => left == right,
    }
}

fn is_layout(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
    )
}

fn line_pieces<'a>(
    tokens: &'a [Token],
    source: &'a str,
    line_start: usize,
    line_end: usize,
) -> Vec<Piece<'a>> {
    tokens
        .iter()
        .filter(|token| {
            !is_layout(&token.kind)
                && token.span.start as usize >= line_start
                && (token.span.start as usize) < line_end
                && (token.span.end as usize) <= line_end
        })
        .map(|token| Piece {
            token,
            text: &source[token.span.start as usize..token.span.end as usize],
        })
        .collect()
}

fn trailing_comment<'a>(line: &'a str, line_start: usize, pieces: &[Piece<'_>]) -> Option<&'a str> {
    if matches!(
        pieces.first().map(|piece| &piece.token.kind),
        Some(TokenKind::DocComment(_))
    ) {
        return None;
    }
    let search_start = pieces
        .last()
        .map_or(0, |piece| piece.token.span.end as usize - line_start);
    line.get(search_start..)?
        .find("//")
        .map(|offset| line[search_start + offset..].trim_end())
}

fn update_delimiters(
    pieces: &[Piece<'_>],
    line_indent: usize,
    delimiters: &mut Vec<OpenDelimiter>,
) {
    for piece in pieces {
        if let Some(kind) = Delimiter::open(&piece.token.kind) {
            delimiters.push(OpenDelimiter {
                kind,
                indent: line_indent,
            });
        } else if let Some(kind) = Delimiter::close(&piece.token.kind)
            && delimiters.last().is_some_and(|open| open.kind == kind)
        {
            delimiters.pop();
        }
    }
}

fn layout_pieces(pieces: &[Piece<'_>], indent: usize, line_length: usize) -> Vec<String> {
    let flat = format_flat(pieces);
    if indent + flat.chars().count() <= line_length {
        return vec![format!("{}{flat}", " ".repeat(indent))];
    }

    let Some((open, close)) = wrapping_pair(pieces) else {
        return vec![format!("{}{flat}", " ".repeat(indent))];
    };
    let prefix = format_flat(&pieces[..=open]);
    let suffix = format_flat(&pieces[close..]);
    let mut lines = vec![format!("{}{prefix}", " ".repeat(indent))];
    let child_indent = indent + INDENT_WIDTH;
    for (start, end) in comma_segments(pieces, open + 1, close) {
        lines.extend(layout_pieces(
            &pieces[start..end],
            child_indent,
            line_length,
        ));
    }
    lines.push(format!("{}{suffix}", " ".repeat(indent)));
    lines
}

fn wrapping_pair(pieces: &[Piece<'_>]) -> Option<(usize, usize)> {
    let mut stack = Vec::<(Delimiter, usize, usize)>::new();
    let mut candidates = Vec::<(usize, bool, usize, usize)>::new();
    for (index, piece) in pieces.iter().enumerate() {
        if let Some(kind) = Delimiter::open(&piece.token.kind) {
            stack.push((kind, index, stack.len()));
        } else if let Some(kind) = Delimiter::close(&piece.token.kind)
            && let Some((open_kind, open, depth)) = stack.pop()
            && open_kind == kind
            && open + 1 < index
        {
            let has_comma = contains_top_level_comma(pieces, open + 1, index);
            candidates.push((depth, !has_comma, open, index));
        }
    }
    candidates.sort();
    candidates
        .first()
        .map(|(_, _, open, close)| (*open, *close))
}

fn contains_top_level_comma(pieces: &[Piece<'_>], start: usize, end: usize) -> bool {
    let mut depth = 0usize;
    for piece in &pieces[start..end] {
        if Delimiter::open(&piece.token.kind).is_some() {
            depth += 1;
        } else if Delimiter::close(&piece.token.kind).is_some() {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && matches!(piece.token.kind, TokenKind::Comma) {
            return true;
        }
    }
    false
}

fn comma_segments(pieces: &[Piece<'_>], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut segment_start = start;
    let mut depth = 0usize;
    for (offset, piece) in pieces[start..end].iter().enumerate() {
        let index = start + offset;
        if Delimiter::open(&piece.token.kind).is_some() {
            depth += 1;
        } else if Delimiter::close(&piece.token.kind).is_some() {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && matches!(piece.token.kind, TokenKind::Comma) {
            segments.push((segment_start, index + 1));
            segment_start = index + 1;
        }
    }
    if segment_start < end {
        segments.push((segment_start, end));
    }
    segments
}

fn format_flat(pieces: &[Piece<'_>]) -> String {
    let mut output = String::new();
    for (index, piece) in pieces.iter().enumerate() {
        if index > 0 && needs_space(pieces, index) {
            output.push(' ');
        }
        output.push_str(piece.text);
    }
    output
}

fn needs_space(pieces: &[Piece<'_>], index: usize) -> bool {
    let previous = &pieces[index - 1].token.kind;
    let current = &pieces[index].token.kind;

    if matches!(current, TokenKind::RBrace) && !matches!(previous, TokenKind::LBrace) {
        return true;
    }
    if matches!(
        current,
        TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::Colon
            | TokenKind::Dot
            | TokenKind::Question
    ) || matches!(
        previous,
        TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot | TokenKind::At
    ) {
        return false;
    }
    if matches!(previous, TokenKind::LBrace) {
        return true;
    }
    if matches!(
        current,
        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
    ) {
        if matches!(current, TokenKind::LBrace) {
            return !(index >= 2 && matches!(pieces[index - 2].token.kind, TokenKind::At));
        }
        return !matches!(
            previous,
            TokenKind::Identifier(_)
                | TokenKind::RParen
                | TokenKind::RBracket
                | TokenKind::RBrace
                | TokenKind::At
                | TokenKind::Keyword(
                    Keyword::Fn
                        | Keyword::Root
                        | Keyword::Super
                        | Keyword::SelfValue
                        | Keyword::SelfType
                )
        ) && !is_prefix_operator(pieces, index - 1);
    }
    if matches!(
        previous,
        TokenKind::Comma | TokenKind::Semicolon | TokenKind::Colon
    ) {
        return true;
    }
    if is_prefix_operator(pieces, index - 1) || matches!(previous, TokenKind::Ellipsis) {
        return false;
    }
    true
}

fn is_prefix_operator(pieces: &[Piece<'_>], index: usize) -> bool {
    if !matches!(
        pieces[index].token.kind,
        TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Amp
    ) {
        return false;
    }
    if index == 0 {
        return true;
    }
    matches!(
        pieces[index - 1].token.kind,
        TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::Colon
            | TokenKind::Arrow
            | TokenKind::Assign
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Amp
            | TokenKind::Pipe
            | TokenKind::Caret
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::Less
            | TokenKind::LessEq
            | TokenKind::Greater
            | TokenKind::GreaterEq
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign
            | TokenKind::PercentAssign
            | TokenKind::AmpAssign
            | TokenKind::PipeAssign
            | TokenKind::CaretAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
            | TokenKind::Keyword(_)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::source::SourceManager;

    use super::*;

    fn format(source: &str, line_length: usize) -> String {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("test.elx"), source.to_string());
        format_source(file, source, FormatOptions { line_length })
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:?}"))
    }

    #[test]
    fn normalizes_spacing_and_preserves_comments_and_literals() {
        let source = "// heading\nfn main( )->( ):\n    let value=Point{ x:1,y }\n    println(f\"{value} // literal\") // tail\n";
        let expected = "// heading\nfn main() -> ():\n    let value = Point { x: 1, y }\n    println(f\"{value} // literal\")  // tail\n";
        assert_eq!(format(source, 100), expected);
    }

    #[test]
    fn wraps_delimited_lists_at_the_configured_width() {
        let source =
            "fn calculate(first: i32, second: i32, third: i32) -> i32:\n    return first\n";
        let expected = "fn calculate(\n    first: i32,\n    second: i32,\n    third: i32\n) -> i32:\n    return first\n";
        assert_eq!(format(source, 40), expected);
    }

    #[test]
    fn formatting_is_idempotent() {
        let source = "fn main() -> ():\n    let values = [\n      1,\n             2\n    ]\n";
        let once = format(source, 100);
        assert_eq!(format(&once, 100), once);
    }
}
