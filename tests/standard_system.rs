use std::path::PathBuf;

use elamite::lexer::lex;
use elamite::parser::{SyntaxKind, parse};
use elamite::source::SourceManager;

fn enum_variants(source: &str, name: &str) -> Vec<String> {
    let declaration = format!("pub enum {name}:");
    let mut lines = source
        .lines()
        .skip_while(|line| line.trim() != declaration)
        .skip(1);
    let mut variants = Vec::new();
    for line in &mut lines {
        if !line.starts_with("    ") || line.trim().is_empty() {
            break;
        }
        variants.push(line.trim().to_string());
    }
    assert!(!variants.is_empty(), "missing or empty enum `{name}`");
    variants
}

fn parse_standard_source(name: &str, source: &str) -> elamite::parser::ParseOutput {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from(name), source.to_string());
    let lexed = lex(file, source);
    assert!(
        lexed.diagnostics.is_empty(),
        "{name}: {:?}",
        lexed.diagnostics
    );
    let output = parse(&lexed.tokens);
    assert!(
        output.diagnostics.is_empty(),
        "{name}: {:?}",
        output.diagnostics
    );
    output
}

#[test]
fn filesystem_surface_has_owned_results_and_explicit_cleanup() {
    let source = include_str!("../stdlib/src/fs.elx");
    let output = parse_standard_source("fs.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Struct), 3);
    assert_eq!(output.tree.count(SyntaxKind::Enum), 2);
    assert_eq!(output.tree.count(SyntaxKind::Impl), 1);
    for signature in [
        "pub fn open(path: &Path, mode: OpenMode) -> Result[File, io.IoError]",
        "pub fn read_dir(path: &Path) -> Result[Directory, io.IoError]",
        "`File.read_to_end`, `write_all`, `metadata`, and idempotent `close`",
        "`Directory.next` and idempotent `close`",
    ] {
        assert!(source.contains(signature), "missing `{signature}`");
    }
    assert!(
        source.contains(
            "cleanup surface is\n// exactly `pub fn close(self: &Self) -> ()` on each type"
        )
    );
    assert!(source.contains(
        "Closing either handle\n// repeatedly succeeds without another effect and never reports an I/O error"
    ));
}

#[test]
fn io_errors_are_portable_and_exhaustive_at_the_api_boundary() {
    let source = include_str!("../stdlib/src/io.elx");
    let output = parse_standard_source("io.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Enum), 1);
    assert_eq!(
        enum_variants(source, "IoError"),
        [
            "NotFound",
            "PermissionDenied",
            "AlreadyExists",
            "InvalidInput",
            "IsDirectory",
            "NotDirectory",
            "DirectoryNotEmpty",
            "ReadOnly",
            "BrokenPipe",
            "Interrupted",
            "WouldBlock",
            "TimedOut",
            "StorageFull",
            "ResourceExhausted",
            "Unsupported",
            "Other",
        ]
    );
    assert!(!source.contains("errno"));
}

#[test]
fn environment_surface_distinguishes_absence_from_failure() {
    let source = include_str!("../stdlib/src/env.elx");
    let output = parse_standard_source("env.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Enum), 1);
    assert!(source.contains("pub fn args() -> Vec[String]"));
    assert!(source.contains("pub fn get(name: str) -> Result[Option[String], EnvError]"));
    assert!(source.contains("pub fn current_dir() -> Result[fs.Path, io.IoError]"));
    assert!(source.contains("InvalidName"));
    assert!(source.contains("InvalidText"));
}

#[test]
fn process_surface_keeps_child_failure_separate_from_launch_failure() {
    let source = include_str!("../stdlib/src/process.elx");
    let output = parse_standard_source("process.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Struct), 2);
    assert_eq!(output.tree.count(SyntaxKind::Enum), 1);
    assert!(source.contains(
        "pub fn run(program: &fs.Path, arguments: [str]) -> Result[Output, ProcessError]"
    ));
    assert!(source.contains("A nonzero child exit is a successful `Output` value."));
    assert!(source.contains("pub stdout: Vec[u8]"));
    assert!(source.contains("pub stderr: Vec[u8]"));
    assert!(source.contains("pub fn exit(code: i32) -> !"));
}
