use std::path::PathBuf;

use elamite::diagnostics::Diagnostic;
use elamite::lexer::{TokenKind, lex};
use elamite::parser::{SyntaxElement, SyntaxKind, SyntaxNode, parse};
use elamite::source::SourceManager;
use proptest::prelude::*;

fn parse_text(source: &str) -> (SourceManager, elamite::parser::ParseOutput) {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from("test.elx"), source.to_string());
    let lexed = lex(file, source);
    assert!(
        lexed.diagnostics.is_empty(),
        "unexpected lexer diagnostics: {:?}",
        lexed.diagnostics
    );
    (sources, parse(&lexed.tokens))
}

fn diagnostics(sources: &SourceManager, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let location = diagnostic.primary.map_or_else(
                || "<no span>".to_string(),
                |span| {
                    let position = sources.line_col(span.file, span.start);
                    format!("{}:{}", position.line, position.column)
                },
            );
            format!("{location}: {}", diagnostic.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(node: &SyntaxNode) -> String {
    fn walk(node: &SyntaxNode, depth: usize, output: &mut String) {
        output.push_str(&"  ".repeat(depth));
        output.push_str(&format!(
            "{:?} @ {}..{}\n",
            node.kind, node.span.start, node.span.end
        ));
        for child in &node.children {
            match child {
                SyntaxElement::Node(child) => walk(child, depth + 1, output),
                SyntaxElement::Token(token)
                    if !matches!(
                        token.kind,
                        TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
                    ) =>
                {
                    output.push_str(&"  ".repeat(depth + 1));
                    output.push_str(&format!("{:?}\n", token.kind));
                }
                SyntaxElement::Token(_) => {}
            }
        }
    }

    let mut output = String::new();
    walk(node, 0, &mut output);
    output
}

#[test]
fn parses_the_authoritative_demonstration() {
    let source = include_str!("../examples/spec_demo.elx");
    let (sources, output) = parse_text(source);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &output.diagnostics)
    );
    assert_eq!(output.tree.kind, SyntaxKind::File);
    assert!(output.tree.count(SyntaxKind::Function) >= 20);
    assert!(output.tree.count(SyntaxKind::FormattedStringExpression) >= 20);
}

#[test]
fn snapshots_declarations_and_type_forms() {
    let source = r#"/// Public facade
pub mod facade:
    pub use root.inner.Value as Value

pub type Callback[T: Display + Hash] = &unsafe fn(&T, ...String) -> Result[(), Error]

pub struct Pair[T](Default, PartialEq):
    pub left: T
    right: [u8; 16]

pub enum Message:
    Quit
    Move(i32, i32)
    Write { text: String }

trait Show[T]:
    fn show(self: &Self, value: &T) -> str

impl[T: Show] Show[T] for Pair[T]:
    fn show(self: &Self, value: &T) -> str:
        return "pair"

@importc("native_handle_t", "native.h")
type Handle

@importc("point_t", "native.h")
struct CPoint:
    x: f64
    y: f64

@importc("open", "native.h")
fn open(callback: *fn(*u8) -> i32) -> *var Handle
"#;
    let (sources, output) = parse_text(source);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &output.diagnostics)
    );
    insta::assert_snapshot!(render(&output.tree));
}

#[test]
fn snapshots_statements_expressions_and_patterns() {
    let source = r#"fn exercise(value: Option[Point], items: [i32]):
    let tuple = (1, 2)
    var values = @vec[1, 2, 3]
    values[0] += -1 + 2 * 3
    defer values.clear()
    defer:
        let pending = 1
        values.clear()
    if value != null && values[0] > 0:
        pass
    else:
        unsafe:
            values[0] = *(&values[0])
    match value:
        Option.Some(Point { x, y: 0, .. }) if x > 0:
            return Result.Ok(x)?
        Option.None | null:
            continue
        _:
            break
    for item in items:
        values.append(item as i64)
    while false:
        pass
    let array = [1, 2, 3]
    let map = @map{"one": 1}
    let set = @set{"one"}
    println(f"value {tuple}")
"#;
    let (sources, output) = parse_text(source);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &output.diagnostics)
    );
    insta::assert_snapshot!(render(&output.tree));
}

#[test]
fn parses_each_initial_type_form() {
    let cases = [
        "i32",
        "root.types.Value",
        "Map[str, Vec[u8]]",
        "(i32, String)",
        "[i32; 4]",
        "[i32]",
        "&i32",
        "&var i32",
        "*i32",
        "*var i32",
        "&fn(i32) -> bool",
        "&unsafe fn(*i32) -> &i32",
        "*fn(*u8) -> i32",
        "*unsafe fn(*u8) -> i32",
        "&Display",
    ];

    for ty in cases {
        let source = format!("type Subject = {ty}\n");
        let (sources, output) = parse_text(&source);
        assert!(
            output.diagnostics.is_empty(),
            "type `{ty}`:\n{}",
            diagnostics(&sources, &output.diagnostics)
        );
        assert!(output.tree.count(SyntaxKind::Type) >= 1, "type `{ty}`");
    }
}

#[test]
fn parses_each_initial_expression_form() {
    let cases = [
        "name",
        "42",
        "\"text\"",
        "'x'",
        "f\"value: {name}\"",
        "(name)",
        "(name,)",
        "[1, 2]",
        "Point{x: 1, y}",
        "Option.Some(1)",
        "value.field",
        "values[0]",
        "result?",
        "!ready",
        "&var value",
        "value as i64",
        "1 + 2 * 3",
        "@vec[1, 2]",
        "@map{\"one\": 1}",
        "@set{\"one\"}",
    ];

    for expression in cases {
        let source = format!("fn subject():\n    let value = {expression}\n");
        let (sources, output) = parse_text(&source);
        assert!(
            output.diagnostics.is_empty(),
            "expression `{expression}`:\n{}",
            diagnostics(&sources, &output.diagnostics)
        );
        assert_eq!(
            output.tree.count(SyntaxKind::LetStatement),
            1,
            "expression `{expression}`"
        );
    }
}

#[test]
fn parses_each_initial_pattern_form() {
    let cases = [
        "_",
        "binding",
        "*binding",
        "42",
        "\"text\"",
        "(left, right)",
        "Point { x, y: 0, .. }",
        "Option.None",
        "Option.Some(value)",
        "Shape.Point { x, y }",
        "Option.None | null",
        "value if value > 0",
    ];

    for pattern in cases {
        let source = format!(
            "fn subject(value: Value):\n    match value:\n        {pattern}:\n            pass\n"
        );
        let (sources, output) = parse_text(&source);
        assert!(
            output.diagnostics.is_empty(),
            "pattern `{pattern}`:\n{}",
            diagnostics(&sources, &output.diagnostics)
        );
        assert_eq!(
            output.tree.count(SyntaxKind::MatchArm),
            1,
            "pattern `{pattern}`"
        );
        if pattern == "*binding" {
            assert_eq!(output.tree.count(SyntaxKind::DereferencePattern), 1);
        }
    }
}

#[test]
fn parses_assignment_operators_as_statements() {
    for operator in [
        "=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=",
    ] {
        let source = format!("fn subject():\n    value {operator} 1\n");
        let (sources, output) = parse_text(&source);
        assert!(
            output.diagnostics.is_empty(),
            "operator `{operator}`:\n{}",
            diagnostics(&sources, &output.diagnostics)
        );
        assert_eq!(output.tree.count(SyntaxKind::AssignmentStatement), 1);
    }
}

#[test]
fn reports_required_invalid_surface_forms_at_source_spans() {
    let cases = [
        ("same-line body", "fn bad(): pass\n", "following line"),
        (
            "brace body",
            "fn bad() {\n    pass\n}\n",
            "expected end of statement",
        ),
        (
            "empty body",
            "fn bad():\nfn recovered():\n    pass\n",
            "expected a four-space indented body",
        ),
        (
            "chained relational comparison",
            "fn bad():\n    let value = a < b < c\n",
            "chained comparisons",
        ),
        (
            "mixed chained comparison",
            "fn bad():\n    let value = a == b < c\n",
            "chained comparisons",
        ),
        (
            "malformed generic list",
            "type Broken[T U] = T\n",
            "expected `]`",
        ),
        (
            "malformed pattern",
            "fn bad(value: Value):\n    match value:\n        Point { x: }:\n            pass\n",
            "expected a pattern",
        ),
        (
            "non-call defer",
            "fn bad():\n    defer resource\n",
            "`defer` requires a single function or method call, or a `defer:` block",
        ),
        (
            "empty derive list",
            "struct Bad():\n    value: i32\n",
            "derive list cannot be empty",
        ),
        (
            "unknown compiler macro",
            "fn bad():\n    let value = @custom[1]\n",
            "unknown compiler macro",
        ),
        (
            "map entry without value separator",
            "fn bad():\n    let value = @map{\"key\", 1}\n",
            "expected `:` between map key and value",
        ),
        (
            "bodyless ordinary function",
            "fn bad()\n",
            "function definition requires an indented body",
        ),
        (
            "foreign function body",
            "@importc(\"bad\", \"bad.h\")\nfn bad():\n    pass\n",
            "foreign function declaration cannot have a body",
        ),
        (
            "anonymous function expression",
            "fn bad():\n    let callback = fn(value: i32) -> i32\n",
            "expected an expression",
        ),
        (
            "reserved parameter name",
            "fn bad(root: i32):\n    pass\n",
            "expected parameter name",
        ),
    ];

    for (name, source, expected) in cases {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("invalid.elx"), source.to_string());
        let lexed = lex(file, source);
        let output = parse(&lexed.tokens);
        let rendered = diagnostics(&sources, &output.diagnostics);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{name} did not report `{expected}`:\n{rendered}\nlexer: {:?}",
            lexed.diagnostics
        );
        assert!(
            output
                .diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic.primary)
                .any(|span| span.start <= span.end && span.end <= source.len() as u32),
            "{name} did not carry a useful source span"
        );
    }
}

#[test]
fn parses_module_level_function_modifiers() {
    let source = r#"pub fn safe_export():
    pass

unsafe pub fn checked_by_caller():
    pass

@exportc("exported_callback")
fn exported_callback(value: i32) -> i32:
    return value

@exportc("unsafe_export")
unsafe fn unsafe_export(value: *i32) -> i32:
    unsafe:
        return *value
"#;
    let (sources, output) = parse_text(source);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &output.diagnostics)
    );
    assert_eq!(output.tree.count(SyntaxKind::Function), 4);
}

#[test]
fn pass_can_make_declaration_bodies_explicitly_nonempty() {
    let source = r#"mod empty_module:
    pass
struct Empty:
    pass
enum Never:
    pass
trait Marker:
    pass
impl Marker for Empty:
    pass
"#;
    let (sources, output) = parse_text(source);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &output.diagnostics)
    );
    assert_eq!(output.tree.count(SyntaxKind::PassStatement), 5);
}

#[test]
fn syntax_tree_dump_is_deterministic_and_token_preserving() {
    let (_, first) = parse_text("fn answer() -> i32:\n    return 42\n");
    let (_, second) = parse_text("fn answer() -> i32:\n    return 42\n");
    assert_eq!(first.tree.dump(), second.tree.dump());
    assert!(first.tree.dump().contains("Function"));
    assert!(first.tree.dump().contains("Keyword(Return)"));
    assert!(first.tree.dump().contains("IntegerLiteral"));
}

#[test]
fn respects_every_expression_precedence_level() {
    let cases = [
        ("value.field()", "CallExpression"),
        ("-value.field", "UnaryExpression"),
        ("-value as i32", "CastExpression"),
        ("left as i32 * right", "Star"),
        ("left * middle + right", "Plus"),
        ("left + middle << right", "Shl"),
        ("left << middle & right", "Amp"),
        ("left & middle ^ right", "Caret"),
        ("left ^ middle | right", "Pipe"),
        ("left | middle < right", "Less"),
        ("left < right", "Less"),
        ("left == right", "EqEq"),
        ("left == middle && right", "AndAnd"),
        ("left && middle || right", "OrOr"),
    ];

    for (expression, expected_root) in cases {
        let source = format!("fn subject():\n    let result = {expression}\n");
        let (sources, output) = parse_text(&source);
        assert!(
            output.diagnostics.is_empty(),
            "expression `{expression}`:\n{}",
            diagnostics(&sources, &output.diagnostics)
        );
        let binding = find_node(&output.tree, SyntaxKind::LetStatement)
            .expect("test source contains one binding");
        let value = binding
            .children
            .iter()
            .rev()
            .find_map(|child| match child {
                SyntaxElement::Node(node) => Some(node.as_ref()),
                SyntaxElement::Token(_) => None,
            })
            .expect("binding contains an expression");
        let root_matches = format!("{:?}", value.kind) == expected_root
            || value.children.iter().any(|child| {
                matches!(child, SyntaxElement::Token(token) if format!("{:?}", token.kind) == expected_root)
            });
        assert!(
            root_matches,
            "`{expression}` expected root `{expected_root}`, got:\n{}",
            value.dump()
        );
    }
}

fn find_node(node: &SyntaxNode, kind: SyntaxKind) -> Option<&SyntaxNode> {
    if node.kind == kind {
        return Some(node);
    }
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Node(child) => find_node(child, kind),
        SyntaxElement::Token(_) => None,
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn parser_recovery_never_panics(
        characters in proptest::collection::vec(any::<char>(), 0..192),
    ) {
        let source: String = characters.into_iter().collect();
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("generated.elx"), source.clone());
        let lexed = lex(file, &source);
        let parsed = parse(&lexed.tokens);
        prop_assert_eq!(parsed.tree.kind, SyntaxKind::File);
    }
}
