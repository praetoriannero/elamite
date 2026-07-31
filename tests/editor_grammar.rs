//! Guards the VS Code TextMate grammar against drift from the compiler.
//!
//! A TextMate grammar cannot call into the lexer, so
//! `editors/vscode/syntaxes/elamite.tmLanguage.json` necessarily duplicates the
//! keyword, numeric-suffix, builtin-name, attribute, and macro lists that
//! `src/` already owns. These tests assert the copies still agree with those
//! sources of truth, so adding a keyword without updating the grammar fails
//! here rather than silently shipping unhighlighted syntax.

use std::collections::BTreeSet;
use std::path::PathBuf;

const GRAMMAR: &str = "editors/vscode/syntaxes/elamite.tmLanguage.json";

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

/// Returns the source text between the first `start` and the following `end`.
fn slice_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let begin = source
        .find(start)
        .unwrap_or_else(|| panic!("marker `{start}` no longer appears; update this test"));
    let rest = &source[begin + start.len()..];
    let finish = rest
        .find(end)
        .unwrap_or_else(|| panic!("marker `{end}` no longer follows `{start}`"));
    &rest[..finish]
}

/// Collects every `"literal"` in `source`.
fn quoted_literals(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = source;
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    found
}

/// Collects every literal appearing as `Some("literal")`, ignoring the
/// diagnostic prose that shares those match blocks.
fn some_literals(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = source;
    while let Some(index) = rest.find("Some(\"") {
        rest = &rest[index + "Some(\"".len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    found
}

/// Returns the alternatives of the `ordinal`th (1-based) parenthesized group in
/// the first `"match"` regex at or after `marker` in the grammar.
fn grammar_group(grammar: &str, marker: &str, ordinal: usize) -> BTreeSet<String> {
    let after = &grammar[grammar
        .find(marker)
        .unwrap_or_else(|| panic!("grammar rule `{marker}` not found"))..];
    let regex = &after[after
        .find("\"match\": \"")
        .unwrap_or_else(|| panic!("grammar rule `{marker}` has no `match`"))..];

    let mut open = None;
    let mut depth = 0usize;
    let mut seen = 0usize;
    for (index, character) in regex.char_indices() {
        match character {
            '(' => {
                depth += 1;
                if depth == 1 {
                    seen += 1;
                    if seen == ordinal {
                        open = Some(index);
                    }
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0
                    && let Some(start) = open
                {
                    let group = &regex[start + 1..index];
                    let group = group.strip_prefix("?:").unwrap_or(group);
                    return group.split('|').map(str::to_string).collect();
                }
            }
            _ => {}
        }
    }
    panic!("grammar rule `{marker}` has no group {ordinal}");
}

/// Returns the alternatives of a rule whose first group holds the whole list.
fn grammar_alternatives(grammar: &str, marker: &str) -> BTreeSet<String> {
    grammar_group(grammar, marker, 1)
}

fn grammar_union(grammar: &str, markers: &[&str]) -> BTreeSet<String> {
    markers
        .iter()
        .flat_map(|marker| grammar_alternatives(grammar, marker))
        .collect()
}

#[test]
fn grammar_highlights_exactly_the_lexer_keywords() {
    let lexer = read("src/lexer.rs");
    let expected = quoted_literals(slice_between(
        &lexer,
        "fn keyword(text: &str) -> Option<Keyword> {",
        "_ => return None",
    ));

    let grammar = read(GRAMMAR);
    let actual = grammar_union(
        &grammar,
        &[
            "\"keyword-control\"",
            "\"keyword-declaration\"",
            "\"keyword-modifier\"",
            "\"keyword-operator-word\"",
            "\"constant-language\"",
            "\"variable-language\"",
        ],
    );

    assert_eq!(
        expected, actual,
        "the VS Code grammar's keyword rules disagree with `keyword` in src/lexer.rs"
    );
}

#[test]
fn grammar_recognizes_exactly_the_lexer_numeric_suffixes() {
    let lexer = read("src/lexer.rs");
    let expected = quoted_literals(slice_between(
        &lexer,
        "fn parse_numeric_suffix(text: &str) -> Option<NumericSuffix> {",
        "_ => return None",
    ));

    let grammar = read(GRAMMAR);
    let actual = grammar_alternatives(&grammar, "\"constant.numeric.integer.elx\"");

    assert_eq!(
        expected, actual,
        "the VS Code grammar's numeric suffixes disagree with `parse_numeric_suffix` in src/lexer.rs"
    );
}

#[test]
fn every_numeric_rule_shares_one_suffix_list() {
    let grammar = read(GRAMMAR);
    let expected = grammar_alternatives(&grammar, "\"constant.numeric.integer.elx\"");

    for marker in [
        "\"constant.numeric.hex.elx\"",
        "\"constant.numeric.octal.elx\"",
        "\"constant.numeric.binary.elx\"",
    ] {
        assert_eq!(
            expected,
            grammar_alternatives(&grammar, marker),
            "{marker} accepts a different suffix list than the integer rule"
        );
    }
}

/// Collects every literal appearing as `path: "literal"`, ignoring the `reason`
/// prose that shares those records.
fn path_literals(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = source;
    while let Some(index) = rest.find("path: \"") {
        rest = &rest[index + "path: \"".len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    found
}

/// Reduces `std.ffi.ForeignRoot` to `ForeignRoot`, matching what appears in
/// source once the name is in scope.
fn leaf_names(paths: BTreeSet<String>) -> BTreeSet<String> {
    paths
        .into_iter()
        .map(|path| {
            path.rsplit_once('.')
                .map_or(path.clone(), |(_, leaf)| leaf.to_string())
        })
        .collect()
}

#[test]
fn grammar_highlights_exactly_the_standard_inventory() {
    let standard = read("src/standard.rs");
    let intrinsics = path_literals(slice_between(
        &standard,
        "pub const INTRINSICS: &[Intrinsic] = &[",
        "\n];",
    ));
    let source_declarations = quoted_literals(slice_between(
        &standard,
        "pub const SOURCE_DECLARATIONS: &[&str] = &[",
        "\n];",
    ));
    let expected = leaf_names(
        intrinsics
            .into_iter()
            .chain(source_declarations)
            .collect::<BTreeSet<_>>(),
    );

    let grammar = read(GRAMMAR);
    let actual = grammar_union(
        &grammar,
        &[
            "\"type-primitive\"",
            "\"type-builtin\"",
            "\"type-builtin-trait\"",
            "\"function-builtin\"",
        ],
    );

    assert_eq!(
        expected, actual,
        "the VS Code grammar's builtin names disagree with the inventory in src/standard.rs"
    );
}

#[test]
fn grammar_accepts_exactly_the_known_foreign_attributes() {
    let resolution = read("src/resolution/collect.rs");
    let expected: BTreeSet<String> = resolution
        .lines()
        .filter(|line| line.contains("ForeignDirection::"))
        .filter_map(|line| {
            let start = line.find("Some(\"")? + "Some(\"".len();
            let end = line[start..].find('"')? + start;
            Some(line[start..end].to_string())
        })
        .collect();
    assert!(!expected.is_empty(), "no FFI attributes found to compare");

    let grammar = read(GRAMMAR);
    let actual = grammar_group(&grammar, "\"attributes\"", 2);

    // The grammar scopes any other `@name(...)` as an unknown attribute, which
    // matches the resolver rejecting it, so this list must stay exact.
    assert_eq!(
        expected, actual,
        "the VS Code grammar's attribute list disagrees with src/resolution/collect.rs"
    );
}

#[test]
fn grammar_accepts_exactly_the_parser_macros() {
    let parser = read("src/parser.rs");
    let expected = some_literals(slice_between(
        &parser,
        "fn parse_macro_expression(",
        "Some(name) =>",
    ));

    let grammar = read(GRAMMAR);
    let actual = grammar_group(&grammar, "\"macros\"", 2);

    assert_eq!(
        expected, actual,
        "the VS Code grammar's macro list disagrees with `parse_macro_expression` in src/parser.rs"
    );
}

#[test]
fn grammar_scopes_variables_the_same_inside_formatted_strings() {
    let grammar = read(GRAMMAR);
    let expression = slice_between(&grammar, "\"expression\": {", "\n    \"comments\":");
    let interpolation = slice_between(
        &grammar,
        "\"interpolation\": {",
        "\n    \"interpolation-nested\":",
    );
    let identifier = slice_between(&grammar, "\"identifier\": {", "\n    \"numbers\":");

    assert!(
        expression.contains(r##""include": "#identifier""##),
        "ordinary expressions must include the shared identifier rule"
    );
    assert!(
        interpolation.contains(r##""include": "#expression""##),
        "formatted-string interpolation must reuse the ordinary expression rules"
    );
    assert!(
        identifier.contains(r#""name": "variable.other.readwrite.elx""#),
        "ordinary identifiers must receive an explicit variable scope"
    );
}
