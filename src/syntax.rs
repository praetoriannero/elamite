//! Phase-neutral tokens, syntax trees, and structural traversal.
//!
//! Lexing produces [`Token`] values and parsing produces [`SyntaxNode`] values.
//! Neither representation belongs to a later semantic phase. Keeping their
//! shared data here lets package parsing, expansion, resolution, documentation,
//! and semantic checking consume syntax without depending on parser internals.

use crate::source::Span;

/// A source token with its exact byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Reserved source words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    As,
    Break,
    Continue,
    Defer,
    Else,
    Enum,
    False,
    Fn,
    For,
    If,
    Impl,
    In,
    Let,
    Match,
    Mod,
    Null,
    Pass,
    Pub,
    Return,
    Root,
    SelfValue,
    SelfType,
    Struct,
    Super,
    Trait,
    True,
    Type,
    Unsafe,
    Use,
    Var,
    While,
}

/// A concrete numeric suffix. Literal range and contextual materialization are
/// later type-checking work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericSuffix {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
}

/// One portion of a formatted string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedSegment {
    pub kind: FormattedSegmentKind,
    pub span: Span,
}

/// Formatted-string text is decoded by the lexer. Interpolation source is
/// retained verbatim with its span and token stream so the parser can attach
/// its expression subtree without losing the original source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormattedSegmentKind {
    Text(String),
    Expression { source: String, tokens: Vec<Token> },
}

/// All tokens needed by the current surface language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Identifier(String),
    Keyword(Keyword),
    IntegerLiteral {
        raw: String,
        radix: u8,
        suffix: Option<NumericSuffix>,
    },
    FloatLiteral {
        raw: String,
        suffix: Option<NumericSuffix>,
    },
    StringLiteral(String),
    CharacterLiteral(String),
    FormattedString(Vec<FormattedSegment>),
    DocComment(String),

    Newline,
    Indent,
    Dedent,
    Eof,

    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    Dot,
    DotDot,
    Ellipsis,
    At,
    Question,
    Arrow,

    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Bang,
    Shl,
    Shr,
    EqEq,
    NotEq,
    Less,
    LessEq,
    Greater,
    GreaterEq,
    AndAnd,
    OrOr,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
    ShlAssign,
    ShrAssign,
}

/// Structural syntax-node categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxKind {
    File,
    Error,
    Documentation,
    Attribute,

    Module,
    Use,
    TypeAlias,
    Struct,
    Enum,
    Trait,
    Impl,
    Function,
    ForeignType,
    Field,
    EnumVariant,
    GenericParameters,
    GenericParameter,
    DeriveList,
    Parameters,
    Parameter,
    Type,
    TypeArguments,
    Block,

    LetStatement,
    AssignmentStatement,
    ExpressionStatement,
    ReturnStatement,
    BreakStatement,
    ContinueStatement,
    PassStatement,
    DeferStatement,
    IfStatement,
    ElseClause,
    MatchStatement,
    MatchArm,
    ForStatement,
    WhileStatement,
    UnsafeBlock,

    Pattern,
    AlternativePattern,
    DereferencePattern,
    TuplePattern,
    RecordPattern,
    VariantPattern,
    PatternField,
    Guard,

    NameExpression,
    LiteralExpression,
    FormattedStringExpression,
    UnaryExpression,
    BinaryExpression,
    CastExpression,
    CallExpression,
    MemberExpression,
    BracketExpression,
    TryExpression,
    ParenthesizedExpression,
    TupleExpression,
    ArrayExpression,
    MacroExpression,
    RecordExpression,
    RecordField,
}

/// A syntax node or one of its original tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxElement {
    Node(Box<SyntaxNode>),
    Token(Token),
}

impl SyntaxElement {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Node(node) => node.span,
            Self::Token(token) => token.span,
        }
    }
}

/// A token-preserving syntax node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode {
    pub kind: SyntaxKind,
    pub span: Span,
    pub children: Vec<SyntaxElement>,
}

impl SyntaxNode {
    pub(crate) fn new(kind: SyntaxKind, children: Vec<SyntaxElement>, fallback: Span) -> Self {
        let span = match (children.first(), children.last()) {
            (Some(first), Some(last)) => {
                let first = first.span();
                let last = last.span();
                Span::new(first.file, first.start, last.end)
            }
            _ => fallback,
        };
        Self {
            kind,
            span,
            children,
        }
    }

    /// Direct token children in source order.
    #[must_use]
    pub fn direct_tokens(&self) -> Vec<&Token> {
        self.children
            .iter()
            .filter_map(|child| match child {
                SyntaxElement::Token(token) => Some(token),
                SyntaxElement::Node(_) => None,
            })
            .collect()
    }

    /// Direct node children in source order.
    #[must_use]
    pub fn direct_nodes(&self) -> Vec<&SyntaxNode> {
        self.children
            .iter()
            .filter_map(|child| match child {
                SyntaxElement::Node(node) => Some(node.as_ref()),
                SyntaxElement::Token(_) => None,
            })
            .collect()
    }

    /// Direct node children of `kind` in source order.
    #[must_use]
    pub fn direct_children(&self, kind: SyntaxKind) -> Vec<&SyntaxNode> {
        self.direct_nodes()
            .into_iter()
            .filter(|child| child.kind == kind)
            .collect()
    }

    /// The first direct node child of `kind`.
    #[must_use]
    pub fn direct_child(&self, kind: SyntaxKind) -> Option<&SyntaxNode> {
        self.direct_nodes()
            .into_iter()
            .find(|child| child.kind == kind)
    }

    /// The first direct node child whose kind differs from `kind`.
    #[must_use]
    pub fn direct_child_not(&self, kind: SyntaxKind) -> Option<&SyntaxNode> {
        self.direct_nodes()
            .into_iter()
            .find(|child| child.kind != kind)
    }

    /// Recursively counts nodes of `kind`.
    #[must_use]
    pub fn count(&self, kind: SyntaxKind) -> usize {
        usize::from(self.kind == kind)
            + self
                .children
                .iter()
                .map(|child| match child {
                    SyntaxElement::Node(node) => node.count(kind),
                    SyntaxElement::Token(_) => 0,
                })
                .sum::<usize>()
    }

    /// Produces a deterministic, indentation-based representation for
    /// snapshots, diagnostics, and command-line debugging.
    #[must_use]
    pub fn dump(&self) -> String {
        fn walk(node: &SyntaxNode, depth: usize, output: &mut String) {
            output.push_str(&"  ".repeat(depth));
            output.push_str(&format!(
                "{:?} @ {}..{}\n",
                node.kind, node.span.start, node.span.end
            ));
            for child in &node.children {
                match child {
                    SyntaxElement::Node(child) => walk(child, depth + 1, output),
                    SyntaxElement::Token(token) => {
                        output.push_str(&"  ".repeat(depth + 1));
                        output.push_str(&format!(
                            "{:?} @ {}..{}\n",
                            token.kind, token.span.start, token.span.end
                        ));
                    }
                }
            }
        }

        let mut output = String::new();
        walk(self, 0, &mut output);
        output
    }
}

#[must_use]
pub fn direct_tokens(node: &SyntaxNode) -> Vec<&Token> {
    node.direct_tokens()
}

#[must_use]
pub fn child_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    node.direct_nodes()
}

#[must_use]
pub fn direct_children(node: &SyntaxNode, kind: SyntaxKind) -> Vec<&SyntaxNode> {
    node.direct_children(kind)
}

#[must_use]
pub fn direct_child(node: &SyntaxNode, kind: SyntaxKind) -> Option<&SyntaxNode> {
    node.direct_child(kind)
}

#[must_use]
pub fn direct_child_not(node: &SyntaxNode, kind: SyntaxKind) -> Option<&SyntaxNode> {
    node.direct_child_not(kind)
}

/// Span of the direct identifier/module-keyword path in a syntax node.
#[must_use]
pub fn direct_path_span(node: &SyntaxNode) -> Option<Span> {
    let meaningful = direct_tokens(node)
        .into_iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::Identifier(_)
                    | TokenKind::Keyword(
                        Keyword::Root | Keyword::SelfValue | Keyword::SelfType | Keyword::Super
                    )
            )
        })
        .collect::<Vec<_>>();
    let first = meaningful.first()?;
    let last = meaningful.last()?;
    Some(Span::new(first.span.file, first.span.start, last.span.end))
}
