use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::docs::extract;
use elamite::package::PackageGraph;
use elamite::resolution::resolve;
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn project_markdown_is_indexed_under_docs() {
    let mut root_markdown = fs::read_dir(".")
        .expect("read repository root")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect::<Vec<_>>();
    root_markdown.sort();
    assert_eq!(
        root_markdown,
        [PathBuf::from("./AGENTS.md"), PathBuf::from("./README.md")]
    );

    let agents = fs::read_to_string("AGENTS.md").expect("read contributor instructions");
    let index = fs::read_to_string("README.md").expect("read documentation index");
    let mut directories = vec![PathBuf::from("docs")];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("read documentation directory") {
            let path = entry.expect("read documentation entry").path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("documentation filenames are UTF-8");
                assert_eq!(
                    name,
                    name.to_lowercase(),
                    "{} is not lowercase",
                    path.display()
                );
            }
        }
    }
    for name in [
        "spec.md",
        "roadmap.md",
        "ledger.md",
        "issues.md",
        "proposals.md",
        "critiques.md",
        "cost_model.md",
    ] {
        let path = format!("docs/{name}");
        assert!(fs::metadata(&path).is_ok(), "missing {path}");
        assert!(agents.contains(&path), "AGENTS.md does not mention {path}");
        assert!(
            index.contains(&format!("](docs/{name})")),
            "index omits {name}"
        );
    }
}

struct TestPackage {
    root: PathBuf,
}

impl TestPackage {
    fn new(source: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("elamite-docs-{}-{serial}", std::process::id()));
        fs::create_dir_all(root.join("src")).expect("create source directory");
        fs::write(
            root.join("elamite.toml"),
            "[package]\nname = \"docs_test\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n",
        )
        .expect("write manifest");
        fs::write(root.join("src/lib.elx"), source).expect("write source");
        Self { root }
    }
}

impl Drop for TestPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn extracts_public_docs_and_source_links_despite_a_private_body_error() {
    let package = TestPackage::new(
        "/// Greets one caller.\n\
         pub fn greet(name: str) -> str:\n\
         \x20\x20\x20\x20return name\n\
         \n\
         /// This declaration is private.\n\
         fn broken() -> ():\n\
         \x20\x20\x20\x20missing_name\n",
    );
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&package.root.join("elamite.toml"), &mut sources)
        .expect("resolve graph");
    let resolution = resolve(&graph, &mut sources);
    assert!(
        !resolution.diagnostics.is_empty(),
        "the private body should remain invalid"
    );

    let docs = extract(&resolution.program, &sources, &graph.root);
    assert_eq!(docs.items.len(), 1);
    let item = &docs.items[0];
    assert_eq!(item.path, "root.greet");
    assert_eq!(item.documentation, "Greets one caller.");
    assert_eq!(item.signature, "pub fn greet(name: str) -> str:");
    assert!(item.source_path.ends_with("src/lib.elx"));
    assert_eq!(item.source_line, 2);

    let markdown = docs.markdown();
    assert!(markdown.contains("## `root.greet`"));
    assert!(markdown.contains("Source: `"));
    assert!(!markdown.contains("broken"));
}

#[test]
fn command_line_documentation_emits_markdown() {
    let package = TestPackage::new(
        "/// Returns its argument.\n\
         pub fn identity[T](value: T) -> T:\n\
         \x20\x20\x20\x20return value\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("doc")
        .arg(&package.root)
        .output()
        .expect("run documentation command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## `root.identity`"), "{stdout}");
    assert!(stdout.contains("Returns its argument."), "{stdout}");
    assert!(stdout.contains("src/lib.elx"), "{stdout}");
}
