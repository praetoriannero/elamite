//! Reproducible package-level conformance runner.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::backend::Target;
use crate::diagnostics::Diagnostic;
use crate::driver::{BuildOptions, Optimization, build, run_with_environment};
use crate::package::PackageGraph;
use crate::source::SourceManager;

static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub filter: Option<String>,
    pub targets: Vec<Target>,
    pub optimizations: Vec<Optimization>,
    pub c_flags: Vec<std::ffi::OsString>,
    pub runtime_environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            filter: None,
            targets: vec![Target::host()],
            optimizations: vec![Optimization::Debug],
            c_flags: Vec::new(),
            runtime_environment: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaseResult {
    pub name: String,
    pub target: Target,
    pub optimization: Optimization,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct RunnerReport {
    pub cases: Vec<CaseResult>,
    /// Retained only when at least one case failed.
    pub retained_artifacts: Option<PathBuf>,
}

impl RunnerReport {
    #[must_use]
    pub fn success(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(|case| case.passed)
    }
}

/// Runs each immediate child package in `suite`.
///
/// A case is a directory containing `elamite.toml` and either a portable
/// `expected.stdout` or target-specific `expected.<target>.stdout` files.
/// Stderr and status expectations are optional and default to empty output and
/// status zero. When `suite` itself contains a manifest, it is one case. Build
/// output is isolated beneath a unique temporary directory, which is deleted
/// after a completely successful run and retained after failure.
pub fn run_suite(suite: &Path, options: &RunnerOptions) -> Result<RunnerReport, String> {
    let cases = discover_cases(suite)?;
    let serial = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let artifact_root = std::env::temp_dir().join(format!(
        "elamite-conformance-{}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(&artifact_root)
        .map_err(|error| format!("cannot create {}: {error}", artifact_root.display()))?;

    let targets = if options.targets.is_empty() {
        vec![Target::host()]
    } else {
        options.targets.clone()
    };
    let optimizations = if options.optimizations.is_empty() {
        vec![Optimization::Debug]
    } else {
        options.optimizations.clone()
    };
    let mut results = Vec::new();
    for case in cases {
        let name = case
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("case")
            .to_string();
        if options
            .filter
            .as_ref()
            .is_some_and(|filter| !name.contains(filter))
        {
            continue;
        }
        for target in &targets {
            for optimization in &optimizations {
                let output_directory = artifact_root.join(format!(
                    "{}-{}-{}",
                    sanitize(&name),
                    target_name(*target),
                    optimization_name(*optimization)
                ));
                results.push(run_case(
                    &case,
                    &name,
                    *target,
                    *optimization,
                    output_directory,
                    &options.c_flags,
                    &options.runtime_environment,
                ));
            }
        }
    }
    if results.is_empty() {
        let _ = fs::remove_dir_all(&artifact_root);
        return Err("no conformance cases matched the requested selection".to_string());
    }
    let failed = results.iter().any(|case| !case.passed);
    if failed {
        Ok(RunnerReport {
            cases: results,
            retained_artifacts: Some(artifact_root),
        })
    } else {
        let _ = fs::remove_dir_all(&artifact_root);
        Ok(RunnerReport {
            cases: results,
            retained_artifacts: None,
        })
    }
}

fn discover_cases(suite: &Path) -> Result<Vec<PathBuf>, String> {
    if suite.join("elamite.toml").is_file() {
        return Ok(vec![suite.to_path_buf()]);
    }
    let mut cases = fs::read_dir(suite)
        .map_err(|error| format!("cannot read suite {}: {error}", suite.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("elamite.toml").is_file())
        .collect::<Vec<_>>();
    cases.sort();
    if cases.is_empty() {
        Err(format!(
            "{} contains no package conformance cases",
            suite.display()
        ))
    } else {
        Ok(cases)
    }
}

fn run_case(
    case: &Path,
    name: &str,
    target: Target,
    optimization: Optimization,
    output_directory: PathBuf,
    c_flags: &[std::ffi::OsString],
    runtime_environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> CaseResult {
    let expected_stdout = match fs::read(expected_path(case, target, "stdout")) {
        Ok(output) => output,
        Err(error) => {
            return failure(
                name,
                target,
                optimization,
                format!("cannot read expected.stdout: {error}"),
            );
        }
    };
    let expected_stderr = fs::read(expected_path(case, target, "stderr")).unwrap_or_default();
    let expected_status = fs::read_to_string(expected_path(case, target, "status"))
        .ok()
        .and_then(|status| status.trim().parse::<i32>().ok())
        .unwrap_or(0);

    let mut sources = SourceManager::new();
    let graph = match PackageGraph::resolve(&case.join("elamite.toml"), &mut sources) {
        Ok(graph) => graph,
        Err(diagnostics) => {
            return failure(
                name,
                target,
                optimization,
                diagnostic_text(&sources, &diagnostics),
            );
        }
    };
    let artifact = match build(
        &graph,
        &mut sources,
        &BuildOptions {
            target,
            optimization,
            output_directory,
            keep_generated_c: true,
            c_compiler: None,
            c_flags: c_flags.to_vec(),
        },
    ) {
        Ok(artifact) => artifact,
        Err(diagnostics) => {
            return failure(
                name,
                target,
                optimization,
                diagnostic_text(&sources, &diagnostics),
            );
        }
    };
    let output = match run_with_environment(&artifact, runtime_environment) {
        Ok(output) => output,
        Err(diagnostic) => {
            return failure(
                name,
                target,
                optimization,
                diagnostic_text(&sources, &[diagnostic]),
            );
        }
    };
    let actual_status = output.status.code().unwrap_or(-1);
    let mut mismatches = Vec::new();
    if output.stdout != expected_stdout {
        mismatches.push(format!(
            "stdout differed\nexpected: {:?}\nactual:   {:?}",
            String::from_utf8_lossy(&expected_stdout),
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    if output.stderr != expected_stderr {
        mismatches.push(format!(
            "stderr differed\nexpected: {:?}\nactual:   {:?}",
            String::from_utf8_lossy(&expected_stderr),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if actual_status != expected_status {
        mismatches.push(format!(
            "status differed: expected {expected_status}, actual {actual_status}"
        ));
    }
    if mismatches.is_empty() {
        CaseResult {
            name: name.to_string(),
            target,
            optimization,
            passed: true,
            detail: "output and status matched".to_string(),
        }
    } else {
        failure(name, target, optimization, mismatches.join("\n"))
    }
}

fn expected_path(case: &Path, target: Target, kind: &str) -> PathBuf {
    let targeted = case.join(format!("expected.{}.{}", target_name(target), kind));
    if targeted.is_file() {
        targeted
    } else {
        case.join(format!("expected.{kind}"))
    }
}

fn failure(name: &str, target: Target, optimization: Optimization, detail: String) -> CaseResult {
    CaseResult {
        name: name.to_string(),
        target,
        optimization,
        passed: false,
        detail,
    }
}

fn diagnostic_text(sources: &SourceManager, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic.primary.map_or_else(
                || format!("{:?}: {}", diagnostic.category, diagnostic.message),
                |span| {
                    let position = sources.line_col(span.file, span.start);
                    format!(
                        "{}:{}:{}: {:?}: {}",
                        sources.path(span.file).display(),
                        position.line,
                        position.column,
                        diagnostic.category,
                        diagnostic.message
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn target_name(target: Target) -> &'static str {
    match target {
        Target::X86 => "x86",
        Target::X86_64 => "x86_64",
    }
}

fn optimization_name(optimization: Optimization) -> &'static str {
    match optimization {
        Optimization::Debug => "debug",
        Optimization::Release => "release",
    }
}
