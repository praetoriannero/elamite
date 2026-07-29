use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::Target;
use elamite::diagnostics::Category;
use elamite::driver::compile;
use elamite::lexer::lex;
use elamite::package::PackageGraph;
use elamite::parser::{SyntaxKind, parse};
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn retained_lexer_parser_corpus_recovers_without_panics_or_cascades() {
    let root = Path::new("tests/fixtures/robustness/parser");
    let mut paths = fs::read_dir(root)
        .expect("read parser corpus")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    assert!(!paths.is_empty());

    for path in paths {
        let source = fs::read_to_string(&path).expect("read corpus input");
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut sources = SourceManager::new();
            let file = sources.add_text(path.clone(), source.clone());
            let lexed = lex(file, &source);
            let parsed = parse(&lexed.tokens);
            assert_eq!(parsed.tree.kind, SyntaxKind::File);
            let diagnostic_count = lexed.diagnostics.len() + parsed.diagnostics.len();
            assert!(
                diagnostic_count <= source.chars().count().saturating_add(16),
                "{} produced an unbounded-looking diagnostic cascade ({diagnostic_count})",
                path.display()
            );
        }));
        assert!(
            outcome.is_ok(),
            "parser corpus panicked on {}",
            path.display()
        );
    }
}

#[test]
fn malformed_semantic_inputs_stop_at_diagnostics_without_internal_leaks() {
    let cases = [
        ("syntax", Category::Syntax, "fn main( -> ():\n    let =\n"),
        (
            "module",
            Category::NameResolution,
            "fn main() -> ():\n    println(missing_name)\n",
        ),
        (
            "type",
            Category::ExpressionType,
            "fn main() -> ():\n    let value: i32 = true\n    println(value)\n",
        ),
        (
            "trait",
            Category::TypeSystem,
            "trait Read:\n    fn read(self: &Self) -> i32\n\nstruct Item:\n    value: i32\n\nimpl Read for Item:\n    fn read(self: &Self) -> bool:\n        return true\n\nfn main() -> ():\n    pass\n",
        ),
        (
            "generic",
            Category::Call,
            "fn identity[T](value: T) -> T:\n    return value\n\nfn main() -> ():\n    println(identity())\n",
        ),
        (
            "collection",
            Category::Place,
            "fn main() -> ():\n    let values = Vec[i32].new()\n    values.append(1)\n",
        ),
        (
            "unsafe",
            Category::UnsafeContext,
            "unsafe fn danger() -> ():\n    pass\n\nfn main() -> ():\n    danger()\n",
        ),
        (
            "ffi",
            Category::TypeSystem,
            "@importc(\"invalid\", \"invalid.h\")\nfn invalid(value: String) -> ()\n\nfn main() -> ():\n    pass\n",
        ),
    ];

    for (name, expected_category, source) in cases {
        let root = temporary_package(name, source);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut sources = SourceManager::new();
            let graph = PackageGraph::resolve(&root.join("elamite.toml"), &mut sources)
                .expect("test manifest resolves");
            let diagnostics = match compile(&graph, &mut sources, Target::X86_64) {
                Ok(_) => panic!("input is invalid"),
                Err(diagnostics) => diagnostics,
            };
            assert!(!diagnostics.is_empty(), "{name} produced no diagnostic");
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.category == expected_category),
                "{name} did not retain its reviewed {expected_category:?} category: {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.category)
                    .collect::<Vec<_>>()
            );
            for diagnostic in diagnostics {
                assert!(
                    diagnostic.primary.is_some(),
                    "{name} diagnostic has no source span: {}",
                    diagnostic.message
                );
                let text = format!("{:?}: {}", diagnostic.category, diagnostic.message);
                assert!(
                    ![
                        "DeclarationId(",
                        "TypeId(",
                        "LocalBindingId(",
                        "MemberId(",
                        "unexpected internal",
                    ]
                    .iter()
                    .any(|needle| text.contains(needle)),
                    "{name} leaked an internal identity: {text}"
                );
                assert!(
                    !matches!(
                        diagnostic.category,
                        Category::Lowering | Category::CodeGeneration
                    ),
                    "{name} reached an unexplained late failure: {text}"
                );
            }
        }));
        let _ = fs::remove_dir_all(&root);
        assert!(outcome.is_ok(), "semantic corpus panicked on {name}");
    }
}

fn temporary_package(name: &str, source: &str) -> PathBuf {
    let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "elamite-robustness-{}-{name}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create temporary package");
    fs::write(
        root.join("elamite.toml"),
        format!(
            "[package]\nname = \"robustness_{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n"
        ),
    )
    .expect("write manifest");
    fs::write(root.join("src/main.elx"), source).expect("write source");
    root
}
