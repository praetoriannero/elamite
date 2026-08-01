use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::formatter::{FormatOptions, format_source};
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestPackage {
    root: PathBuf,
}

impl TestPackage {
    fn new(name: &str, line_length: Option<usize>, source: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-formatter-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create formatter fixture");
        let format = line_length.map_or_else(String::new, |line_length| {
            format!("\n[format]\nline_length = {line_length}\n")
        });
        fs::write(
            root.join("elamite.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n{format}"
            ),
        )
        .expect("write formatter manifest");
        fs::write(root.join("src/main.elx"), source).expect("write formatter source");
        Self { root }
    }

    fn source_path(&self) -> PathBuf {
        self.root.join("src/main.elx")
    }

    fn source(&self) -> String {
        fs::read_to_string(self.source_path()).expect("read formatter source")
    }
}

impl Drop for TestPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn elamc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_elamc"))
}

fn run_fmt(input: &Path, arguments: &[&str]) -> std::process::Output {
    let mut command = elamc();
    command.arg("fmt").arg(input).args(arguments);
    command.output().expect("run elamc fmt")
}

#[test]
fn formats_a_single_file_quietly_with_the_default_width() {
    let package = TestPackage::new(
        "single",
        None,
        "fn main( )->( ):\n    let value=Point{ x:1,y }\n    pass\n",
    );
    let output = run_fmt(&package.source_path(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(
        package.source(),
        "fn main() -> ():\n    let value = Point { x: 1, y }\n    pass\n"
    );
}

#[test]
fn formats_concatenation_as_a_spaced_binary_operator() {
    let package = TestPackage::new(
        "concatenation",
        None,
        "fn main() -> ():\n    let message=\"hello\"++\"world\"\n    println(message)\n",
    );
    let output = run_fmt(&package.source_path(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        package.source(),
        "fn main() -> ():\n    let message = \"hello\" ++ \"world\"\n    println(message)\n"
    );
}

#[test]
fn package_formatting_uses_manifest_width_and_cli_override() {
    let source = "fn calculate(first: i32, second: i32, third: i32) -> i32:\n    return first\n";
    let package = TestPackage::new("configured", Some(40), source);
    let output = run_fmt(&package.root, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        package.source(),
        "fn calculate(\n    first: i32,\n    second: i32,\n    third: i32\n) -> i32:\n    return first\n"
    );

    fs::write(package.source_path(), source).expect("restore unformatted source");
    let output = run_fmt(&package.root, &["--line-length=100"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(package.source(), source);
}

#[test]
fn check_reports_changes_without_writing_and_accepts_formatted_sources() {
    let package = TestPackage::new(
        "check",
        None,
        "fn main( )->( ):\n    // retained\n    pass\n",
    );
    let before = package.source();
    let checked = run_fmt(&package.root, &["--check"]);
    assert!(!checked.status.success());
    assert_eq!(package.source(), before);
    assert!(
        String::from_utf8_lossy(&checked.stderr).contains("is not formatted"),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );

    assert!(run_fmt(&package.root, &[]).status.success());
    let checked = run_fmt(&package.root, &["--check"]);
    assert!(checked.status.success());
    assert_eq!(checked.stdout, b"");
    assert_eq!(checked.stderr, b"");
}

#[test]
fn invalid_source_is_diagnosed_and_left_untouched() {
    let package = TestPackage::new("invalid", None, "fn main( -> ():\n    pass\n");
    let before = package.source();
    let output = run_fmt(&package.root, &[]);
    assert!(!output.status.success());
    assert_eq!(package.source(), before);
    assert!(!output.stderr.is_empty());
}

#[test]
fn package_formatting_includes_owned_modules_but_not_dependencies() {
    let package = TestPackage::new("root", None, "fn main() -> ():\n    pass\n");
    let dependency = TestPackage::new("dependency", None, "pub fn value( )->i32:\n    return 1\n");
    let module = package.root.join("src/helper.elx");
    fs::write(&module, "pub fn helper( )->i32:\n    return 2\n").expect("write package module");
    let dependency_path = toml::Value::String(dependency.root.display().to_string()).to_string();
    fs::write(
        package.root.join("elamite.toml"),
        format!(
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n\n\
             [dependencies.dependency]\npath = {dependency_path}\n"
        ),
    )
    .expect("write dependency manifest");
    let dependency_before = dependency.source();

    let output = run_fmt(&package.root, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(module).expect("read formatted module"),
        "pub fn helper() -> i32:\n    return 2\n"
    );
    assert_eq!(dependency.source(), dependency_before);
}

#[test]
fn cli_rejects_zero_line_length_without_writing() {
    let package = TestPackage::new("zero", None, "fn main( )->( ):\n    pass\n");
    let before = package.source();
    let output = run_fmt(&package.root, &["--line-length=0"]);
    assert!(!output.status.success());
    assert_eq!(package.source(), before);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("greater than zero"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn every_shipped_source_formats_idempotently() {
    fn collect_sources(directory: &Path, output: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("read source entry").path();
            if path.is_dir() {
                collect_sources(&path, output);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("elx") {
                output.push(path);
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    collect_sources(&root.join("examples"), &mut paths);
    collect_sources(&root.join("stdlib"), &mut paths);
    paths.sort();
    for path in paths {
        let source = fs::read_to_string(&path).expect("read shipped source");
        let mut sources = SourceManager::new();
        let file = sources.add_text(path.clone(), source.clone());
        let formatted = format_source(file, &source, FormatOptions::default())
            .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", path.display()));
        let mut sources = SourceManager::new();
        let file = sources.add_text(path.clone(), formatted.clone());
        let repeated = format_source(file, &formatted, FormatOptions::default())
            .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", path.display()));
        assert_eq!(repeated, formatted, "{} was not idempotent", path.display());
    }
}
