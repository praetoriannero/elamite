//! Span-preserving lexer and indentation engine.
//!
//! This module implements `docs/ROADMAP.md` Milestone 2. It recognizes surface tokens
//! and layout but deliberately performs no parsing, name resolution, or type
//! checking.

use crate::diagnostics::{Category, Diagnostic};
use crate::ident::is_valid_identifier;
use crate::source::{FileId, Span};
pub use crate::syntax::{
    FormattedSegment, FormattedSegmentKind, Keyword, NumericSuffix, Token, TokenKind,
};

/// One complete lexing result. Tokens are retained even when diagnostics were
/// produced so later tooling can inspect or recover from the valid remainder.
#[derive(Debug)]
pub struct LexOutput {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    Parenthesis,
    Bracket,
    Brace,
}

impl Delimiter {
    fn closing_name(self) -> &'static str {
        match self {
            Delimiter::Parenthesis => "`)`",
            Delimiter::Bracket => "`]`",
            Delimiter::Brace => "`}`",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OpenDelimiter {
    kind: Delimiter,
    span: Span,
}

#[derive(Debug, Clone, Copy)]
struct PendingNewline {
    statement_base: usize,
    span: Span,
}

/// Lexes one UTF-8 source file.
#[must_use]
pub fn lex(file: FileId, source: &str) -> LexOutput {
    Lexer::new(file, source).run()
}

struct Lexer<'a> {
    file: FileId,
    source: &'a str,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    block_indents: Vec<usize>,
    delimiters: Vec<OpenDelimiter>,
    pending_newline: Option<PendingNewline>,
    active_statement_base: Option<usize>,
    expected_block_base: Option<usize>,
}

impl<'a> Lexer<'a> {
    fn new(file: FileId, source: &'a str) -> Self {
        Self {
            file,
            source,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            block_indents: vec![0],
            delimiters: Vec::new(),
            pending_newline: None,
            active_statement_base: None,
            expected_block_base: None,
        }
    }

    fn run(mut self) -> LexOutput {
        let mut line_start = 0;
        while line_start < self.source.len() {
            let remainder = &self.source[line_start..];
            let (raw_end, newline_end) = match remainder.find('\n') {
                Some(relative) => {
                    let end = line_start + relative;
                    (end, end + 1)
                }
                None => (self.source.len(), self.source.len()),
            };
            let content_end =
                if raw_end > line_start && self.source.as_bytes()[raw_end - 1] == b'\r' {
                    raw_end - 1
                } else {
                    raw_end
                };
            self.lex_physical_line(line_start, content_end, newline_end);
            if newline_end == self.source.len() {
                break;
            }
            line_start = newline_end;
        }

        self.finish();
        LexOutput {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(
            self.file,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
        )
    }

    fn emit(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token {
            kind,
            span: self.span(start, end),
        });
    }

    fn error(&mut self, category: Category, start: usize, end: usize, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(category, message).with_primary(self.span(start, end)));
    }

    fn lex_physical_line(&mut self, line_start: usize, line_end: usize, newline_end: usize) {
        let bytes = self.source.as_bytes();
        let mut content_start = line_start;
        let mut indent = 0usize;
        let mut reported_tab = false;
        while content_start < line_end {
            match bytes[content_start] {
                b' ' => {
                    indent += 1;
                    content_start += 1;
                }
                b'\t' => {
                    if !reported_tab {
                        self.error(
                            Category::LexicalIndentation,
                            content_start,
                            content_start + 1,
                            "a tab is not permitted in leading indentation",
                        );
                        reported_tab = true;
                    }
                    indent = (indent / 4 + 1) * 4;
                    content_start += 1;
                }
                _ => break,
            }
        }

        let rest = &self.source[content_start..line_end];
        if rest.is_empty() || (rest.starts_with("//") && !rest.starts_with("///")) {
            return;
        }

        let is_doc_comment = rest.starts_with("///");
        let grouped_at_start = !self.delimiters.is_empty();
        if !grouped_at_start {
            if self.prepare_logical_line(indent, is_doc_comment, line_start, content_start) {
                // This physical line continues the current logical statement.
            } else {
                self.active_statement_base = Some(*self.block_indents.last().unwrap_or(&0));
            }
        } else if self.active_statement_base.is_none() {
            // An opener can only have been emitted as part of an active
            // statement; this is defensive recovery after an earlier error.
            self.active_statement_base = Some(*self.block_indents.last().unwrap_or(&0));
        }

        let token_start = self.tokens.len();
        if is_doc_comment {
            let raw = &rest[3..];
            let contents = raw.strip_prefix(' ').unwrap_or(raw).to_string();
            self.emit(TokenKind::DocComment(contents), content_start, line_end);
        } else {
            self.lex_line_tokens(content_start, line_end);
        }

        if self.delimiters.is_empty() && self.tokens.len() > token_start {
            let newline_start = if newline_end > line_end {
                line_end
            } else {
                newline_end
            };
            let newline_span = self.span(newline_start, newline_end);

            if is_doc_comment {
                self.emit(
                    TokenKind::Newline,
                    newline_span.start as usize,
                    newline_span.end as usize,
                );
                self.active_statement_base = None;
                return;
            }

            if matches!(
                self.tokens.last().map(|token| &token.kind),
                Some(TokenKind::Colon)
            ) {
                self.emit(
                    TokenKind::Newline,
                    newline_span.start as usize,
                    newline_span.end as usize,
                );
                self.expected_block_base = self.active_statement_base.take();
                self.pending_newline = None;
            } else {
                self.pending_newline = Some(PendingNewline {
                    statement_base: self
                        .active_statement_base
                        .unwrap_or_else(|| *self.block_indents.last().unwrap_or(&0)),
                    span: newline_span,
                });
            }
        }
    }

    /// Returns true when the line is a continuation of the active statement.
    fn prepare_logical_line(
        &mut self,
        indent: usize,
        is_doc_comment: bool,
        line_start: usize,
        content_start: usize,
    ) -> bool {
        if let Some(pending) = self.pending_newline {
            if !is_doc_comment && indent == pending.statement_base + 4 {
                self.pending_newline = None;
                return true;
            }
            self.emit(
                TokenKind::Newline,
                pending.span.start as usize,
                pending.span.end as usize,
            );
            self.pending_newline = None;
            self.active_statement_base = None;
        }

        if let Some(base) = self.expected_block_base.take() {
            let expected = base + 4;
            if indent == expected {
                self.emit(TokenKind::Indent, line_start, content_start);
                self.block_indents.push(expected);
                return false;
            }

            self.error(
                Category::LexicalIndentation,
                line_start,
                content_start,
                format!("expected exactly {expected} spaces of indentation after `:`"),
            );
            if indent > base {
                self.emit(TokenKind::Indent, line_start, content_start);
                self.block_indents.push(expected);
                if indent != expected {
                    // Continue using the specified level so later valid lines
                    // are not forced to repeat the erroneous indentation.
                    return false;
                }
            }
        }

        self.apply_indentation(indent, line_start, content_start);
        false
    }

    fn apply_indentation(&mut self, indent: usize, line_start: usize, content_start: usize) {
        let current = *self.block_indents.last().unwrap_or(&0);
        if indent == current {
            return;
        }
        if indent > current {
            self.error(
                Category::LexicalIndentation,
                line_start,
                content_start,
                format!("unexpected indentation: expected {current} spaces, found {indent}"),
            );
            return;
        }

        if !self.block_indents.contains(&indent) {
            self.error(
                Category::LexicalIndentation,
                line_start,
                content_start,
                format!("dedent to {indent} spaces does not match an open block"),
            );
        }
        while self
            .block_indents
            .last()
            .is_some_and(|level| *level > indent)
        {
            self.block_indents.pop();
            self.emit(TokenKind::Dedent, line_start, content_start);
        }
    }

    fn lex_line_tokens(&mut self, mut pos: usize, line_end: usize) {
        let bytes = self.source.as_bytes();
        while pos < line_end {
            match bytes[pos] {
                b' ' | b'\t' => {
                    pos += 1;
                }
                b'/' if pos + 1 < line_end && bytes[pos + 1] == b'/' => break,
                b'f' if pos + 1 < line_end && bytes[pos + 1] == b'"' => {
                    pos = self.lex_formatted_string(pos, line_end);
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    pos = self.lex_identifier(pos, line_end);
                }
                b'0'..=b'9' => {
                    // After a postfix dot, keep a decimal selector separate
                    // from a following dot (`nested.0.1`). Ordinary literals
                    // such as `1.0` retain their existing tokenization.
                    let allow_float = !self
                        .tokens
                        .last()
                        .is_some_and(|token| matches!(token.kind, TokenKind::Dot));
                    pos = self.lex_number(pos, line_end, allow_float);
                }
                b'"' => {
                    pos = self.lex_quoted(pos, line_end, b'"');
                }
                b'\'' => {
                    pos = self.lex_quoted(pos, line_end, b'\'');
                }
                _ => {
                    if let Some((kind, length)) = punctuation(&self.source[pos..line_end]) {
                        let start = pos;
                        pos += length;
                        self.handle_delimiter(&kind, start, pos);
                        self.emit(kind, start, pos);
                    } else {
                        let ch = self.source[pos..line_end]
                            .chars()
                            .next()
                            .expect("pos is before line_end");
                        let next = pos + ch.len_utf8();
                        self.error(
                            Category::LexicalCharacter,
                            pos,
                            next,
                            format!("character `{ch}` cannot begin an Elamite token"),
                        );
                        pos = next;
                    }
                }
            }
        }
    }

    fn lex_identifier(&mut self, start: usize, line_end: usize) -> usize {
        let bytes = self.source.as_bytes();
        let mut end = start + 1;
        while end < line_end && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        let text = &self.source[start..end];
        debug_assert!(is_valid_identifier(text));
        let kind = keyword(text)
            .map(TokenKind::Keyword)
            .unwrap_or_else(|| TokenKind::Identifier(text.to_string()));
        self.emit(kind, start, end);
        end
    }

    fn lex_number(&mut self, start: usize, line_end: usize, allow_float: bool) -> usize {
        let bytes = self.source.as_bytes();
        if bytes[start] == b'0' && start + 1 < line_end {
            let radix = match bytes[start + 1] {
                b'b' => Some(2),
                b'o' => Some(8),
                b'x' => Some(16),
                _ => None,
            };
            if let Some(radix) = radix {
                return self.lex_prefixed_integer(start, line_end, radix);
            }
        }

        let mut end = scan_while(self.source, start, line_end, |byte| {
            byte.is_ascii_digit() || byte == b'_'
        });
        self.validate_digit_run(start, end, 10);
        let mut is_float = false;

        if allow_float
            && end + 1 < line_end
            && bytes[end] == b'.'
            && bytes[end + 1].is_ascii_digit()
        {
            is_float = true;
            end += 1;
            let fraction_start = end;
            end = scan_while(self.source, end, line_end, |byte| {
                byte.is_ascii_digit() || byte == b'_'
            });
            self.validate_digit_run(fraction_start, end, 10);
        }

        if allow_float && end < line_end && matches!(bytes[end], b'e' | b'E') {
            is_float = true;
            end += 1;
            if end < line_end && matches!(bytes[end], b'+' | b'-') {
                end += 1;
            }
            let exponent_start = end;
            end = scan_while(self.source, end, line_end, |byte| {
                byte.is_ascii_digit() || byte == b'_'
            });
            if exponent_start == end {
                self.error(
                    Category::LexicalLiteral,
                    start,
                    end,
                    "floating-point exponent requires at least one digit",
                );
            } else {
                self.validate_digit_run(exponent_start, end, 10);
            }
        }

        let suffix_start = end;
        end = scan_while(self.source, end, line_end, |byte| {
            byte.is_ascii_alphanumeric() || byte == b'_'
        });
        let suffix_text = &self.source[suffix_start..end];
        let suffix = if suffix_text.is_empty() {
            None
        } else {
            parse_numeric_suffix(suffix_text)
        };

        if !suffix_text.is_empty() {
            let valid = match suffix {
                Some(NumericSuffix::F32 | NumericSuffix::F64) => is_float,
                Some(_) => !is_float,
                None => false,
            };
            if !valid {
                self.error(
                    Category::LexicalLiteral,
                    suffix_start,
                    end,
                    format!("invalid suffix `{suffix_text}` for this numeric literal"),
                );
            }
        }

        let raw = self.source[start..end].to_string();
        if is_float {
            self.emit(TokenKind::FloatLiteral { raw, suffix }, start, end);
        } else {
            self.emit(
                TokenKind::IntegerLiteral {
                    raw,
                    radix: 10,
                    suffix,
                },
                start,
                end,
            );
        }
        end
    }

    fn lex_prefixed_integer(&mut self, start: usize, line_end: usize, radix: u8) -> usize {
        let mut end = start + 2;
        let digits_start = end;
        end = scan_while(self.source, end, line_end, |byte| {
            byte == b'_' || digit_value(byte).is_some_and(|value| value < radix)
        });
        if digits_start == end {
            self.error(
                Category::LexicalLiteral,
                start,
                end,
                format!("base-{radix} literal requires at least one digit"),
            );
        } else {
            self.validate_digit_run(digits_start, end, radix);
        }

        let suffix_start = end;
        end = scan_while(self.source, end, line_end, |byte| {
            byte.is_ascii_alphanumeric() || byte == b'_'
        });
        let suffix_text = &self.source[suffix_start..end];
        let suffix = if suffix_text.is_empty() {
            None
        } else {
            parse_numeric_suffix(suffix_text)
        };
        if !suffix_text.is_empty()
            && !matches!(
                suffix,
                Some(
                    NumericSuffix::I8
                        | NumericSuffix::I16
                        | NumericSuffix::I32
                        | NumericSuffix::I64
                        | NumericSuffix::I128
                        | NumericSuffix::Isize
                        | NumericSuffix::U8
                        | NumericSuffix::U16
                        | NumericSuffix::U32
                        | NumericSuffix::U64
                        | NumericSuffix::U128
                        | NumericSuffix::Usize
                )
            )
        {
            self.error(
                Category::LexicalLiteral,
                suffix_start,
                end,
                format!("invalid integer suffix `{suffix_text}`"),
            );
        }

        self.emit(
            TokenKind::IntegerLiteral {
                raw: self.source[start..end].to_string(),
                radix,
                suffix,
            },
            start,
            end,
        );
        end
    }

    fn validate_digit_run(&mut self, start: usize, end: usize, radix: u8) {
        let text = &self.source[start..end];
        let invalid_separator = text.starts_with('_')
            || text.ends_with('_')
            || text.as_bytes().windows(2).any(|pair| pair == b"__");
        let invalid_digit = text
            .bytes()
            .any(|byte| byte != b'_' && digit_value(byte).is_none_or(|value| value >= radix));
        if invalid_separator || invalid_digit {
            self.error(
                Category::LexicalLiteral,
                start,
                end,
                format!("invalid digit sequence for a base-{radix} literal"),
            );
        }
    }

    fn lex_quoted(&mut self, start: usize, line_end: usize, quote: u8) -> usize {
        let mut pos = start + 1;
        let mut decoded = String::new();
        let mut terminated = false;
        while pos < line_end {
            let byte = self.source.as_bytes()[pos];
            if byte == quote {
                pos += 1;
                terminated = true;
                break;
            }
            if byte == b'\\' {
                let (value, next) = self.decode_escape(pos, line_end);
                if let Some(value) = value {
                    decoded.push(value);
                }
                pos = next;
                continue;
            }
            let ch = self.source[pos..line_end]
                .chars()
                .next()
                .expect("pos is before line_end");
            decoded.push(ch);
            pos += ch.len_utf8();
        }

        if !terminated {
            self.error(
                Category::LexicalLiteral,
                start,
                line_end,
                if quote == b'"' {
                    "unterminated string literal"
                } else {
                    "unterminated character literal"
                },
            );
        }

        if quote == b'\'' && decoded.chars().count() != 1 {
            self.error(
                Category::LexicalLiteral,
                start,
                pos,
                "a character literal must contain exactly one Unicode scalar value",
            );
        }

        let kind = if quote == b'"' {
            TokenKind::StringLiteral(decoded)
        } else {
            TokenKind::CharacterLiteral(decoded)
        };
        self.emit(kind, start, pos);
        pos
    }

    fn decode_escape(&mut self, start: usize, limit: usize) -> (Option<char>, usize) {
        let mut pos = start + 1;
        if pos >= limit {
            self.error(
                Category::LexicalLiteral,
                start,
                limit,
                "incomplete escape sequence",
            );
            return (None, limit);
        }
        let escaped = self.source[pos..limit]
            .chars()
            .next()
            .expect("pos is before the escape limit");
        pos += escaped.len_utf8();
        let simple = match escaped {
            '\\' => Some('\\'),
            '"' => Some('"'),
            '\'' => Some('\''),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            '0' => Some('\0'),
            _ => None,
        };
        if simple.is_some() {
            return (simple, pos);
        }

        if escaped == 'u' && pos < limit && self.source.as_bytes()[pos] == b'{' {
            pos += 1;
            let digits_start = pos;
            while pos < limit && self.source.as_bytes()[pos].is_ascii_hexdigit() {
                pos += 1;
            }
            let digits_end = pos;
            let has_close = pos < limit && self.source.as_bytes()[pos] == b'}';
            if has_close {
                pos += 1;
            }
            let digits = &self.source[digits_start..digits_end];
            if !has_close || digits.is_empty() || digits.len() > 6 {
                self.error(
                    Category::LexicalLiteral,
                    start,
                    pos,
                    "Unicode escape must be `\\u{HEX}` with one through six hexadecimal digits",
                );
                return (None, pos);
            }
            let value = u32::from_str_radix(digits, 16).expect("validated hexadecimal digits");
            if let Some(ch) = char::from_u32(value) {
                return (Some(ch), pos);
            }
            self.error(
                Category::LexicalLiteral,
                start,
                pos,
                "Unicode escape does not denote a valid Unicode scalar value",
            );
            return (None, pos);
        }

        self.error(
            Category::LexicalLiteral,
            start,
            pos,
            format!("unsupported escape sequence `\\{escaped}`"),
        );
        (None, pos)
    }

    fn lex_formatted_string(&mut self, start: usize, line_end: usize) -> usize {
        let mut pos = start + 2;
        let mut text_start = pos;
        let mut text = String::new();
        let mut segments = Vec::new();
        let mut terminated = false;

        while pos < line_end {
            match self.source.as_bytes()[pos] {
                b'"' => {
                    self.flush_formatted_text(&mut segments, &mut text, text_start, pos);
                    pos += 1;
                    terminated = true;
                    break;
                }
                b'\\' => {
                    let (value, next) = self.decode_escape(pos, line_end);
                    if let Some(value) = value {
                        text.push(value);
                    }
                    pos = next;
                }
                b'{' if pos + 1 < line_end && self.source.as_bytes()[pos + 1] == b'{' => {
                    text.push('{');
                    pos += 2;
                }
                b'}' if pos + 1 < line_end && self.source.as_bytes()[pos + 1] == b'}' => {
                    text.push('}');
                    pos += 2;
                }
                b'{' => {
                    self.flush_formatted_text(&mut segments, &mut text, text_start, pos);
                    let expression_start = pos + 1;
                    match self.find_interpolation_end(expression_start, line_end) {
                        Some(expression_end) => {
                            let expression = &self.source[expression_start..expression_end];
                            if expression.trim().is_empty() {
                                self.error(
                                    Category::LexicalLiteral,
                                    pos,
                                    expression_end + 1,
                                    "formatted-string interpolation cannot be empty",
                                );
                            }
                            let (tokens, diagnostics) = lex_fragment(
                                self.file,
                                self.source,
                                expression_start,
                                expression_end,
                            );
                            self.diagnostics.extend(diagnostics);
                            segments.push(FormattedSegment {
                                kind: FormattedSegmentKind::Expression {
                                    source: expression.to_string(),
                                    tokens,
                                },
                                span: self.span(expression_start, expression_end),
                            });
                            pos = expression_end + 1;
                            text_start = pos;
                        }
                        None => {
                            self.error(
                                Category::LexicalLiteral,
                                pos,
                                line_end,
                                "unclosed `{` in formatted string",
                            );
                            pos = line_end;
                        }
                    }
                }
                b'}' => {
                    self.error(
                        Category::LexicalLiteral,
                        pos,
                        pos + 1,
                        "unmatched `}` in formatted string; use `}}` for literal text",
                    );
                    pos += 1;
                }
                _ => {
                    let ch = self.source[pos..line_end]
                        .chars()
                        .next()
                        .expect("pos is before line_end");
                    text.push(ch);
                    pos += ch.len_utf8();
                }
            }
        }

        if !terminated {
            self.flush_formatted_text(&mut segments, &mut text, text_start, pos);
            self.error(
                Category::LexicalLiteral,
                start,
                line_end,
                "unterminated formatted string literal",
            );
        }
        self.emit(TokenKind::FormattedString(segments), start, pos);
        pos
    }

    fn flush_formatted_text(
        &self,
        segments: &mut Vec<FormattedSegment>,
        text: &mut String,
        start: usize,
        end: usize,
    ) {
        if text.is_empty() {
            return;
        }
        segments.push(FormattedSegment {
            kind: FormattedSegmentKind::Text(std::mem::take(text)),
            span: self.span(start, end),
        });
    }

    fn find_interpolation_end(&self, mut pos: usize, limit: usize) -> Option<usize> {
        let mut brace_depth = 0usize;
        while pos < limit {
            match self.source.as_bytes()[pos] {
                b'"' | b'\'' => {
                    let quote = self.source.as_bytes()[pos];
                    pos += 1;
                    while pos < limit {
                        match self.source.as_bytes()[pos] {
                            b'\\' => {
                                pos += 1;
                                if pos < limit {
                                    let escaped = self.source[pos..limit].chars().next()?;
                                    pos += escaped.len_utf8();
                                }
                            }
                            byte if byte == quote => {
                                pos += 1;
                                break;
                            }
                            _ => {
                                let ch = self.source[pos..limit].chars().next()?;
                                pos += ch.len_utf8();
                            }
                        }
                    }
                }
                b'{' => {
                    brace_depth += 1;
                    pos += 1;
                }
                b'}' if brace_depth == 0 => return Some(pos),
                b'}' => {
                    brace_depth -= 1;
                    pos += 1;
                }
                _ => {
                    let ch = self.source[pos..limit].chars().next()?;
                    pos += ch.len_utf8();
                }
            }
        }
        None
    }

    fn handle_delimiter(&mut self, kind: &TokenKind, start: usize, end: usize) {
        let opening = match kind {
            TokenKind::LParen => Some(Delimiter::Parenthesis),
            TokenKind::LBracket => Some(Delimiter::Bracket),
            TokenKind::LBrace => Some(Delimiter::Brace),
            _ => None,
        };
        if let Some(kind) = opening {
            self.delimiters.push(OpenDelimiter {
                kind,
                span: self.span(start, end),
            });
            return;
        }

        let closing = match kind {
            TokenKind::RParen => Some(Delimiter::Parenthesis),
            TokenKind::RBracket => Some(Delimiter::Bracket),
            TokenKind::RBrace => Some(Delimiter::Brace),
            _ => None,
        };
        let Some(closing) = closing else {
            return;
        };
        match self.delimiters.last().copied() {
            Some(open) if open.kind == closing => {
                self.delimiters.pop();
            }
            Some(open) => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::LexicalDelimiter,
                        format!(
                            "mismatched delimiter: expected {}, found a different closer",
                            open.kind.closing_name()
                        ),
                    )
                    .with_primary(self.span(start, end))
                    .with_related(open.span, "opening delimiter is here"),
                );
                self.delimiters.pop();
            }
            None => self.error(
                Category::LexicalDelimiter,
                start,
                end,
                "closing delimiter has no matching opener",
            ),
        }
    }

    fn finish(&mut self) {
        if let Some(pending) = self.pending_newline.take() {
            self.emit(
                TokenKind::Newline,
                pending.span.start as usize,
                pending.span.end as usize,
            );
            self.active_statement_base = None;
        } else if self.active_statement_base.take().is_some() {
            let end = self.source.len();
            self.emit(TokenKind::Newline, end, end);
        }

        if self.expected_block_base.take().is_some() {
            let end = self.source.len();
            self.error(
                Category::LexicalIndentation,
                end,
                end,
                "expected an indented body before end of file",
            );
        }

        let unclosed = std::mem::take(&mut self.delimiters);
        for open in unclosed.into_iter().rev() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::LexicalDelimiter,
                    format!("unclosed delimiter; expected {}", open.kind.closing_name()),
                )
                .with_primary(open.span),
            );
        }

        let end = self.source.len();
        while self.block_indents.len() > 1 {
            self.block_indents.pop();
            self.emit(TokenKind::Dedent, end, end);
        }
        self.emit(TokenKind::Eof, end, end);
    }
}

fn lex_fragment(
    file: FileId,
    source: &str,
    start: usize,
    end: usize,
) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lexer = Lexer::new(file, source);
    lexer.lex_line_tokens(start, end);
    let unclosed = std::mem::take(&mut lexer.delimiters);
    for open in unclosed.into_iter().rev() {
        lexer.diagnostics.push(
            Diagnostic::new(
                Category::LexicalDelimiter,
                format!("unclosed delimiter; expected {}", open.kind.closing_name()),
            )
            .with_primary(open.span),
        );
    }
    (lexer.tokens, lexer.diagnostics)
}

fn scan_while(source: &str, mut pos: usize, limit: usize, predicate: impl Fn(u8) -> bool) -> usize {
    while pos < limit && predicate(source.as_bytes()[pos]) {
        pos += 1;
    }
    pos
}

fn digit_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_numeric_suffix(text: &str) -> Option<NumericSuffix> {
    Some(match text {
        "i8" => NumericSuffix::I8,
        "i16" => NumericSuffix::I16,
        "i32" => NumericSuffix::I32,
        "i64" => NumericSuffix::I64,
        "i128" => NumericSuffix::I128,
        "isize" => NumericSuffix::Isize,
        "u8" => NumericSuffix::U8,
        "u16" => NumericSuffix::U16,
        "u32" => NumericSuffix::U32,
        "u64" => NumericSuffix::U64,
        "u128" => NumericSuffix::U128,
        "usize" => NumericSuffix::Usize,
        "f32" => NumericSuffix::F32,
        "f64" => NumericSuffix::F64,
        _ => return None,
    })
}

fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "as" => Keyword::As,
        "attr" => Keyword::Attr,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "defer" => Keyword::Defer,
        "derive" => Keyword::Derive,
        "else" => Keyword::Else,
        "enum" => Keyword::Enum,
        "expect" => Keyword::Expect,
        "false" => Keyword::False,
        "fn" => Keyword::Fn,
        "for" => Keyword::For,
        "if" => Keyword::If,
        "impl" => Keyword::Impl,
        "in" => Keyword::In,
        "let" => Keyword::Let,
        "macro" => Keyword::Macro,
        "match" => Keyword::Match,
        "mod" => Keyword::Mod,
        "null" => Keyword::Null,
        "pass" => Keyword::Pass,
        "pub" => Keyword::Pub,
        "quote" => Keyword::Quote,
        "return" => Keyword::Return,
        "root" => Keyword::Root,
        "self" => Keyword::SelfValue,
        "Self" => Keyword::SelfType,
        "struct" => Keyword::Struct,
        "super" => Keyword::Super,
        "test" => Keyword::Test,
        "trait" => Keyword::Trait,
        "true" => Keyword::True,
        "type" => Keyword::Type,
        "unsafe" => Keyword::Unsafe,
        "use" => Keyword::Use,
        "var" => Keyword::Var,
        "while" => Keyword::While,
        _ => return None,
    })
}

fn punctuation(source: &str) -> Option<(TokenKind, usize)> {
    let candidates: &[(&str, TokenKind)] = &[
        ("<<=", TokenKind::ShlAssign),
        (">>=", TokenKind::ShrAssign),
        ("...", TokenKind::Ellipsis),
        ("->", TokenKind::Arrow),
        ("..", TokenKind::DotDot),
        ("<<", TokenKind::Shl),
        (">>", TokenKind::Shr),
        ("==", TokenKind::EqEq),
        ("!=", TokenKind::NotEq),
        ("<=", TokenKind::LessEq),
        (">=", TokenKind::GreaterEq),
        ("&&", TokenKind::AndAnd),
        ("||", TokenKind::OrOr),
        ("++", TokenKind::PlusPlus),
        ("+=", TokenKind::PlusAssign),
        ("-=", TokenKind::MinusAssign),
        ("*=", TokenKind::StarAssign),
        ("/=", TokenKind::SlashAssign),
        ("%=", TokenKind::PercentAssign),
        ("&=", TokenKind::AmpAssign),
        ("|=", TokenKind::PipeAssign),
        ("^=", TokenKind::CaretAssign),
        ("(", TokenKind::LParen),
        (")", TokenKind::RParen),
        ("[", TokenKind::LBracket),
        ("]", TokenKind::RBracket),
        ("{", TokenKind::LBrace),
        ("}", TokenKind::RBrace),
        (",", TokenKind::Comma),
        (";", TokenKind::Semicolon),
        (":", TokenKind::Colon),
        (".", TokenKind::Dot),
        ("@", TokenKind::At),
        ("$", TokenKind::Dollar),
        ("?", TokenKind::Question),
        ("=", TokenKind::Assign),
        ("+", TokenKind::Plus),
        ("-", TokenKind::Minus),
        ("*", TokenKind::Star),
        ("/", TokenKind::Slash),
        ("%", TokenKind::Percent),
        ("&", TokenKind::Amp),
        ("|", TokenKind::Pipe),
        ("^", TokenKind::Caret),
        ("~", TokenKind::Tilde),
        ("!", TokenKind::Bang),
        ("<", TokenKind::Less),
        (">", TokenKind::Greater),
    ];
    candidates
        .iter()
        .find(|(spelling, _)| source.starts_with(spelling))
        .map(|(spelling, kind)| (kind.clone(), spelling.len()))
}
