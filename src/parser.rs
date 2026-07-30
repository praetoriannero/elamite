//! Hand-written, token-preserving surface parser.
//!
//! The parser implements `ROADMAP.md` Milestone 3. It constructs a structural
//! syntax tree and deliberately performs no name resolution, type inference,
//! visibility checking, receiver validation, or safety checking.

use std::mem::discriminant;

use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::{FormattedSegmentKind, Keyword, Token, TokenKind};
use crate::source::Span;

/// A complete parse result. A recovered tree is returned alongside diagnostics.
#[derive(Debug)]
pub struct ParseOutput {
    pub tree: SyntaxNode,
    pub diagnostics: Vec<Diagnostic>,
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
            SyntaxElement::Node(node) => node.span,
            SyntaxElement::Token(token) => token.span,
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
    fn new(kind: SyntaxKind, children: Vec<SyntaxElement>, fallback: Span) -> Self {
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

/// Parses a complete token stream produced by [`crate::lexer::lex`].
#[must_use]
pub fn parse(tokens: &[Token]) -> ParseOutput {
    Parser::new(tokens).parse_file()
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemHead {
    Module,
    Use,
    TypeAlias,
    Struct,
    Enum,
    Trait,
    Impl,
    Function,
    Unknown,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            position: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse_file(mut self) -> ParseOutput {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.eat_newlines(&mut children);
        while !self.at_eof() {
            if self.at_simple(&TokenKind::Dedent) {
                self.error_here("unexpected dedent at file scope");
                children.push(self.bump());
                continue;
            }
            let before = self.position;
            children.push(node(self.parse_item()));
            if self.position == before {
                self.error_here("parser could not make progress");
                children.push(self.bump());
            }
            self.eat_newlines(&mut children);
        }
        if self.at_eof() {
            children.push(self.bump());
        }
        ParseOutput {
            tree: SyntaxNode::new(SyntaxKind::File, children, fallback),
            diagnostics: self.diagnostics,
        }
    }

    fn parse_item(&mut self) -> SyntaxNode {
        match self.classify_item() {
            ItemHead::Module => self.parse_module(),
            ItemHead::Use => self.parse_use(),
            ItemHead::TypeAlias => self.parse_type_alias(),
            ItemHead::Struct => self.parse_struct(false),
            ItemHead::Enum => self.parse_enum(),
            ItemHead::Trait => self.parse_trait(),
            ItemHead::Impl => self.parse_impl(),
            ItemHead::Function => {
                let body = if self.leading_attribute_named("importc") {
                    FunctionBody::Forbidden
                } else {
                    FunctionBody::Required
                };
                self.parse_function(body)
            }
            ItemHead::Unknown => {
                let fallback = self.current_span();
                let mut children = self.take_docs();
                self.error_here("expected a declaration");
                self.recover_line(&mut children);
                SyntaxNode::new(SyntaxKind::Error, children, fallback)
            }
        }
    }

    fn classify_item(&self) -> ItemHead {
        let mut offset = 0;
        while matches!(
            self.kind_at(offset),
            Some(TokenKind::DocComment(_) | TokenKind::Newline)
        ) {
            offset += 1;
        }
        while matches!(self.kind_at(offset), Some(TokenKind::At)) {
            while !matches!(
                self.kind_at(offset),
                Some(TokenKind::Newline | TokenKind::Eof) | None
            ) {
                offset += 1;
            }
            if matches!(self.kind_at(offset), Some(TokenKind::Newline)) {
                offset += 1;
            }
        }
        while let Some(TokenKind::Keyword(Keyword::Pub | Keyword::Unsafe)) = self.kind_at(offset) {
            offset += 1;
        }
        match self.kind_at(offset) {
            Some(TokenKind::Keyword(Keyword::Mod)) => ItemHead::Module,
            Some(TokenKind::Keyword(Keyword::Use)) => ItemHead::Use,
            Some(TokenKind::Keyword(Keyword::Type)) => ItemHead::TypeAlias,
            Some(TokenKind::Keyword(Keyword::Struct)) => ItemHead::Struct,
            Some(TokenKind::Keyword(Keyword::Enum)) => ItemHead::Enum,
            Some(TokenKind::Keyword(Keyword::Trait)) => ItemHead::Trait,
            Some(TokenKind::Keyword(Keyword::Impl)) => ItemHead::Impl,
            Some(TokenKind::Keyword(Keyword::Fn)) => ItemHead::Function,
            _ => ItemHead::Unknown,
        }
    }

    fn parse_module(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_keyword(Keyword::Mod, &mut children, "expected `mod`");
        self.expect_identifier(&mut children, "expected module name");
        self.parse_item_block(&mut children, MemberContext::Module);
        SyntaxNode::new(SyntaxKind::Module, children, fallback)
    }

    fn parse_use(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_keyword(Keyword::Use, &mut children, "expected `use`");
        self.parse_path_tokens(&mut children);
        if self.at_keyword(Keyword::As) {
            children.push(self.bump());
            self.expect_identifier(&mut children, "expected alias name");
        }
        self.expect_line_end(&mut children);
        SyntaxNode::new(SyntaxKind::Use, children, fallback)
    }

    fn parse_type_alias(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let imported = self.leading_attribute_named("importc");
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_keyword(Keyword::Type, &mut children, "expected `type`");
        self.expect_identifier(&mut children, "expected type alias name");
        if self.at_simple(&TokenKind::LBracket) {
            children.push(node(self.parse_generic_parameters()));
        }
        if self.at_simple(&TokenKind::Assign) {
            children.push(self.bump());
            children.push(node(self.parse_type()));
        } else if !imported {
            self.error_here("a bodyless type declaration requires `@importc`");
        }
        self.expect_line_end(&mut children);
        SyntaxNode::new(
            if imported {
                SyntaxKind::ForeignType
            } else {
                SyntaxKind::TypeAlias
            },
            children,
            fallback,
        )
    }

    fn parse_struct(&mut self, foreign: bool) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_keyword(Keyword::Struct, &mut children, "expected `struct`");
        self.expect_identifier(&mut children, "expected struct name");
        if self.at_simple(&TokenKind::LBracket) {
            children.push(node(self.parse_generic_parameters()));
        }
        if self.at_simple(&TokenKind::LParen) {
            children.push(node(self.parse_derive_list()));
        }
        self.parse_item_block(
            &mut children,
            if foreign {
                MemberContext::ForeignStruct
            } else {
                MemberContext::Struct
            },
        );
        SyntaxNode::new(SyntaxKind::Struct, children, fallback)
    }

    fn parse_enum(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_keyword(Keyword::Enum, &mut children, "expected `enum`");
        self.expect_identifier(&mut children, "expected enum name");
        if self.at_simple(&TokenKind::LBracket) {
            children.push(node(self.parse_generic_parameters()));
        }
        if self.at_simple(&TokenKind::LParen) {
            children.push(node(self.parse_derive_list()));
        }
        self.parse_item_block(&mut children, MemberContext::Enum);
        SyntaxNode::new(SyntaxKind::Enum, children, fallback)
    }

    fn parse_trait(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_keyword(Keyword::Trait, &mut children, "expected `trait`");
        self.expect_identifier(&mut children, "expected trait name");
        if self.at_simple(&TokenKind::LBracket) {
            children.push(node(self.parse_generic_parameters()));
        }
        self.parse_item_block(&mut children, MemberContext::Trait);
        SyntaxNode::new(SyntaxKind::Trait, children, fallback)
    }

    fn parse_impl(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.expect_keyword(Keyword::Impl, &mut children, "expected `impl`");
        if self.at_simple(&TokenKind::LBracket) {
            children.push(node(self.parse_generic_parameters()));
        }
        children.push(node(self.parse_type()));
        self.expect_keyword(
            Keyword::For,
            &mut children,
            "expected `for` in trait implementation",
        );
        children.push(node(self.parse_type()));
        self.parse_item_block(&mut children, MemberContext::Impl);
        SyntaxNode::new(SyntaxKind::Impl, children, fallback)
    }

    fn parse_function(&mut self, body: FunctionBody) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        loop {
            if self.at_keyword(Keyword::Pub) || self.at_keyword(Keyword::Unsafe) {
                children.push(self.bump());
            } else {
                break;
            }
        }
        self.expect_keyword(Keyword::Fn, &mut children, "expected `fn`");
        self.expect_identifier(&mut children, "expected function name");
        if self.at_simple(&TokenKind::LBracket) {
            children.push(node(self.parse_generic_parameters()));
        }
        children.push(node(self.parse_parameters()));
        if self.at_simple(&TokenKind::Arrow) {
            children.push(self.bump());
            children.push(node(self.parse_type()));
        }
        match (body, self.at_simple(&TokenKind::Colon)) {
            (FunctionBody::Required | FunctionBody::Optional, true) => {
                self.parse_statement_block(&mut children);
            }
            (FunctionBody::Forbidden, true) => {
                self.error_here("a foreign function declaration cannot have a body");
                self.parse_statement_block(&mut children);
            }
            (FunctionBody::Required, false) => {
                self.error_here("a function definition requires an indented body");
                self.expect_line_end(&mut children);
            }
            (FunctionBody::Optional | FunctionBody::Forbidden, false) => {
                self.expect_line_end(&mut children);
            }
        }
        SyntaxNode::new(SyntaxKind::Function, children, fallback)
    }

    fn parse_generic_parameters(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.expect_simple(&TokenKind::LBracket, &mut children, "expected `[`");
        while !self.at_eof() && !self.at_simple(&TokenKind::RBracket) {
            let param_fallback = self.current_span();
            let mut param = Vec::new();
            self.expect_identifier(&mut param, "expected generic parameter name");
            if self.at_simple(&TokenKind::Colon) {
                param.push(self.bump());
                param.push(node(self.parse_type()));
                while self.at_simple(&TokenKind::Plus) {
                    param.push(self.bump());
                    param.push(node(self.parse_type()));
                }
            }
            children.push(node(SyntaxNode::new(
                SyntaxKind::GenericParameter,
                param,
                param_fallback,
            )));
            if !self.eat_simple(&TokenKind::Comma, &mut children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RBracket, &mut children, "expected `]`");
        SyntaxNode::new(SyntaxKind::GenericParameters, children, fallback)
    }

    fn parse_derive_list(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.expect_simple(&TokenKind::LParen, &mut children, "expected `(`");
        if self.at_simple(&TokenKind::RParen) {
            self.error_here("derive list cannot be empty");
        }
        while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
            self.parse_path_tokens(&mut children);
            if !self.eat_simple(&TokenKind::Comma, &mut children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
        SyntaxNode::new(SyntaxKind::DeriveList, children, fallback)
    }

    fn parse_parameters(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.expect_simple(&TokenKind::LParen, &mut children, "expected `(`");
        while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
            let param_fallback = self.current_span();
            let mut param = Vec::new();
            self.expect_parameter_name(&mut param);
            self.expect_simple(&TokenKind::Colon, &mut param, "expected `:`");
            self.eat_simple(&TokenKind::Ellipsis, &mut param);
            param.push(node(self.parse_type()));
            children.push(node(SyntaxNode::new(
                SyntaxKind::Parameter,
                param,
                param_fallback,
            )));
            if !self.eat_simple(&TokenKind::Comma, &mut children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
        SyntaxNode::new(SyntaxKind::Parameters, children, fallback)
    }

    fn parse_type(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        if self.at_simple(&TokenKind::Amp) || self.at_simple(&TokenKind::Star) {
            children.push(self.bump());
            self.eat_keyword(Keyword::Var, &mut children);
            if self.at_keyword(Keyword::Unsafe) {
                children.push(self.bump());
            }
            children.push(node(self.parse_type()));
            return SyntaxNode::new(SyntaxKind::Type, children, fallback);
        }
        if self.at_keyword(Keyword::Unsafe) {
            children.push(self.bump());
            if self.at_keyword(Keyword::Fn) {
                self.parse_function_type_tail(&mut children);
            } else {
                self.error_here("expected `fn` after `unsafe` in a function type");
            }
            return SyntaxNode::new(SyntaxKind::Type, children, fallback);
        }
        if self.at_keyword(Keyword::Fn) {
            self.parse_function_type_tail(&mut children);
            return SyntaxNode::new(SyntaxKind::Type, children, fallback);
        }
        if self.at_simple(&TokenKind::LBracket) {
            children.push(self.bump());
            children.push(node(self.parse_type()));
            if self.at_simple(&TokenKind::Semicolon) {
                children.push(self.bump());
                children.push(node(self.parse_expression()));
            }
            self.expect_simple(&TokenKind::RBracket, &mut children, "expected `]`");
            return SyntaxNode::new(SyntaxKind::Type, children, fallback);
        }
        if self.at_simple(&TokenKind::LParen) {
            children.push(self.bump());
            if !self.at_simple(&TokenKind::RParen) {
                children.push(node(self.parse_type()));
                while self.at_simple(&TokenKind::Comma) {
                    children.push(self.bump());
                    if self.at_simple(&TokenKind::RParen) {
                        break;
                    }
                    children.push(node(self.parse_type()));
                }
            }
            self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
            return SyntaxNode::new(SyntaxKind::Type, children, fallback);
        }
        if self.at_path_component() {
            self.parse_path_tokens(&mut children);
            if self.at_simple(&TokenKind::LBracket) {
                children.push(node(self.parse_type_arguments()));
            }
        } else {
            self.error_here("expected a type");
            if !self.at_type_boundary() {
                children.push(self.bump());
            }
        }
        SyntaxNode::new(SyntaxKind::Type, children, fallback)
    }

    fn parse_function_type_tail(&mut self, children: &mut Vec<SyntaxElement>) {
        self.expect_keyword(Keyword::Fn, children, "expected `fn`");
        self.expect_simple(&TokenKind::LParen, children, "expected `(`");
        while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
            self.eat_simple(&TokenKind::Ellipsis, children);
            children.push(node(self.parse_type()));
            if !self.eat_simple(&TokenKind::Comma, children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RParen, children, "expected `)`");
        if self.at_simple(&TokenKind::Arrow) {
            children.push(self.bump());
            children.push(node(self.parse_type()));
        } else {
            self.error_here("expected `->` in function type");
        }
    }

    fn parse_type_arguments(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.expect_simple(&TokenKind::LBracket, &mut children, "expected `[`");
        while !self.at_eof() && !self.at_simple(&TokenKind::RBracket) {
            children.push(node(self.parse_type()));
            if !self.eat_simple(&TokenKind::Comma, &mut children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RBracket, &mut children, "expected `]`");
        SyntaxNode::new(SyntaxKind::TypeArguments, children, fallback)
    }

    fn parse_item_block(&mut self, children: &mut Vec<SyntaxElement>, context: MemberContext) {
        let fallback = self.current_span();
        let mut block = Vec::new();
        self.expect_block_start(&mut block);
        let mut members = 0usize;
        while !self.at_eof() && !self.at_simple(&TokenKind::Dedent) {
            if self.at_simple(&TokenKind::Newline) {
                block.push(self.bump());
                continue;
            }
            let before = self.position;
            let member = if self.at_keyword(Keyword::Pass) {
                self.parse_simple_statement(SyntaxKind::PassStatement)
            } else {
                match context {
                    MemberContext::Module => self.parse_item(),
                    MemberContext::Struct => {
                        if self.looks_like_function() {
                            self.parse_function(FunctionBody::Required)
                        } else {
                            self.parse_field()
                        }
                    }
                    MemberContext::ForeignStruct => {
                        if self.looks_like_function() {
                            self.error_here("a foreign struct can contain only fields");
                            self.parse_function(FunctionBody::Required)
                        } else {
                            self.parse_field()
                        }
                    }
                    MemberContext::Enum => self.parse_enum_variant(),
                    MemberContext::Trait => self.parse_function(FunctionBody::Optional),
                    MemberContext::Impl => self.parse_function(FunctionBody::Required),
                }
            };
            block.push(node(member));
            members += 1;
            if self.position == before {
                block.push(self.bump());
            }
        }
        if members == 0 {
            self.error_here("an indented body cannot be empty; use `pass` where permitted");
        }
        self.expect_simple(
            &TokenKind::Dedent,
            &mut block,
            "expected end of indented body",
        );
        children.push(node(SyntaxNode::new(SyntaxKind::Block, block, fallback)));
    }

    fn parse_field(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.eat_keyword(Keyword::Pub, &mut children);
        self.expect_identifier(&mut children, "expected field name");
        self.expect_simple(&TokenKind::Colon, &mut children, "expected `:`");
        children.push(node(self.parse_type()));
        self.eat_simple(&TokenKind::Comma, &mut children);
        self.expect_line_end(&mut children);
        SyntaxNode::new(SyntaxKind::Field, children, fallback)
    }

    fn parse_enum_variant(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = self.take_item_prefix();
        self.expect_identifier(&mut children, "expected variant name");
        if self.at_simple(&TokenKind::LParen) {
            children.push(self.bump());
            if !self.at_simple(&TokenKind::RParen) {
                children.push(node(self.parse_type()));
                while self.at_simple(&TokenKind::Comma) {
                    children.push(self.bump());
                    if self.at_simple(&TokenKind::RParen) {
                        break;
                    }
                    children.push(node(self.parse_type()));
                }
            }
            self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
        } else if self.at_simple(&TokenKind::LBrace) {
            children.push(self.bump());
            while !self.at_eof() && !self.at_simple(&TokenKind::RBrace) {
                children.push(node(self.parse_inline_field()));
                if !self.eat_simple(&TokenKind::Comma, &mut children) {
                    break;
                }
            }
            self.expect_simple(&TokenKind::RBrace, &mut children, "expected `}`");
        }
        self.eat_simple(&TokenKind::Comma, &mut children);
        self.expect_line_end(&mut children);
        SyntaxNode::new(SyntaxKind::EnumVariant, children, fallback)
    }

    fn parse_inline_field(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.expect_identifier(&mut children, "expected field name");
        self.expect_simple(&TokenKind::Colon, &mut children, "expected `:`");
        children.push(node(self.parse_type()));
        SyntaxNode::new(SyntaxKind::Field, children, fallback)
    }

    fn parse_statement_block(&mut self, children: &mut Vec<SyntaxElement>) {
        let fallback = self.current_span();
        let mut block = Vec::new();
        self.expect_block_start(&mut block);
        let mut statements = 0usize;
        while !self.at_eof() && !self.at_simple(&TokenKind::Dedent) {
            if self.at_simple(&TokenKind::Newline) {
                block.push(self.bump());
                continue;
            }
            let before = self.position;
            block.push(node(self.parse_statement()));
            statements += 1;
            if self.position == before {
                block.push(self.bump());
            }
        }
        if statements == 0 {
            self.error_here("an indented body cannot be empty; use `pass`");
        }
        self.expect_simple(
            &TokenKind::Dedent,
            &mut block,
            "expected end of indented body",
        );
        children.push(node(SyntaxNode::new(SyntaxKind::Block, block, fallback)));
    }

    fn expect_block_start(&mut self, children: &mut Vec<SyntaxElement>) {
        self.expect_simple(&TokenKind::Colon, children, "expected `:` before body");
        self.expect_simple(
            &TokenKind::Newline,
            children,
            "body must begin on the following line",
        );
        self.expect_simple(
            &TokenKind::Indent,
            children,
            "expected a four-space indented body",
        );
    }

    fn parse_statement(&mut self) -> SyntaxNode {
        match self.current_kind() {
            Some(TokenKind::Keyword(Keyword::Let | Keyword::Var)) => self.parse_let(),
            Some(TokenKind::Keyword(Keyword::Return)) => self.parse_return(),
            Some(TokenKind::Keyword(Keyword::Break)) => {
                self.parse_simple_statement(SyntaxKind::BreakStatement)
            }
            Some(TokenKind::Keyword(Keyword::Continue)) => {
                self.parse_simple_statement(SyntaxKind::ContinueStatement)
            }
            Some(TokenKind::Keyword(Keyword::Pass)) => {
                self.parse_simple_statement(SyntaxKind::PassStatement)
            }
            Some(TokenKind::Keyword(Keyword::Defer)) => self.parse_defer(),
            Some(TokenKind::Keyword(Keyword::If)) => self.parse_if(),
            Some(TokenKind::Keyword(Keyword::Match)) => self.parse_match(),
            Some(TokenKind::Keyword(Keyword::For)) => self.parse_for(),
            Some(TokenKind::Keyword(Keyword::While)) => self.parse_while(),
            Some(TokenKind::Keyword(Keyword::Unsafe))
                if matches!(self.kind_at(1), Some(TokenKind::Colon)) =>
            {
                self.parse_unsafe_block()
            }
            _ => self.parse_expression_or_assignment_statement(),
        }
    }

    fn parse_let(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        self.expect_identifier(&mut children, "expected binding name");
        if self.at_simple(&TokenKind::Colon) {
            children.push(self.bump());
            children.push(node(self.parse_type()));
        }
        self.expect_simple(&TokenKind::Assign, &mut children, "expected `=`");
        children.push(node(self.parse_expression()));
        self.expect_line_end(&mut children);
        SyntaxNode::new(SyntaxKind::LetStatement, children, fallback)
    }

    fn parse_return(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        if !self.at_line_end() {
            children.push(node(self.parse_expression()));
        }
        self.expect_line_end(&mut children);
        SyntaxNode::new(SyntaxKind::ReturnStatement, children, fallback)
    }

    fn parse_simple_statement(&mut self, kind: SyntaxKind) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        self.expect_line_end(&mut children);
        SyntaxNode::new(kind, children, fallback)
    }

    fn parse_defer(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let keyword = self.bump();
        // `defer:` defers a block of statements; `defer call` defers one call.
        if self.at_simple(&TokenKind::Colon) {
            let mut children = vec![keyword];
            self.parse_statement_block(&mut children);
            return SyntaxNode::new(SyntaxKind::DeferStatement, children, fallback);
        }
        let call = self.parse_expression();
        if call.kind != SyntaxKind::CallExpression {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Syntax,
                    "`defer` requires a single function or method call, or a `defer:` block",
                )
                .with_primary(call.span),
            );
        }
        let mut children = vec![keyword, node(call)];
        self.expect_line_end(&mut children);
        SyntaxNode::new(SyntaxKind::DeferStatement, children, fallback)
    }

    fn parse_if(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump(), node(self.parse_expression())];
        self.parse_statement_block(&mut children);
        if self.at_keyword(Keyword::Else) {
            let else_fallback = self.current_span();
            let mut else_children = vec![self.bump()];
            self.parse_statement_block(&mut else_children);
            children.push(node(SyntaxNode::new(
                SyntaxKind::ElseClause,
                else_children,
                else_fallback,
            )));
        }
        SyntaxNode::new(SyntaxKind::IfStatement, children, fallback)
    }

    fn parse_match(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump(), node(self.parse_expression())];
        let block_fallback = self.current_span();
        let mut block = Vec::new();
        self.expect_block_start(&mut block);
        let mut arms = 0;
        while !self.at_eof() && !self.at_simple(&TokenKind::Dedent) {
            if self.at_simple(&TokenKind::Newline) {
                block.push(self.bump());
                continue;
            }
            block.push(node(self.parse_match_arm()));
            arms += 1;
        }
        if arms == 0 {
            self.error_here("a match body requires at least one arm");
        }
        self.expect_simple(&TokenKind::Dedent, &mut block, "expected end of match body");
        children.push(node(SyntaxNode::new(
            SyntaxKind::Block,
            block,
            block_fallback,
        )));
        SyntaxNode::new(SyntaxKind::MatchStatement, children, fallback)
    }

    fn parse_match_arm(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![node(self.parse_pattern())];
        if self.at_keyword(Keyword::If) {
            let guard_fallback = self.current_span();
            let guard = vec![self.bump(), node(self.parse_expression())];
            children.push(node(SyntaxNode::new(
                SyntaxKind::Guard,
                guard,
                guard_fallback,
            )));
        }
        self.parse_statement_block(&mut children);
        SyntaxNode::new(SyntaxKind::MatchArm, children, fallback)
    }

    fn parse_for(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        self.expect_identifier(&mut children, "expected loop binding");
        self.expect_keyword(Keyword::In, &mut children, "expected `in`");
        children.push(node(self.parse_expression()));
        self.parse_statement_block(&mut children);
        SyntaxNode::new(SyntaxKind::ForStatement, children, fallback)
    }

    fn parse_while(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump(), node(self.parse_expression())];
        self.parse_statement_block(&mut children);
        SyntaxNode::new(SyntaxKind::WhileStatement, children, fallback)
    }

    fn parse_unsafe_block(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        self.parse_statement_block(&mut children);
        SyntaxNode::new(SyntaxKind::UnsafeBlock, children, fallback)
    }

    fn parse_expression_or_assignment_statement(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![node(self.parse_expression())];
        let kind = if self.at_assignment_operator() {
            children.push(self.bump());
            children.push(node(self.parse_expression()));
            SyntaxKind::AssignmentStatement
        } else {
            SyntaxKind::ExpressionStatement
        };
        self.expect_line_end(&mut children);
        SyntaxNode::new(kind, children, fallback)
    }

    fn parse_expression(&mut self) -> SyntaxNode {
        self.parse_expression_bp(0)
    }

    fn parse_expression_bp(&mut self, minimum: u8) -> SyntaxNode {
        let fallback = self.current_span();
        let mut left = self.parse_prefix_expression();

        loop {
            if let Some(postfix) = self.parse_postfix(left.clone()) {
                left = postfix;
                continue;
            }

            if self.at_keyword(Keyword::As) {
                let precedence = 12;
                if precedence < minimum {
                    break;
                }
                let mut children = vec![node(left), self.bump(), node(self.parse_type())];
                left = SyntaxNode::new(
                    SyntaxKind::CastExpression,
                    std::mem::take(&mut children),
                    fallback,
                );
                continue;
            }

            let Some((left_bp, right_bp, is_comparison)) = self.binary_binding_power() else {
                break;
            };
            if left_bp < minimum {
                break;
            }
            let operator = self.bump();
            let right = self.parse_expression_bp(right_bp);
            if is_comparison
                && (is_comparison_expression(&left) || is_comparison_expression(&right))
            {
                self.diagnostics.push(
                    Diagnostic::new(Category::Syntax, "chained comparisons are invalid")
                        .with_primary(operator.span()),
                );
            }
            left = SyntaxNode::new(
                SyntaxKind::BinaryExpression,
                vec![node(left), operator, node(right)],
                fallback,
            );
        }
        left
    }

    fn parse_prefix_expression(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        match self.current_kind() {
            Some(
                TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Amp,
            ) => {
                let mut children = vec![self.bump()];
                if matches!(
                    children[0],
                    SyntaxElement::Token(Token {
                        kind: TokenKind::Amp,
                        ..
                    })
                ) {
                    self.eat_keyword(Keyword::Var, &mut children);
                }
                children.push(node(self.parse_expression_bp(13)));
                SyntaxNode::new(SyntaxKind::UnaryExpression, children, fallback)
            }
            Some(
                TokenKind::IntegerLiteral { .. }
                | TokenKind::FloatLiteral { .. }
                | TokenKind::StringLiteral(_)
                | TokenKind::CharacterLiteral(_)
                | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Null),
            ) => SyntaxNode::new(SyntaxKind::LiteralExpression, vec![self.bump()], fallback),
            Some(TokenKind::FormattedString(_)) => self.parse_formatted_string_expression(),
            Some(TokenKind::LParen) => self.parse_parenthesized_or_tuple(),
            Some(TokenKind::LBracket) => self.parse_array_expression(),
            Some(TokenKind::At) => self.parse_macro_expression(),
            Some(_) if self.at_path_component() => {
                SyntaxNode::new(SyntaxKind::NameExpression, vec![self.bump()], fallback)
            }
            _ => {
                self.error_here("expected an expression");
                let mut children = Vec::new();
                if !self.at_expression_boundary() {
                    children.push(self.bump());
                }
                SyntaxNode::new(SyntaxKind::Error, children, fallback)
            }
        }
    }

    fn parse_formatted_string_expression(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let token = self.current().cloned();
        let mut children = vec![self.bump()];
        if let Some(Token {
            kind: TokenKind::FormattedString(segments),
            ..
        }) = token
        {
            for segment in segments {
                if let FormattedSegmentKind::Expression { tokens, .. } = segment.kind {
                    if tokens.is_empty() {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::Syntax,
                                "formatted-string interpolation requires an expression",
                            )
                            .with_primary(segment.span),
                        );
                        continue;
                    }
                    let mut nested = Parser::new(&tokens);
                    let expression = nested.parse_expression();
                    if nested.position < tokens.len() {
                        nested.error_here("unexpected token in formatted-string interpolation");
                    }
                    self.diagnostics.extend(nested.diagnostics);
                    children.push(node(expression));
                }
            }
        }
        SyntaxNode::new(SyntaxKind::FormattedStringExpression, children, fallback)
    }

    fn parse_parenthesized_or_tuple(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        if self.at_simple(&TokenKind::RParen) {
            children.push(self.bump());
            return SyntaxNode::new(SyntaxKind::TupleExpression, children, fallback);
        }
        children.push(node(self.parse_expression()));
        let mut tuple = false;
        while self.at_simple(&TokenKind::Comma) {
            tuple = true;
            children.push(self.bump());
            if self.at_simple(&TokenKind::RParen) {
                break;
            }
            children.push(node(self.parse_expression()));
        }
        self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
        SyntaxNode::new(
            if tuple {
                SyntaxKind::TupleExpression
            } else {
                SyntaxKind::ParenthesizedExpression
            },
            children,
            fallback,
        )
    }

    fn parse_array_expression(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        while !self.at_eof() && !self.at_simple(&TokenKind::RBracket) {
            children.push(node(self.parse_expression()));
            if !self.eat_simple(&TokenKind::Comma, &mut children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RBracket, &mut children, "expected `]`");
        SyntaxNode::new(SyntaxKind::ArrayExpression, children, fallback)
    }

    fn parse_macro_expression(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        let macro_name = match self.current_kind() {
            Some(TokenKind::Identifier(name)) => Some(name.clone()),
            _ => None,
        };
        self.expect_identifier(&mut children, "expected macro name after `@`");
        match macro_name.as_deref() {
            Some("vec") => {
                self.parse_macro_sequence(&mut children, TokenKind::LBracket, TokenKind::RBracket);
            }
            Some("map") => {
                self.expect_simple(
                    &TokenKind::LBrace,
                    &mut children,
                    "expected `{` after `@map`",
                );
                while !self.at_eof() && !self.at_simple(&TokenKind::RBrace) {
                    children.push(node(self.parse_expression()));
                    self.expect_simple(
                        &TokenKind::Colon,
                        &mut children,
                        "expected `:` between map key and value",
                    );
                    children.push(node(self.parse_expression()));
                    if !self.eat_simple(&TokenKind::Comma, &mut children) {
                        break;
                    }
                }
                self.expect_simple(&TokenKind::RBrace, &mut children, "expected `}`");
            }
            Some("set") => {
                self.parse_macro_sequence(&mut children, TokenKind::LBrace, TokenKind::RBrace);
            }
            Some(name) => {
                self.error_here(format!(
                    "unknown compiler macro `@{name}`; expected `@vec`, `@map`, or `@set`"
                ));
                self.recover_macro_invocation(&mut children);
            }
            None => self.recover_macro_invocation(&mut children),
        }
        SyntaxNode::new(SyntaxKind::MacroExpression, children, fallback)
    }

    fn parse_macro_sequence(
        &mut self,
        children: &mut Vec<SyntaxElement>,
        open: TokenKind,
        close: TokenKind,
    ) {
        self.expect_simple(&open, children, "expected collection macro delimiter");
        while !self.at_eof() && !self.at_simple(&close) {
            children.push(node(self.parse_expression()));
            if !self.eat_simple(&TokenKind::Comma, children) {
                break;
            }
        }
        self.expect_simple(
            &close,
            children,
            "expected collection macro closing delimiter",
        );
    }

    fn recover_macro_invocation(&mut self, children: &mut Vec<SyntaxElement>) {
        let (open, close) = if self.at_simple(&TokenKind::LBracket) {
            (TokenKind::LBracket, TokenKind::RBracket)
        } else if self.at_simple(&TokenKind::LBrace) {
            (TokenKind::LBrace, TokenKind::RBrace)
        } else {
            self.error_here("expected `[` or `{` after macro name");
            return;
        };
        self.parse_macro_sequence(children, open, close);
    }

    fn parse_postfix(&mut self, left: SyntaxNode) -> Option<SyntaxNode> {
        let fallback = left.span;
        if self.at_simple(&TokenKind::Dot) {
            let mut children = vec![node(left), self.bump()];
            self.expect_identifier(&mut children, "expected member name after `.`");
            return Some(SyntaxNode::new(
                SyntaxKind::MemberExpression,
                children,
                fallback,
            ));
        }
        if self.at_simple(&TokenKind::LParen) {
            let mut children = vec![node(left), self.bump()];
            while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
                children.push(node(self.parse_expression()));
                if !self.eat_simple(&TokenKind::Comma, &mut children) {
                    break;
                }
            }
            self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
            return Some(SyntaxNode::new(
                SyntaxKind::CallExpression,
                children,
                fallback,
            ));
        }
        if self.at_simple(&TokenKind::LBracket) {
            let mut children = vec![node(left), self.bump()];
            if !self.at_simple(&TokenKind::RBracket) {
                children.push(node(self.parse_expression()));
                while self.at_simple(&TokenKind::Comma) {
                    children.push(self.bump());
                    children.push(node(self.parse_expression()));
                }
            }
            self.expect_simple(&TokenKind::RBracket, &mut children, "expected `]`");
            return Some(SyntaxNode::new(
                SyntaxKind::BracketExpression,
                children,
                fallback,
            ));
        }
        if self.at_simple(&TokenKind::Question) {
            return Some(SyntaxNode::new(
                SyntaxKind::TryExpression,
                vec![node(left), self.bump()],
                fallback,
            ));
        }
        if self.at_simple(&TokenKind::LBrace)
            && matches!(
                left.kind,
                SyntaxKind::NameExpression
                    | SyntaxKind::MemberExpression
                    | SyntaxKind::BracketExpression
            )
        {
            let mut children = vec![node(left), self.bump()];
            while !self.at_eof() && !self.at_simple(&TokenKind::RBrace) {
                children.push(node(self.parse_record_field()));
                if !self.eat_simple(&TokenKind::Comma, &mut children) {
                    break;
                }
            }
            self.expect_simple(&TokenKind::RBrace, &mut children, "expected `}`");
            return Some(SyntaxNode::new(
                SyntaxKind::RecordExpression,
                children,
                fallback,
            ));
        }
        None
    }

    fn parse_record_field(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = Vec::new();
        self.expect_identifier(&mut children, "expected record field name");
        if self.at_simple(&TokenKind::Colon) {
            children.push(self.bump());
            children.push(node(self.parse_expression()));
        }
        SyntaxNode::new(SyntaxKind::RecordField, children, fallback)
    }

    fn binary_binding_power(&self) -> Option<(u8, u8, bool)> {
        Some(match self.current_kind()? {
            TokenKind::OrOr => (1, 2, false),
            TokenKind::AndAnd => (2, 3, false),
            TokenKind::EqEq | TokenKind::NotEq => (3, 4, true),
            TokenKind::Less | TokenKind::LessEq | TokenKind::Greater | TokenKind::GreaterEq => {
                (4, 5, true)
            }
            TokenKind::Pipe => (5, 6, false),
            TokenKind::Caret => (6, 7, false),
            TokenKind::Amp => (7, 8, false),
            TokenKind::Shl | TokenKind::Shr => (8, 9, false),
            TokenKind::Plus | TokenKind::Minus => (9, 10, false),
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent => (10, 11, false),
            _ => return None,
        })
    }

    fn parse_pattern(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let first = self.parse_pattern_atom();
        if !self.at_simple(&TokenKind::Pipe) {
            return SyntaxNode::new(SyntaxKind::Pattern, vec![node(first)], fallback);
        }
        let mut children = vec![node(first)];
        while self.at_simple(&TokenKind::Pipe) {
            children.push(self.bump());
            children.push(node(self.parse_pattern_atom()));
        }
        SyntaxNode::new(SyntaxKind::AlternativePattern, children, fallback)
    }

    fn parse_pattern_atom(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        if self.at_simple(&TokenKind::Star) {
            let children = vec![self.bump(), node(self.parse_pattern_atom())];
            return SyntaxNode::new(SyntaxKind::DereferencePattern, children, fallback);
        }
        if self.at_simple(&TokenKind::LParen) {
            let mut children = vec![self.bump()];
            while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
                children.push(node(self.parse_pattern()));
                if !self.eat_simple(&TokenKind::Comma, &mut children) {
                    break;
                }
            }
            self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
            return SyntaxNode::new(SyntaxKind::TuplePattern, children, fallback);
        }
        if matches!(
            self.current_kind(),
            Some(
                TokenKind::IntegerLiteral { .. }
                    | TokenKind::FloatLiteral { .. }
                    | TokenKind::StringLiteral(_)
                    | TokenKind::CharacterLiteral(_)
                    | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Null)
            )
        ) {
            return SyntaxNode::new(SyntaxKind::Pattern, vec![self.bump()], fallback);
        }
        if self.at_path_component() {
            let mut children = Vec::new();
            self.parse_path_tokens(&mut children);
            if self.at_simple(&TokenKind::LParen) {
                children.push(self.bump());
                while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
                    children.push(node(self.parse_pattern()));
                    if !self.eat_simple(&TokenKind::Comma, &mut children) {
                        break;
                    }
                }
                self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
                return SyntaxNode::new(SyntaxKind::VariantPattern, children, fallback);
            }
            if self.at_simple(&TokenKind::LBrace) {
                children.push(self.bump());
                while !self.at_eof() && !self.at_simple(&TokenKind::RBrace) {
                    let field_fallback = self.current_span();
                    let mut field = Vec::new();
                    if self.at_simple(&TokenKind::DotDot) {
                        field.push(self.bump());
                    } else {
                        self.expect_identifier(&mut field, "expected pattern field");
                        if self.at_simple(&TokenKind::Colon) {
                            field.push(self.bump());
                            field.push(node(self.parse_pattern()));
                        }
                    }
                    children.push(node(SyntaxNode::new(
                        SyntaxKind::PatternField,
                        field,
                        field_fallback,
                    )));
                    if !self.eat_simple(&TokenKind::Comma, &mut children) {
                        break;
                    }
                }
                self.expect_simple(&TokenKind::RBrace, &mut children, "expected `}`");
                return SyntaxNode::new(SyntaxKind::RecordPattern, children, fallback);
            }
            return SyntaxNode::new(SyntaxKind::Pattern, children, fallback);
        }
        self.error_here("expected a pattern");
        let children = if self.at_expression_boundary() {
            Vec::new()
        } else {
            vec![self.bump()]
        };
        SyntaxNode::new(SyntaxKind::Error, children, fallback)
    }

    fn looks_like_function(&self) -> bool {
        matches!(self.classify_item(), ItemHead::Function)
    }

    fn parse_path_tokens(&mut self, children: &mut Vec<SyntaxElement>) {
        self.expect_name(children, "expected path component");
        while self.at_simple(&TokenKind::Dot) {
            children.push(self.bump());
            self.expect_identifier(children, "expected path component after `.`");
        }
    }

    fn take_docs(&mut self) -> Vec<SyntaxElement> {
        let mut children = Vec::new();
        if !matches!(self.current_kind(), Some(TokenKind::DocComment(_))) {
            return children;
        }
        let fallback = self.current_span();
        let mut documentation = Vec::new();
        while matches!(self.current_kind(), Some(TokenKind::DocComment(_))) {
            documentation.push(self.bump());
            self.eat_simple(&TokenKind::Newline, &mut documentation);
        }
        children.push(node(SyntaxNode::new(
            SyntaxKind::Documentation,
            documentation,
            fallback,
        )));
        children
    }

    fn take_item_prefix(&mut self) -> Vec<SyntaxElement> {
        let mut children = self.take_docs();
        while self.at_simple(&TokenKind::At) {
            children.push(node(self.parse_attribute()));
            self.eat_simple(&TokenKind::Newline, &mut children);
        }
        children
    }

    fn parse_attribute(&mut self) -> SyntaxNode {
        let fallback = self.current_span();
        let mut children = vec![self.bump()];
        self.expect_identifier(&mut children, "expected attribute name after `@`");
        self.expect_simple(
            &TokenKind::LParen,
            &mut children,
            "expected `(` after attribute name",
        );
        while !self.at_eof() && !self.at_simple(&TokenKind::RParen) {
            if matches!(self.current_kind(), Some(TokenKind::StringLiteral(_))) {
                children.push(self.bump());
            } else {
                self.error_here("attribute arguments must be string literals");
                if !self.at_simple(&TokenKind::Comma) {
                    children.push(self.bump());
                }
            }
            if !self.eat_simple(&TokenKind::Comma, &mut children) {
                break;
            }
        }
        self.expect_simple(&TokenKind::RParen, &mut children, "expected `)`");
        SyntaxNode::new(SyntaxKind::Attribute, children, fallback)
    }

    fn leading_attribute_named(&self, expected: &str) -> bool {
        let mut offset = 0usize;
        while matches!(
            self.kind_at(offset),
            Some(TokenKind::DocComment(_) | TokenKind::Newline)
        ) {
            offset += 1;
        }
        while matches!(self.kind_at(offset), Some(TokenKind::At)) {
            if matches!(
                self.kind_at(offset + 1),
                Some(TokenKind::Identifier(name)) if name == expected
            ) {
                return true;
            }
            while !matches!(
                self.kind_at(offset),
                Some(TokenKind::Newline | TokenKind::Eof) | None
            ) {
                offset += 1;
            }
            if matches!(self.kind_at(offset), Some(TokenKind::Newline)) {
                offset += 1;
            }
        }
        false
    }

    fn expect_line_end(&mut self, children: &mut Vec<SyntaxElement>) {
        if !self.eat_simple(&TokenKind::Newline, children)
            && !self.at_simple(&TokenKind::Dedent)
            && !self.at_eof()
        {
            self.error_here("expected end of statement");
            self.recover_line(children);
        }
    }

    fn recover_line(&mut self, children: &mut Vec<SyntaxElement>) {
        while !self.at_eof()
            && !self.at_simple(&TokenKind::Newline)
            && !self.at_simple(&TokenKind::Dedent)
        {
            children.push(self.bump());
        }
        self.eat_simple(&TokenKind::Newline, children);
    }

    fn eat_newlines(&mut self, children: &mut Vec<SyntaxElement>) {
        while self.at_simple(&TokenKind::Newline) {
            children.push(self.bump());
        }
    }

    fn at_assignment_operator(&self) -> bool {
        matches!(
            self.current_kind(),
            Some(
                TokenKind::Assign
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
            )
        )
    }

    fn at_line_end(&self) -> bool {
        self.at_simple(&TokenKind::Newline) || self.at_simple(&TokenKind::Dedent) || self.at_eof()
    }

    fn at_expression_boundary(&self) -> bool {
        self.at_line_end()
            || matches!(
                self.current_kind(),
                Some(
                    TokenKind::Comma
                        | TokenKind::Colon
                        | TokenKind::RParen
                        | TokenKind::RBracket
                        | TokenKind::RBrace
                )
            )
    }

    fn at_type_boundary(&self) -> bool {
        self.at_expression_boundary()
            || matches!(
                self.current_kind(),
                Some(TokenKind::Assign | TokenKind::Arrow | TokenKind::Plus)
            )
    }

    fn at_path_component(&self) -> bool {
        matches!(
            self.current_kind(),
            Some(
                TokenKind::Identifier(_)
                    | TokenKind::Keyword(
                        Keyword::Root | Keyword::SelfValue | Keyword::SelfType | Keyword::Super
                    )
            )
        )
    }

    fn expect_identifier(&mut self, children: &mut Vec<SyntaxElement>, message: &str) {
        if matches!(self.current_kind(), Some(TokenKind::Identifier(_))) {
            children.push(self.bump());
        } else {
            self.error_here(message);
        }
    }

    fn expect_name(&mut self, children: &mut Vec<SyntaxElement>, message: &str) {
        if self.at_path_component() {
            children.push(self.bump());
        } else {
            self.error_here(message);
        }
    }

    fn expect_parameter_name(&mut self, children: &mut Vec<SyntaxElement>) {
        if matches!(
            self.current_kind(),
            Some(TokenKind::Identifier(_) | TokenKind::Keyword(Keyword::SelfValue))
        ) {
            children.push(self.bump());
        } else {
            self.error_here("expected parameter name");
        }
    }

    fn expect_keyword(
        &mut self,
        keyword: Keyword,
        children: &mut Vec<SyntaxElement>,
        message: &str,
    ) {
        if self.at_keyword(keyword) {
            children.push(self.bump());
        } else {
            self.error_here(message);
        }
    }

    fn eat_keyword(&mut self, keyword: Keyword, children: &mut Vec<SyntaxElement>) -> bool {
        if self.at_keyword(keyword) {
            children.push(self.bump());
            true
        } else {
            false
        }
    }

    fn expect_simple(
        &mut self,
        expected: &TokenKind,
        children: &mut Vec<SyntaxElement>,
        message: &str,
    ) {
        if self.at_simple(expected) {
            children.push(self.bump());
        } else {
            self.error_here(message);
        }
    }

    fn eat_simple(&mut self, expected: &TokenKind, children: &mut Vec<SyntaxElement>) -> bool {
        if self.at_simple(expected) {
            children.push(self.bump());
            true
        } else {
            false
        }
    }

    fn at_simple(&self, expected: &TokenKind) -> bool {
        self.current_kind()
            .is_some_and(|actual| discriminant(actual) == discriminant(expected))
    }

    fn at_keyword(&self, keyword: Keyword) -> bool {
        matches!(self.current_kind(), Some(TokenKind::Keyword(actual)) if *actual == keyword)
    }

    fn current(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn current_kind(&self) -> Option<&TokenKind> {
        self.current().map(|token| &token.kind)
    }

    fn kind_at(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens
            .get(self.position + offset)
            .map(|token| &token.kind)
    }

    fn current_span(&self) -> Span {
        self.current()
            .map(|token| token.span)
            .or_else(|| self.tokens.last().map(|token| token.span))
            .expect("parser requires at least an EOF token")
    }

    fn at_eof(&self) -> bool {
        self.position >= self.tokens.len() || self.at_simple(&TokenKind::Eof)
    }

    fn bump(&mut self) -> SyntaxElement {
        let token = self
            .tokens
            .get(self.position)
            .cloned()
            .expect("cannot bump past end of token stream");
        self.position += 1;
        SyntaxElement::Token(token)
    }

    fn error_here(&mut self, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(Category::Syntax, message).with_primary(self.current_span()));
    }
}

#[derive(Debug, Clone, Copy)]
enum MemberContext {
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    ForeignStruct,
}

#[derive(Debug, Clone, Copy)]
enum FunctionBody {
    Required,
    Optional,
    Forbidden,
}

fn node(node: SyntaxNode) -> SyntaxElement {
    SyntaxElement::Node(Box::new(node))
}

fn is_comparison_expression(expression: &SyntaxNode) -> bool {
    expression.kind == SyntaxKind::BinaryExpression
        && expression.children.iter().any(|child| {
            matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::Less
                        | TokenKind::LessEq
                        | TokenKind::Greater
                        | TokenKind::GreaterEq
                        | TokenKind::EqEq
                        | TokenKind::NotEq,
                    ..
                })
            )
        })
}
