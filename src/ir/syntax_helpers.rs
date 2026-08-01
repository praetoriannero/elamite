//! Syntax-to-IR conversions shared by typed and control-flow lowering.

use super::*;

pub(super) fn lower_constant(node: &SyntaxNode, negative: bool) -> Option<Constant> {
    let token = first_token(node)?;
    Some(match &token.kind {
        TokenKind::IntegerLiteral { raw, radix, suffix } => Constant::Integer {
            magnitude: parse_integer_magnitude(raw, *radix, *suffix).ok()?,
            negative,
        },
        TokenKind::FloatLiteral { raw, .. } => Constant::Float(if negative {
            format!("-{raw}")
        } else {
            raw.clone()
        }),
        TokenKind::StringLiteral(value) => Constant::String(value.clone()),
        TokenKind::CharacterLiteral(value) => Constant::Character(value.chars().next()?),
        TokenKind::Keyword(Keyword::True) => Constant::Bool(true),
        TokenKind::Keyword(Keyword::False) => Constant::Bool(false),
        TokenKind::Keyword(Keyword::Null) => Constant::Null,
        _ => return None,
    })
}

pub(super) fn unary_operator(kind: &TokenKind) -> Option<UnaryOperator> {
    Some(match kind {
        TokenKind::Plus => UnaryOperator::Positive,
        TokenKind::Minus => UnaryOperator::Negative,
        TokenKind::Bang => UnaryOperator::LogicalNot,
        TokenKind::Tilde => UnaryOperator::BitwiseNot,
        _ => return None,
    })
}

pub(super) fn binary_operator(kind: &TokenKind) -> Option<BinaryOperator> {
    Some(match kind {
        TokenKind::Plus => BinaryOperator::Add,
        TokenKind::PlusPlus => BinaryOperator::Concatenate,
        TokenKind::Minus => BinaryOperator::Subtract,
        TokenKind::Star => BinaryOperator::Multiply,
        TokenKind::Slash => BinaryOperator::Divide,
        TokenKind::Percent => BinaryOperator::Remainder,
        TokenKind::Amp => BinaryOperator::BitAnd,
        TokenKind::Pipe => BinaryOperator::BitOr,
        TokenKind::Caret => BinaryOperator::BitXor,
        TokenKind::Shl => BinaryOperator::ShiftLeft,
        TokenKind::Shr => BinaryOperator::ShiftRight,
        TokenKind::EqEq => BinaryOperator::Equal,
        TokenKind::NotEq => BinaryOperator::NotEqual,
        TokenKind::Less => BinaryOperator::Less,
        TokenKind::LessEq => BinaryOperator::LessEqual,
        TokenKind::Greater => BinaryOperator::Greater,
        TokenKind::GreaterEq => BinaryOperator::GreaterEqual,
        TokenKind::AndAnd => BinaryOperator::LogicalAnd,
        TokenKind::OrOr => BinaryOperator::LogicalOr,
        _ => return None,
    })
}

pub(super) fn assignment_operator(kind: &TokenKind) -> Option<AssignmentOperator> {
    Some(match kind {
        TokenKind::Assign => AssignmentOperator::Assign,
        TokenKind::PlusAssign => AssignmentOperator::Add,
        TokenKind::MinusAssign => AssignmentOperator::Subtract,
        TokenKind::StarAssign => AssignmentOperator::Multiply,
        TokenKind::SlashAssign => AssignmentOperator::Divide,
        TokenKind::PercentAssign => AssignmentOperator::Remainder,
        TokenKind::AmpAssign => AssignmentOperator::BitAnd,
        TokenKind::PipeAssign => AssignmentOperator::BitOr,
        TokenKind::CaretAssign => AssignmentOperator::BitXor,
        TokenKind::ShlAssign => AssignmentOperator::ShiftLeft,
        TokenKind::ShrAssign => AssignmentOperator::ShiftRight,
        _ => return None,
    })
}

pub(super) fn first_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) => Some(token),
        SyntaxElement::Node(_) => None,
    })
}

pub(super) fn first_identifier(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
            Some(token)
        }
        _ => None,
    })
}

pub(super) fn pattern_path_tokens(node: &SyntaxNode) -> Vec<&Token> {
    node.children
        .iter()
        .take_while(|child| {
            !matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::LParen | TokenKind::LBrace,
                    ..
                })
            )
        })
        .filter_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        })
        .collect()
}

pub(super) fn parameter_name_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token)
            if matches!(
                token.kind,
                TokenKind::Identifier(_) | TokenKind::Keyword(Keyword::SelfValue)
            ) =>
        {
            Some(token)
        }
        _ => None,
    })
}

pub(super) fn let_name_token(node: &SyntaxNode) -> Option<&Token> {
    let mut saw_binding = false;
    node.children.iter().find_map(|child| {
        let SyntaxElement::Token(token) = child else {
            return None;
        };
        match token.kind {
            TokenKind::Keyword(Keyword::Let | Keyword::Var) => {
                saw_binding = true;
                None
            }
            TokenKind::Identifier(_) if saw_binding => Some(token),
            _ => None,
        }
    })
}

pub(super) fn callee_target_span(node: &SyntaxNode) -> Option<Span> {
    match node.kind {
        SyntaxKind::NameExpression => first_token(node).map(|token| token.span),
        SyntaxKind::MemberExpression => node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token.span)
            }
            _ => None,
        }),
        SyntaxKind::BracketExpression => child_nodes(node)
            .into_iter()
            .next()
            .and_then(callee_target_span),
        _ => None,
    }
}

pub(super) fn identifier_text(token: &Token) -> &str {
    match &token.kind {
        TokenKind::Identifier(text) => text,
        _ => "",
    }
}
