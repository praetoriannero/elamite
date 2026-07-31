//! Native package-test discovery, compilation, isolation, and reporting.

use std::ffi::OsString;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::diagnostics::Diagnostic;
use crate::driver::{BuildOptions, TestCase, build_tests};
use crate::package::PackageGraph;
use crate::source::SourceManager;

static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct TestOptions {
    pub build: BuildOptions,
    pub filter: Option<String>,
    pub runtime_environment: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub struct TestReport {
    pub results: Vec<TestResult>,
}

impl TestReport {
    #[must_use]
    pub fn success(&self) -> bool {
        self.results.iter().all(|result| result.passed)
    }
}

#[derive(Debug)]
pub enum TestError {
    Diagnostics(Vec<Diagnostic>),
    Selection(String),
    Execution(Diagnostic),
}

/// Builds the selected package's test-only artifact once, then executes every
/// selected declaration in a fresh native process.
pub fn run_package(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    options: &TestOptions,
) -> Result<TestReport, TestError> {
    let mut build = options.build.clone();
    let serial = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    build.output_directory = build
        .output_directory
        .join(format!("package-test-{}-{serial}", std::process::id()));
    let built = build_tests(graph, sources, &build).map_err(TestError::Diagnostics)?;
    let selected = match select_cases(&built.cases, options.filter.as_deref()) {
        Ok(selected) => selected,
        Err(error) => {
            remove_generated_artifacts(&build);
            return Err(error);
        }
    };
    let mut results = Vec::new();
    for case in selected {
        let output = match Command::new(&built.artifact.path)
            .envs(options.runtime_environment.iter().cloned())
            .env("ELAMITE_TEST", &case.name)
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                remove_generated_artifacts(&build);
                return Err(TestError::Execution(Diagnostic::new(
                    crate::diagnostics::Category::Toolchain,
                    format!(
                        "cannot execute test `{}` through {}: {error}",
                        case.name,
                        built.artifact.path.display()
                    ),
                )));
            }
        };
        results.push(TestResult {
            name: case.name.clone(),
            passed: output.status.success(),
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        });
    }
    let report = TestReport { results };
    remove_generated_artifacts(&build);
    Ok(report)
}

fn remove_generated_artifacts(build: &BuildOptions) {
    if !build.keep_generated_c {
        let _ = std::fs::remove_dir_all(&build.output_directory);
    }
}

fn select_cases<'a>(
    cases: &'a [TestCase],
    filter: Option<&str>,
) -> Result<Vec<&'a TestCase>, TestError> {
    let selected = cases
        .iter()
        .filter(|case| {
            filter.is_none_or(|filter| case.name == filter || case.name.contains(filter))
        })
        .collect::<Vec<_>>();
    if filter.is_some() && selected.is_empty() {
        return Err(TestError::Selection(format!(
            "test filter `{}` matched no tests",
            filter.unwrap_or_default()
        )));
    }
    Ok(selected)
}
