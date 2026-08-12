use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use elamite::lexer::lex;
use elamite::parser::{SyntaxKind, parse};
use elamite::source::SourceManager;

fn parses(name: &str, source: &str) -> elamite::parser::ParseOutput {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from(name), source.to_string());
    let lexed = lex(file, source);
    assert!(lexed.diagnostics.is_empty(), "{:#?}", lexed.diagnostics);
    let parsed = parse(&lexed.tokens);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    parsed
}

static NEXT_PACKAGE: AtomicUsize = AtomicUsize::new(0);

struct TestPackage {
    root: PathBuf,
}

impl TestPackage {
    fn new(source: &str) -> Self {
        let serial = NEXT_PACKAGE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-standard-ordering-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src")).expect("create test package");
        std::fs::write(
            root.join("elamite.toml"),
            "[package]\nname = \"standard_ordering_test\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n",
        )
        .expect("write manifest");
        std::fs::write(root.join("src/main.elx"), source).expect("write source");
        Self { root }
    }

    fn command(&self, action: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_elamc"))
            .arg(action)
            .arg(&self.root)
            .output()
            .unwrap_or_else(|error| panic!("run elamc {action}: {error}"))
    }
}

impl Drop for TestPackage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn text_surface_states_borrowing_materialization_and_unicode_rules() {
    let source = include_str!("../stdlib/src/text.elx");
    let parsed = parses("text.elx", source);

    assert_eq!(parsed.tree.count(SyntaxKind::Enum), 1);
    assert_eq!(parsed.tree.count(SyntaxKind::Function), 23);
    assert_eq!(parsed.tree.count(SyntaxKind::Attribute), 5);
    assert!(source.contains("fn _next_scalar"));
    assert!(source.contains("pub fn find(text: str, needle: str) -> Option[usize]:"));
    assert!(!source.contains("pub fn find(text: str, needle: str) -> Option[usize]:\n    pass"));
    assert!(source.contains("element is a borrowed view"));
    assert!(source.contains("every returned substring allocate"));
    assert!(source.contains("Unicode-scalar index"));
    assert!(source.contains("no normalization"));
    assert!(source.contains("Unicode White_Space"));
}

#[test]
fn text_algorithms_run_with_utf8_views_materialization_and_parse_results() {
    let package = TestPackage::new(
        r#"use std.text
use std.text.ParseError

fn show_index(value: Option[usize]) -> ():
    match value:
        Option.Some(index):
            println(index)
        Option.None:
            println("none")

fn show_i64(value: Result[i64, ParseError]) -> ():
    match value:
        Result.Ok(number):
            println(number)
        Result.Err(error):
            match error:
                ParseError.Empty:
                    println("empty")
                ParseError.InvalidSyntax:
                    println("syntax")
                ParseError.OutOfRange:
                    println("range")

fn main() -> ():
    show_index(text.find("aλ雪z", "雪"))
    println(text.contains("aλ雪z", "λ雪"))
    let pieces = text.split("aλ雪", "")
    println(f"{pieces.len()}:{pieces[0]}:{pieces[1]}:{pieces[2]}")
    let separated = text.split("a--b--", "--")
    println(f"{separated.len()}:{separated[0]}:{separated[1]}:{separated[2]}")
    let owned = text.split_string(String.from("x:y"), ":")
    let owned_first = owned[0].clone()
    let owned_second = owned[1].clone()
    println(f"{owned.len()}:{&owned_first}:{&owned_second}")
    println(f"[{text.trim("\u{2003} hi \u{00a0}")}]")
    println(f"[{text.trim_string(String.from("\u{2003}owned\u{2003}"))}]")
    println(text.to_uppercase("straße"))
    println(text.to_lowercase("ASCII"))
    show_i64(text.parse_i64("-42"))
    show_i64(text.parse_i64(""))
    show_i64(text.parse_i64("4x"))
    show_i64(text.parse_i64("9223372036854775808"))
    match text.parse_u64("18446744073709551615"):
        Result.Ok(number):
            println(number)
        Result.Err(_):
            println("unexpected")
    match text.parse_bool("true"):
        Result.Ok(value):
            println(value)
        Result.Err(_):
            println("unexpected")
"#,
    );
    let run = package.command("run");
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(
        String::from_utf8(run.stdout).expect("UTF-8 output"),
        "2\ntrue\n3:a:λ:雪\n3:a:b:\n2:x:y\n[hi]\n[owned]\nSTRASSE\nascii\n-42\nempty\nsyntax\nrange\n18446744073709551615\ntrue\n"
    );

    let built = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("build")
        .arg(&package.root)
        .arg("--keep-c")
        .output()
        .expect("build source-hosted text package");
    assert!(built.status.success(), "{}", stderr(&built));
    let generated = std::fs::read_to_string(package.root.join("build/standard_ordering_test.c"))
        .expect("read retained generated C");
    for removed in [
        "el_text_find_t",
        "el_text_split_t",
        "el_text_trim_t",
        "el_text_parse_i64_t",
        "el_text_lowercase_t",
    ] {
        assert!(
            !generated.contains(removed),
            "algorithm-level native helper `{removed}` survived source hosting"
        );
    }
    assert!(generated.contains("el_text_next_scalar_t"));
}
