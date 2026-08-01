//! Typed quote-role inference and structural validation.
//!
//! Parsing records a role-neutral quote template. This layer recognizes the
//! public `std.ast` type expected by an explicitly annotated binding or a
//! compile-time declaration return, adapts interpolation sites to temporary
//! parser holes, and sends the result back through the ordinary hand-written
//! grammar. It never converts a compiler `SyntaxNode` into the public AST
//! façade; that conversion belongs to interpreter lowering.

use crate::config::CompilerFeatures;
use crate::diagnostics::{Category, Diagnostic};
use crate::parser::{ParseOutput, QuoteFragmentKind, parse_quote_fragment};
use crate::source::Span;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind};

use super::ExpandedUnit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteRole {
    Expression,
    Pattern,
    TypeSyntax,
    StatementList,
    MemberList,
    Item,
    ItemList,
    StructDefinition,
    EnumDefinition,
    FunctionDefinition,
    Implementation,
    FieldDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpolationForm {
    Named(String),
    Computed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolationPosition {
    Scalar,
    Collection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteInterpolation {
    pub form: InterpolationForm,
    pub position: InterpolationPosition,
    pub span: Span,
}

impl QuoteRole {
    fn fragment(self) -> QuoteFragmentKind {
        match self {
            Self::Expression => QuoteFragmentKind::Expression,
            Self::Pattern => QuoteFragmentKind::Pattern,
            Self::TypeSyntax => QuoteFragmentKind::Type,
            Self::StatementList => QuoteFragmentKind::StatementList,
            Self::MemberList => QuoteFragmentKind::MemberList,
            Self::FieldDefinition => QuoteFragmentKind::Member,
            Self::Item
            | Self::StructDefinition
            | Self::EnumDefinition
            | Self::FunctionDefinition
            | Self::Implementation => QuoteFragmentKind::Item,
            Self::ItemList => QuoteFragmentKind::ItemList,
        }
    }

    fn collection(self) -> bool {
        matches!(
            self,
            Self::StatementList | Self::MemberList | Self::ItemList
        )
    }
}

/// Recognizes one exact public AST role from written, unresolved type syntax.
#[must_use]
pub fn role_from_type(node: &SyntaxNode) -> Option<QuoteRole> {
    if node.kind != SyntaxKind::Type || !node.direct_nodes().is_empty() {
        return None;
    }
    let path = node
        .direct_tokens()
        .into_iter()
        .filter_map(|token| match &token.kind {
            TokenKind::Identifier(text) => Some(text.as_str()),
            TokenKind::Dot => Some("."),
            _ => None,
        })
        .collect::<String>();
    Some(match path.as_str() {
        "std.ast.Expression" => QuoteRole::Expression,
        "std.ast.Pattern" => QuoteRole::Pattern,
        "std.ast.TypeSyntax" => QuoteRole::TypeSyntax,
        "std.ast.StatementList" => QuoteRole::StatementList,
        "std.ast.MemberList" => QuoteRole::MemberList,
        "std.ast.Item" => QuoteRole::Item,
        "std.ast.ItemList" => QuoteRole::ItemList,
        "std.ast.StructDefinition" => QuoteRole::StructDefinition,
        "std.ast.EnumDefinition" => QuoteRole::EnumDefinition,
        "std.ast.FunctionDefinition" => QuoteRole::FunctionDefinition,
        "std.ast.Implementation" => QuoteRole::Implementation,
        "std.ast.FieldDefinition" => QuoteRole::FieldDefinition,
        _ => return None,
    })
}

/// Validates one parsed quote template in an explicit role.
#[must_use]
pub fn validate_quote(node: &SyntaxNode, role: QuoteRole) -> ParseOutput {
    assert_eq!(
        node.kind,
        SyntaxKind::QuoteExpression,
        "typed quote validation requires a quote expression"
    );
    let body = node
        .direct_child(SyntaxKind::QuoteBody)
        .expect("the parser always attaches a quote body");
    let tokens = adapted_body_tokens(body, role);
    let mut output = parse_quote_fragment(&tokens, role.fragment());
    if output.diagnostics.is_empty()
        && let Some(expected) = role.expected_syntax_kind()
        && output.tree.kind != expected
    {
        output.diagnostics.push(
            Diagnostic::new(
                Category::CompileTime,
                format!(
                    "this quote must produce {}, but its body produces {:?}",
                    role.description(),
                    output.tree.kind
                ),
            )
            .with_primary(body.span),
        );
    }
    output
}

/// Returns interpolation sites in source order, including whether each value
/// inserts one node or splices an AST list in a collection position.
#[must_use]
pub fn interpolations(node: &SyntaxNode, role: QuoteRole) -> Vec<QuoteInterpolation> {
    assert_eq!(node.kind, SyntaxKind::QuoteExpression);
    let Some(body) = node.direct_child(SyntaxKind::QuoteBody) else {
        return Vec::new();
    };
    body.children
        .iter()
        .enumerate()
        .filter_map(|(index, child)| {
            let SyntaxElement::Node(interpolation) = child else {
                return None;
            };
            if interpolation.kind != SyntaxKind::QuoteInterpolation {
                return None;
            }
            let form = interpolation
                .direct_tokens()
                .into_iter()
                .find_map(|token| match &token.kind {
                    TokenKind::Identifier(name) => Some(InterpolationForm::Named(name.clone())),
                    _ => None,
                })
                .unwrap_or(InterpolationForm::Computed);
            Some(QuoteInterpolation {
                form,
                position: if is_collection_position(&body.children, index, role) {
                    InterpolationPosition::Collection
                } else {
                    InterpolationPosition::Scalar
                },
                span: interpolation.span,
            })
        })
        .collect()
}

impl QuoteRole {
    fn expected_syntax_kind(self) -> Option<SyntaxKind> {
        Some(match self {
            Self::StructDefinition => SyntaxKind::Struct,
            Self::EnumDefinition => SyntaxKind::Enum,
            Self::FunctionDefinition => SyntaxKind::Function,
            Self::Implementation => SyntaxKind::Impl,
            Self::FieldDefinition => SyntaxKind::Field,
            _ => return None,
        })
    }

    fn description(self) -> &'static str {
        match self {
            Self::StructDefinition => "a struct definition",
            Self::EnumDefinition => "an enum definition",
            Self::FunctionDefinition => "a function definition",
            Self::Implementation => "an implementation",
            Self::FieldDefinition => "a field definition",
            _ => "the requested AST role",
        }
    }
}

fn adapted_body_tokens(body: &SyntaxNode, role: QuoteRole) -> Vec<Token> {
    let mut tokens = Vec::new();
    let last = body.children.len().saturating_sub(1);
    for (index, child) in body.children.iter().enumerate() {
        match child {
            SyntaxElement::Token(token)
                if (index == 0 && matches!(token.kind, TokenKind::Indent))
                    || (index == last && matches!(token.kind, TokenKind::Dedent)) => {}
            SyntaxElement::Token(token) => tokens.push(token.clone()),
            SyntaxElement::Node(interpolation)
                if interpolation.kind == SyntaxKind::QuoteInterpolation =>
            {
                if !is_collection_position(&body.children, index, role) {
                    tokens.push(Token {
                        kind: TokenKind::Identifier("__elamite_quote_hole".to_string()),
                        span: interpolation.span,
                    });
                }
            }
            SyntaxElement::Node(_) => {
                unreachable!("quote bodies contain only tokens and interpolation nodes")
            }
        }
    }
    let boundary = tokens.last().map_or(body.span, |token| token.span);
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(boundary.file, boundary.end, boundary.end),
    });
    tokens
}

fn is_collection_position(children: &[SyntaxElement], index: usize, role: QuoteRole) -> bool {
    if !role.collection() && role != QuoteRole::Item {
        return false;
    }
    let previous = children[..index].iter().rev().find_map(element_token_kind);
    let next = children[index + 1..].iter().find_map(element_token_kind);
    matches!(
        previous,
        None | Some(TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline)
    ) && matches!(next, None | Some(TokenKind::Dedent | TokenKind::Newline))
}

fn element_token_kind(element: &SyntaxElement) -> Option<&TokenKind> {
    match element {
        SyntaxElement::Token(token) => Some(&token.kind),
        SyntaxElement::Node(_) => None,
    }
}

pub(super) fn validate(
    units: &[ExpandedUnit],
    features: CompilerFeatures,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !features.unstable_macros {
        return;
    }
    for unit in units.iter().filter(|unit| !unit.is_standard()) {
        validate_node(&unit.tree, None, false, diagnostics);
    }
}

fn validate_node(
    node: &SyntaxNode,
    return_role: Option<QuoteRole>,
    in_compile_time: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.kind == SyntaxKind::ClosureExpression {
        let closure_return = node
            .direct_children(SyntaxKind::Type)
            .into_iter()
            .next_back()
            .and_then(role_from_type);
        for child in node.direct_nodes() {
            if child.kind == SyntaxKind::Block {
                validate_node(child, closure_return, in_compile_time, diagnostics);
            }
        }
        return;
    }

    if matches!(
        node.kind,
        SyntaxKind::MacroDeclaration
            | SyntaxKind::AttributeDeclaration
            | SyntaxKind::DeriveDeclaration
    ) {
        let declared_role = node
            .direct_nodes()
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::Type)
            .next_back()
            .and_then(role_from_type);
        for child in node.direct_nodes() {
            if child.kind == SyntaxKind::Block {
                validate_node(child, declared_role, true, diagnostics);
            }
        }
        return;
    }

    if node.kind == SyntaxKind::LetStatement {
        let annotation = node.direct_child(SyntaxKind::Type);
        if let Some(quote) = node.direct_child(SyntaxKind::QuoteExpression) {
            if !in_compile_time {
                diagnostics.push(compile_time_only(quote));
                return;
            }
            match annotation.and_then(role_from_type) {
                Some(role) => diagnostics.extend(validate_quote(quote, role).diagnostics),
                None if annotation.is_some() => diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "a quote binding annotation must name one `std.ast` quote role",
                    )
                    .with_primary(annotation.expect("checked above").span),
                ),
                None => diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "this quote has no expected `std.ast` role; add a binding annotation",
                    )
                    .with_primary(quote.span),
                ),
            }
            validate_interpolation_expressions(quote, return_role, in_compile_time, diagnostics);
            return;
        }
    }

    if node.kind == SyntaxKind::ReturnStatement
        && let Some(quote) = node.direct_child(SyntaxKind::QuoteExpression)
    {
        if !in_compile_time {
            diagnostics.push(compile_time_only(quote));
            return;
        }
        match return_role {
            Some(role) => diagnostics.extend(validate_quote(quote, role).diagnostics),
            None => diagnostics.push(
                Diagnostic::new(
                    Category::CompileTime,
                    "the enclosing compile-time declaration return type is not a quote role",
                )
                .with_primary(quote.span),
            ),
        }
        validate_interpolation_expressions(quote, return_role, in_compile_time, diagnostics);
        return;
    }

    for child in node.direct_nodes() {
        if child.kind == SyntaxKind::QuoteExpression {
            if !in_compile_time {
                diagnostics.push(compile_time_only(child));
                continue;
            }
            // Parameter-driven expected roles require compile-time signature
            // checking. Preserve the template without guessing here.
            validate_interpolation_expressions(child, return_role, in_compile_time, diagnostics);
        } else {
            validate_node(child, return_role, in_compile_time, diagnostics);
        }
    }
}

fn compile_time_only(quote: &SyntaxNode) -> Diagnostic {
    Diagnostic::new(
        Category::CompileTime,
        "`quote:` is available only inside a compile-time declaration body",
    )
    .with_primary(quote.span)
}

fn validate_interpolation_expressions(
    quote: &SyntaxNode,
    return_role: Option<QuoteRole>,
    in_compile_time: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(body) = quote.direct_child(SyntaxKind::QuoteBody) else {
        return;
    };
    for interpolation in body.direct_children(SyntaxKind::QuoteInterpolation) {
        for expression in interpolation.direct_nodes() {
            validate_node(expression, return_role, in_compile_time, diagnostics);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::lexer::lex;
    use crate::parser::parse;
    use crate::source::SourceManager;

    use super::*;

    fn quote(source: &str) -> (SourceManager, SyntaxNode) {
        let complete = format!(
            "macro example() -> std.ast.Expression:\n    let value: std.ast.Expression = quote:\n{source}"
        );
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("quote.elx"), complete.clone());
        let lexed = lex(file, &complete);
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        let parsed = parse(&lexed.tokens);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        fn find(node: &SyntaxNode) -> Option<&SyntaxNode> {
            if node.kind == SyntaxKind::QuoteExpression {
                return Some(node);
            }
            node.direct_nodes().into_iter().find_map(find)
        }
        let found = find(&parsed.tree).expect("quote exists").clone();
        (sources, found)
    }

    #[test]
    fn recognizes_every_exact_public_quote_role() {
        for (name, expected) in [
            ("Expression", QuoteRole::Expression),
            ("Pattern", QuoteRole::Pattern),
            ("TypeSyntax", QuoteRole::TypeSyntax),
            ("StatementList", QuoteRole::StatementList),
            ("MemberList", QuoteRole::MemberList),
            ("Item", QuoteRole::Item),
            ("ItemList", QuoteRole::ItemList),
            ("StructDefinition", QuoteRole::StructDefinition),
            ("EnumDefinition", QuoteRole::EnumDefinition),
            ("FunctionDefinition", QuoteRole::FunctionDefinition),
            ("Implementation", QuoteRole::Implementation),
            ("FieldDefinition", QuoteRole::FieldDefinition),
        ] {
            let mut sources = SourceManager::new();
            let source = format!("std.ast.{name}");
            let file = sources.add_text(PathBuf::from("type.elx"), source.clone());
            let tokens = lex(file, &source).tokens;
            let parsed = crate::parser::parse_fragment(&tokens, crate::parser::FragmentKind::Type);
            assert_eq!(role_from_type(&parsed.tree), Some(expected), "{name}");
        }

        let mut sources = SourceManager::new();
        let source = "other.ast.Expression";
        let file = sources.add_text(PathBuf::from("type.elx"), source.to_string());
        let tokens = lex(file, source).tokens;
        let parsed = crate::parser::parse_fragment(&tokens, crate::parser::FragmentKind::Type);
        assert_eq!(role_from_type(&parsed.tree), None);
    }

    #[test]
    fn validates_scalar_roles_through_the_ordinary_parser() {
        let (_, expression) = quote("        ($left, 2)\n");
        assert!(
            validate_quote(&expression, QuoteRole::Expression)
                .diagnostics
                .is_empty()
        );
        assert!(
            validate_quote(&expression, QuoteRole::Pattern)
                .diagnostics
                .is_empty()
        );

        let (_, type_quote) = quote("        Result[$payload, Error]\n");
        assert!(
            validate_quote(&type_quote, QuoteRole::TypeSyntax)
                .diagnostics
                .is_empty()
        );
    }

    #[test]
    fn validates_statement_member_and_item_collections() {
        let (_, statements) = quote("        let value = $expression\n        return value\n");
        assert!(
            validate_quote(&statements, QuoteRole::StatementList)
                .diagnostics
                .is_empty()
        );

        let (_, members) = quote(
            "        id: u64\n\n        pub fn id(self: &Self) -> u64:\n            return self.id\n",
        );
        assert!(
            validate_quote(&members, QuoteRole::MemberList)
                .diagnostics
                .is_empty()
        );

        let (_, items) = quote(
            "        struct One:\n            pass\n\n        struct Two:\n            pass\n",
        );
        assert!(
            validate_quote(&items, QuoteRole::ItemList)
                .diagnostics
                .is_empty()
        );
        assert!(
            !validate_quote(&items, QuoteRole::Item)
                .diagnostics
                .is_empty()
        );
    }

    #[test]
    fn specific_definition_roles_reject_other_item_kinds() {
        let (_, structure) = quote("        struct Record:\n            pass\n");
        assert!(
            validate_quote(&structure, QuoteRole::StructDefinition)
                .diagnostics
                .is_empty()
        );
        assert!(
            !validate_quote(&structure, QuoteRole::Implementation)
                .diagnostics
                .is_empty()
        );

        let (_, implementation) = quote("        impl Display for Record:\n            pass\n");
        assert!(
            validate_quote(&implementation, QuoteRole::Implementation)
                .diagnostics
                .is_empty()
        );

        let (_, field) = quote("        value: u64\n");
        assert!(
            validate_quote(&field, QuoteRole::FieldDefinition)
                .diagnostics
                .is_empty()
        );
    }

    #[test]
    fn collection_position_interpolation_can_supply_a_complete_list() {
        let (_, quote) = quote("        $members\n");
        assert!(
            validate_quote(&quote, QuoteRole::MemberList)
                .diagnostics
                .is_empty()
        );
        assert!(
            validate_quote(&quote, QuoteRole::ItemList)
                .diagnostics
                .is_empty()
        );
        assert!(
            validate_quote(&quote, QuoteRole::StatementList)
                .diagnostics
                .is_empty()
        );
        assert_eq!(
            interpolations(&quote, QuoteRole::MemberList),
            [QuoteInterpolation {
                form: InterpolationForm::Named("members".to_string()),
                position: InterpolationPosition::Collection,
                span: quote
                    .direct_child(SyntaxKind::QuoteBody)
                    .unwrap()
                    .direct_child(SyntaxKind::QuoteInterpolation)
                    .unwrap()
                    .span,
            }]
        );
    }

    #[test]
    fn computed_and_embedded_interpolations_are_scalar() {
        let (_, quote) = quote("        call($value, $(transform(value)))\n");
        let sites = interpolations(&quote, QuoteRole::Expression);
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].form, InterpolationForm::Named("value".to_string()));
        assert_eq!(sites[1].form, InterpolationForm::Computed);
        assert!(
            sites
                .iter()
                .all(|site| site.position == InterpolationPosition::Scalar)
        );
    }

    #[test]
    fn wrong_roles_report_the_physical_quoted_syntax() {
        let (_, quote) = quote("        value + 1\n");
        let output = validate_quote(&quote, QuoteRole::Item);
        assert!(!output.diagnostics.is_empty());
        assert!(
            output
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.primary.is_some())
        );
    }
}
