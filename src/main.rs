//! Command-line interface for the Elamite compiler.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use codespan_reporting::diagnostic::{Diagnostic as CsDiagnostic, Label};
use codespan_reporting::term::{
    self,
    termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor},
};
use elamite::config::{Optimization, Target};
use elamite::diagnostics::Diagnostic;
use elamite::driver::{BuildOptions, DumpStage, build, check_frontend, dump, run};
use elamite::manifest::TargetKind;
use elamite::package::PackageGraph;
use elamite::resolution::resolve;
use elamite::scaffold::init_package;
use elamite::source::SourceManager;

#[derive(Debug, Parser)]
#[command(
    name = "elamc",
    version = concat!(env!("CARGO_PKG_VERSION"), " (SPEC 0.5.0-draft)"),
    about = "The Elamite programming language compiler",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check a package without generating native code.
    Check(CheckArgs),
    /// Compile a package into a native artifact.
    Build(BuildArgs),
    /// Compile and run an executable package.
    Run(BuildArgs),
    /// Create a new hello-world package.
    Init(InitArgs),
    /// Print a deterministic compiler intermediate representation.
    Dump(DumpArgs),
    /// Extract public API documentation as Markdown.
    Doc(DocArgs),
    /// Compile and run language-native package tests.
    Test(PackageTestArgs),
    /// Run compiler conformance fixtures.
    Conformance(ConformanceArgs),
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[command(flatten)]
    package: PackageArgs,
}

#[derive(Debug, Args)]
struct BuildArgs {
    #[command(flatten)]
    package: PackageArgs,

    /// Optimize the generated native artifact.
    #[arg(long)]
    release: bool,

    /// Directory for generated and native artifacts.
    #[arg(long, value_name = "PATH")]
    out_dir: Option<PathBuf>,

    /// C compiler executable to invoke.
    #[arg(long, value_name = "PATH")]
    cc: Option<OsString>,

    /// Retain the generated C translation unit.
    #[arg(long)]
    keep_c: bool,

    /// Pass an additional hardening or instrumentation flag to the C compiler.
    #[arg(long, value_name = "FLAG", allow_hyphen_values = true)]
    c_flag: Vec<OsString>,
}

#[derive(Debug, Args)]
struct PackageArgs {
    /// Package directory containing elamite.toml.
    #[arg(value_name = "PACKAGE", default_value = ".")]
    package_dir: PathBuf,

    /// Native target architecture; defaults to the host architecture.
    #[arg(long, value_enum, value_name = "ARCH")]
    target: Option<CliTarget>,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Directory to initialize.
    #[arg(value_name = "PATH", default_value = ".")]
    path: PathBuf,

    /// Package name; defaults to the destination directory name.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// Create a library package instead of an executable.
    #[arg(long)]
    lib: bool,
}

#[derive(Debug, Args)]
struct DumpArgs {
    /// Intermediate representation to print.
    #[arg(value_enum, value_name = "STAGE")]
    stage: CliDumpStage,

    #[command(flatten)]
    package: PackageArgs,
}

#[derive(Debug, Args)]
struct DocArgs {
    /// Package directory containing elamite.toml.
    #[arg(value_name = "PACKAGE", default_value = ".")]
    package_dir: PathBuf,
}

#[derive(Debug, Args)]
struct PackageTestArgs {
    #[command(flatten)]
    build: BuildArgs,

    /// Run only exact or qualified test names containing this text.
    #[arg(long, value_name = "TEXT")]
    filter: Option<String>,
}

#[derive(Debug, Args)]
struct ConformanceArgs {
    /// Suite directory, or one package fixture directory.
    #[arg(value_name = "SUITE")]
    suite: PathBuf,

    /// Run only case names containing this text.
    #[arg(long, value_name = "TEXT")]
    filter: Option<String>,

    /// Target one architecture; defaults to the host.
    #[arg(long, value_enum, conflicts_with = "all_targets")]
    target: Option<CliTarget>,

    /// Run both supported target architectures.
    #[arg(long)]
    all_targets: bool,

    /// Use release optimization.
    #[arg(long, conflicts_with = "all_modes")]
    release: bool,

    /// Run both debug and release configurations.
    #[arg(long)]
    all_modes: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliTarget {
    #[value(name = "x86")]
    X86,
    #[value(name = "x86_64")]
    X86_64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliDumpStage {
    Tokens,
    Syntax,
    Resolution,
    Types,
    #[value(name = "typed-ir")]
    TypedIr,
    #[value(name = "control-flow")]
    ControlFlow,
    Monomorphized,
    #[value(name = "generated-c")]
    GeneratedC,
}

impl From<CliDumpStage> for DumpStage {
    fn from(stage: CliDumpStage) -> Self {
        match stage {
            CliDumpStage::Tokens => Self::Tokens,
            CliDumpStage::Syntax => Self::Syntax,
            CliDumpStage::Resolution => Self::Resolution,
            CliDumpStage::Types => Self::Types,
            CliDumpStage::TypedIr => Self::TypedIr,
            CliDumpStage::ControlFlow => Self::ControlFlow,
            CliDumpStage::Monomorphized => Self::Monomorphized,
            CliDumpStage::GeneratedC => Self::GeneratedC,
        }
    }
}

impl From<CliTarget> for Target {
    fn from(target: CliTarget) -> Self {
        match target {
            CliTarget::X86 => Self::X86,
            CliTarget::X86_64 => Self::X86_64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompileCommand {
    Check,
    Build,
    Run,
}

struct CompileRequest {
    command: CompileCommand,
    package_dir: PathBuf,
    target: Target,
    optimization: Optimization,
    output_directory: PathBuf,
    keep_generated_c: bool,
    c_compiler: Option<OsString>,
    c_flags: Vec<OsString>,
}

impl CompileRequest {
    fn check(arguments: CheckArgs) -> Self {
        let package_dir = arguments.package.package_dir;
        Self {
            command: CompileCommand::Check,
            target: arguments
                .package
                .target
                .map_or_else(Target::host, Target::from),
            output_directory: package_dir.join("build"),
            package_dir,
            optimization: Optimization::Debug,
            keep_generated_c: false,
            c_compiler: None,
            c_flags: Vec::new(),
        }
    }

    fn build(command: CompileCommand, arguments: BuildArgs) -> Self {
        let package_dir = arguments.package.package_dir;
        let output_directory = arguments
            .out_dir
            .unwrap_or_else(|| package_dir.join("build"));
        Self {
            command,
            target: arguments
                .package
                .target
                .map_or_else(Target::host, Target::from),
            package_dir,
            optimization: if arguments.release {
                Optimization::Release
            } else {
                Optimization::Debug
            },
            output_directory,
            keep_generated_c: arguments.keep_c,
            c_compiler: arguments.cc,
            c_flags: arguments.c_flag,
        }
    }
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check(arguments) => compile_package(CompileRequest::check(arguments)),
        Command::Build(arguments) => {
            compile_package(CompileRequest::build(CompileCommand::Build, arguments))
        }
        Command::Run(arguments) => {
            compile_package(CompileRequest::build(CompileCommand::Run, arguments))
        }
        Command::Init(arguments) => initialize_package(arguments),
        Command::Dump(arguments) => dump_package(arguments),
        Command::Doc(arguments) => document_package(arguments),
        Command::Test(arguments) => test_package(arguments),
        Command::Conformance(arguments) => test_packages(arguments),
    }
}

fn initialize_package(arguments: InitArgs) -> ExitCode {
    let target_kind = if arguments.lib {
        TargetKind::Library
    } else {
        TargetKind::Executable
    };
    match init_package(&arguments.path, arguments.name.as_deref(), target_kind) {
        Ok(package) => {
            println!("initialized {}", package.directory.display());
            println!("run with: elamc run {}", package.directory.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dump_package(arguments: DumpArgs) -> ExitCode {
    let manifest_path = arguments.package.package_dir.join("elamite.toml");
    let target = arguments
        .package
        .target
        .map_or_else(Target::host, Target::from);
    let mut sources = SourceManager::new();
    let graph = match PackageGraph::resolve(&manifest_path, &mut sources) {
        Ok(graph) => graph,
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            return ExitCode::FAILURE;
        }
    };
    match dump(&graph, &mut sources, target, arguments.stage.into()) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            ExitCode::FAILURE
        }
    }
}

fn document_package(arguments: DocArgs) -> ExitCode {
    let manifest_path = arguments.package_dir.join("elamite.toml");
    let mut sources = SourceManager::new();
    let graph = match PackageGraph::resolve(&manifest_path, &mut sources) {
        Ok(graph) => graph,
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            return ExitCode::FAILURE;
        }
    };
    let output = resolve(&graph, &mut sources);
    let documentation = elamite::docs::extract(&output.program, &sources, &graph.root);
    print!("{}", documentation.markdown());
    if output.diagnostics.is_empty() {
        ExitCode::SUCCESS
    } else {
        render_diagnostics(&sources, &output.diagnostics);
        ExitCode::FAILURE
    }
}

fn test_packages(arguments: ConformanceArgs) -> ExitCode {
    let targets = if arguments.all_targets {
        vec![Target::X86, Target::X86_64]
    } else {
        vec![arguments.target.map_or_else(Target::host, Target::from)]
    };
    let optimizations = if arguments.all_modes {
        vec![Optimization::Debug, Optimization::Release]
    } else if arguments.release {
        vec![Optimization::Release]
    } else {
        vec![Optimization::Debug]
    };
    let options = elamite::conformance::RunnerOptions {
        filter: arguments.filter,
        targets,
        optimizations,
        c_flags: Vec::new(),
        runtime_environment: Vec::new(),
    };
    let report = match elamite::conformance::run_suite(&arguments.suite, &options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    for case in &report.cases {
        let status = if case.passed { "pass" } else { "FAIL" };
        println!(
            "{status} {} {:?} {:?}: {}",
            case.name, case.target, case.optimization, case.detail
        );
    }
    if let Some(path) = &report.retained_artifacts {
        eprintln!("retained failing artifacts at {}", path.display());
    }
    if report.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn test_package(arguments: PackageTestArgs) -> ExitCode {
    let request = CompileRequest::build(CompileCommand::Build, arguments.build);
    let manifest_path = request.package_dir.join("elamite.toml");
    let mut sources = SourceManager::new();
    let graph = match PackageGraph::resolve(&manifest_path, &mut sources) {
        Ok(graph) => graph,
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            return ExitCode::from(2);
        }
    };
    let report = elamite::testing::run_package(
        &graph,
        &mut sources,
        &elamite::testing::TestOptions {
            build: BuildOptions {
                target: request.target,
                optimization: request.optimization,
                output_directory: request.output_directory.join("tests"),
                keep_generated_c: request.keep_generated_c,
                c_compiler: request.c_compiler,
                c_flags: request.c_flags,
            },
            filter: arguments.filter,
            runtime_environment: Vec::new(),
        },
    );
    let report = match report {
        Ok(report) => report,
        Err(elamite::testing::TestError::Diagnostics(diagnostics)) => {
            render_diagnostics(&sources, &diagnostics);
            return ExitCode::from(2);
        }
        Err(elamite::testing::TestError::Selection(message)) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
        Err(elamite::testing::TestError::Execution(diagnostic)) => {
            render_diagnostics(&sources, &[diagnostic]);
            return ExitCode::from(2);
        }
    };
    render_package_test_report(&report);
    if report.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn render_package_test_report(report: &elamite::testing::TestReport) {
    let stdout = StandardStream::stdout(ColorChoice::Auto);
    let stderr = StandardStream::stderr(ColorChoice::Auto);
    let mut stdout = stdout.lock();
    let mut stderr = stderr.lock();
    let name_width = report
        .results
        .iter()
        .map(|result| result.name.len())
        .max()
        .unwrap_or(0);

    let _ = writeln!(
        stdout,
        "running {} test{}",
        report.results.len(),
        if report.results.len() == 1 { "" } else { "s" }
    );
    for result in &report.results {
        let _ = write!(stdout, "test {:name_width$} ... ", result.name);
        let (status, color) = if result.passed {
            ("ok", Color::Green)
        } else {
            ("FAILED", Color::Red)
        };
        let _ = write_colored(&mut stdout, status, color, !result.passed);
        let _ = writeln!(stdout);
        let _ = write_captured_output(&mut stdout, "stdout", &result.stdout);
        let _ = stdout.flush();
        let _ = write_captured_output(&mut stderr, "stderr", &result.stderr);
        let _ = stderr.flush();
    }

    let passed = report.results.iter().filter(|result| result.passed).count();
    let _ = writeln!(stdout);
    let _ = write!(stdout, "test result: ");
    let _ = write_colored(
        &mut stdout,
        if report.success() { "ok" } else { "FAILED" },
        if report.success() {
            Color::Green
        } else {
            Color::Red
        },
        !report.success(),
    );
    let _ = writeln!(
        stdout,
        ". {} passed; {} failed",
        passed,
        report.results.len() - passed
    );
}

fn write_colored(
    writer: &mut impl WriteColor,
    text: &str,
    color: Color,
    bold: bool,
) -> std::io::Result<()> {
    writer.set_color(ColorSpec::new().set_fg(Some(color)).set_bold(bold))?;
    write!(writer, "{text}")?;
    writer.reset()
}

fn write_captured_output(
    writer: &mut impl WriteColor,
    label: &str,
    output: &[u8],
) -> std::io::Result<()> {
    if output.is_empty() {
        return Ok(());
    }
    writer.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)).set_dimmed(true))?;
    writeln!(writer, "  {label}:")?;
    writer.reset()?;
    writer.write_all(output)?;
    if !output.ends_with(b"\n") {
        writeln!(writer)?;
    }
    Ok(())
}

fn compile_package(request: CompileRequest) -> ExitCode {
    let manifest_path = request.package_dir.join("elamite.toml");
    let mut sources = SourceManager::new();
    match PackageGraph::resolve(&manifest_path, &mut sources) {
        Ok(graph) => {
            if request.command != CompileCommand::Check {
                let artifact = match build(
                    &graph,
                    &mut sources,
                    &BuildOptions {
                        target: request.target,
                        optimization: request.optimization,
                        output_directory: request.output_directory,
                        keep_generated_c: request.keep_generated_c,
                        c_compiler: request.c_compiler,
                        c_flags: request.c_flags,
                    },
                ) {
                    Ok(artifact) => artifact,
                    Err(diagnostics) => {
                        render_diagnostics(&sources, &diagnostics);
                        return ExitCode::FAILURE;
                    }
                };
                if request.command == CompileCommand::Run {
                    let result = match run(&artifact) {
                        Ok(result) => result,
                        Err(diagnostic) => {
                            render_diagnostics(&sources, &[diagnostic]);
                            return ExitCode::FAILURE;
                        }
                    };
                    let _ = std::io::stdout().write_all(&result.stdout);
                    let _ = std::io::stderr().write_all(&result.stderr);
                    return result.status.code().map_or(ExitCode::FAILURE, |code| {
                        ExitCode::from(u8::try_from(code).unwrap_or(1))
                    });
                }
                println!("built {}", artifact.path.display());
                if let Some(path) = artifact.generated_c_path {
                    println!("generated C {}", path.display());
                }
                return ExitCode::SUCCESS;
            }
            let root = &graph.packages[&graph.root];
            let frontend = match check_frontend(&graph, &mut sources, request.target) {
                Ok(frontend) => frontend,
                Err(diagnostics) => {
                    render_diagnostics(&sources, &diagnostics);
                    return ExitCode::FAILURE;
                }
            };
            let source_module_count = root.modules.len() + 1;
            println!(
                "elamc {} \u{2014} {} {} ({} source module{}, {} package{}, {} declaration{}, {} canonical types)",
                elamite::version(),
                root.manifest.name,
                root.manifest.version,
                source_module_count,
                if source_module_count == 1 { "" } else { "s" },
                graph.packages.len(),
                if graph.packages.len() == 1 { "" } else { "s" },
                frontend.resolved.declarations.len(),
                if frontend.resolved.declarations.len() == 1 {
                    ""
                } else {
                    "s"
                },
                frontend.typed.types.len(),
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            ExitCode::FAILURE
        }
    }
}

/// Renders every diagnostic against `sources` using `codespan-reporting`'s
/// rustc-style span/underline output when a diagnostic carries a span, and a
/// plain message otherwise.
fn render_diagnostics(sources: &SourceManager, diagnostics: &[Diagnostic]) {
    let writer = StandardStream::stderr(ColorChoice::Auto);
    let config = term::Config::default();

    for diagnostic in diagnostics {
        let mut labels = Vec::new();
        if let Some(span) = diagnostic.primary {
            labels.push(Label::primary(
                span.file,
                span.start as usize..span.end as usize,
            ));
        }
        for (span, message) in &diagnostic.related {
            labels.push(
                Label::secondary(span.file, span.start as usize..span.end as usize)
                    .with_message(message),
            );
        }

        let rendered = CsDiagnostic::error()
            .with_message(&diagnostic.message)
            .with_labels(labels);
        let _ = term::emit_to_write_style(&mut writer.lock(), &config, sources, &rendered);
    }
}
