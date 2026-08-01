//! Command-line interface for the Elamite compiler.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use codespan_reporting::diagnostic::{Diagnostic as CsDiagnostic, Label};
use codespan_reporting::term::{
    self,
    termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor},
};
use elamite::config::{CompilerFeatures, Optimization, Target};
use elamite::diagnostics::Diagnostic;
use elamite::driver::{
    BuildOptions, DumpStage, build, build_to, check_frontend_with_features, dump_with_features, run,
};
use elamite::formatter::{FormatOptions, format_source};
use elamite::manifest::TargetKind;
use elamite::package::{Package, PackageGraph};
use elamite::resolution::resolve_with_features;
use elamite::scaffold::init_package;
use elamite::source::SourceManager;

#[derive(Debug, Parser)]
#[command(
    name = "elamc",
    version = concat!(env!("CARGO_PKG_VERSION"), " (SPEC 0.9.0-draft)"),
    about = "The Elamite programming language compiler",
    arg_required_else_help = true,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    direct: DirectBuildArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check a package or source file without generating native code.
    Check(CheckArgs),
    /// Compile a package or source file into a native artifact.
    Build(BuildArgs),
    /// Compile and run an executable package or source file.
    Run(BuildArgs),
    /// Create a new hello-world package.
    Init(InitArgs),
    /// Print a deterministic compiler intermediate representation.
    Dump(DumpArgs),
    /// Format an Elamite source file or package.
    Fmt(FmtArgs),
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
struct DirectBuildArgs {
    /// Compile one .elx source file directly.
    #[arg(value_name = "SOURCE", required = true)]
    source: Option<PathBuf>,

    /// Native target architecture; defaults to the host architecture.
    #[arg(long, value_enum, value_name = "ARCH")]
    target: Option<CliTarget>,

    #[command(flatten)]
    features: FeatureArgs,

    #[command(flatten)]
    native: NativeBuildArgs,

    /// Write the executable to this exact path.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct BuildArgs {
    #[command(flatten)]
    package: PackageArgs,

    #[command(flatten)]
    native: NativeBuildArgs,

    /// Write the final executable or object to this exact path.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct NativeBuildArgs {
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
    /// Package directory containing elamite.toml, or one .elx source file.
    #[arg(value_name = "INPUT", default_value = ".")]
    input: PathBuf,

    /// Native target architecture; defaults to the host architecture.
    #[arg(long, value_enum, value_name = "ARCH")]
    target: Option<CliTarget>,

    #[command(flatten)]
    features: FeatureArgs,
}

#[derive(Debug, Clone, Copy, Default, Args)]
struct FeatureArgs {}

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

    #[command(flatten)]
    features: FeatureArgs,
}

#[derive(Debug, Args)]
struct FmtArgs {
    /// Package directory containing elamite.toml, or one .elx source file.
    #[arg(value_name = "INPUT", default_value = ".")]
    input: PathBuf,

    /// Verify formatting without changing files.
    #[arg(long)]
    check: bool,

    /// Override the preferred maximum line length.
    #[arg(long, value_name = "COLUMNS", value_parser = parse_line_length)]
    line_length: Option<usize>,
}

#[derive(Debug, Args)]
struct PackageTestArgs {
    /// Package directory containing elamite.toml.
    #[arg(value_name = "PACKAGE", default_value = ".")]
    package_dir: PathBuf,

    /// Native target architecture; defaults to the host architecture.
    #[arg(long, value_enum, value_name = "ARCH")]
    target: Option<CliTarget>,

    #[command(flatten)]
    features: FeatureArgs,

    #[command(flatten)]
    native: NativeBuildArgs,

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

    #[command(flatten)]
    features: FeatureArgs,
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
    Expanded,
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
            CliDumpStage::Expanded => Self::Expanded,
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

impl From<FeatureArgs> for CompilerFeatures {
    fn from(_features: FeatureArgs) -> Self {
        Self::default()
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
    input: PathBuf,
    target: Target,
    optimization: Optimization,
    features: CompilerFeatures,
    output_directory: PathBuf,
    output_file: Option<PathBuf>,
    keep_generated_c: bool,
    c_compiler: Option<OsString>,
    c_flags: Vec<OsString>,
}

impl CompileRequest {
    fn check(arguments: CheckArgs) -> Self {
        let input = arguments.package.input;
        Self {
            command: CompileCommand::Check,
            target: arguments
                .package
                .target
                .map_or_else(Target::host, Target::from),
            output_directory: default_output_directory(&input),
            input,
            output_file: None,
            optimization: Optimization::Debug,
            features: arguments.package.features.into(),
            keep_generated_c: false,
            c_compiler: None,
            c_flags: Vec::new(),
        }
    }

    fn build(command: CompileCommand, arguments: BuildArgs) -> Self {
        Self::native(
            command,
            arguments.package,
            arguments.native,
            arguments.output,
        )
    }

    fn native(
        command: CompileCommand,
        package: PackageArgs,
        native: NativeBuildArgs,
        output_file: Option<PathBuf>,
    ) -> Self {
        let input = package.input;
        let output_directory = native
            .out_dir
            .unwrap_or_else(|| default_output_directory(&input));
        Self {
            command,
            target: package.target.map_or_else(Target::host, Target::from),
            input,
            optimization: if native.release {
                Optimization::Release
            } else {
                Optimization::Debug
            },
            features: package.features.into(),
            output_directory,
            output_file,
            keep_generated_c: native.keep_c,
            c_compiler: native.cc,
            c_flags: native.c_flag,
        }
    }
}

fn default_output_directory(input: &std::path::Path) -> PathBuf {
    if input.is_file() || input.extension().and_then(|extension| extension.to_str()) == Some("elx")
    {
        input
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("build")
    } else {
        input.join("build")
    }
}

fn parse_line_length(value: &str) -> Result<usize, String> {
    let line_length = value
        .parse::<usize>()
        .map_err(|_| "line length must be a positive integer".to_string())?;
    if line_length == 0 {
        Err("line length must be greater than zero".to_string())
    } else {
        Ok(line_length)
    }
}

fn main() -> ExitCode {
    let arguments = Cli::parse();
    match arguments.command {
        Some(Command::Check(arguments)) => compile_package(CompileRequest::check(arguments)),
        Some(Command::Build(arguments)) => {
            compile_package(CompileRequest::build(CompileCommand::Build, arguments))
        }
        Some(Command::Run(arguments)) => {
            compile_package(CompileRequest::build(CompileCommand::Run, arguments))
        }
        Some(Command::Init(arguments)) => initialize_package(arguments),
        Some(Command::Dump(arguments)) => dump_package(arguments),
        Some(Command::Fmt(arguments)) => format_input(arguments),
        Some(Command::Doc(arguments)) => document_package(arguments),
        Some(Command::Test(arguments)) => test_package(arguments),
        Some(Command::Conformance(arguments)) => test_packages(arguments),
        None => compile_package(CompileRequest::native(
            CompileCommand::Build,
            PackageArgs {
                input: arguments
                    .direct
                    .source
                    .expect("Clap requires SOURCE when no subcommand is present"),
                target: arguments.direct.target,
                features: arguments.direct.features,
            },
            arguments.direct.native,
            arguments.direct.output,
        )),
    }
}

fn format_input(arguments: FmtArgs) -> ExitCode {
    let mut sources = SourceManager::new();
    let is_file_input = arguments.input.is_file()
        || arguments
            .input
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("elx");
    let (mut paths, configured_line_length) = if is_file_input {
        if arguments
            .input
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("elx")
        {
            render_diagnostics(
                &sources,
                &[Diagnostic::new(
                    elamite::diagnostics::Category::Formatting,
                    format!(
                        "format input {} must have the `.elx` extension",
                        arguments.input.display()
                    ),
                )],
            );
            return ExitCode::FAILURE;
        }
        (
            vec![arguments.input],
            elamite::formatter::DEFAULT_LINE_LENGTH,
        )
    } else {
        let manifest_path = arguments.input.join("elamite.toml");
        let package = match Package::load(&manifest_path, &mut sources) {
            Ok(package) => package,
            Err(diagnostics) => {
                render_diagnostics(&sources, &diagnostics);
                return ExitCode::FAILURE;
            }
        };
        let mut paths = vec![package.manifest_dir.join(&package.manifest.root)];
        paths.extend(package.modules.into_values());
        (paths, package.manifest.format_line_length)
    };
    paths.sort();
    paths.dedup();
    let options = FormatOptions {
        line_length: arguments.line_length.unwrap_or(configured_line_length),
    };

    let mut changes = Vec::<(PathBuf, String)>::new();
    let mut diagnostics = Vec::new();
    for path in paths {
        let file = match sources.load_file(&path) {
            Ok(file) => file,
            Err(error) => {
                diagnostics.push(Diagnostic::new(
                    elamite::diagnostics::Category::Formatting,
                    error.to_string(),
                ));
                continue;
            }
        };
        let source = sources.text(file);
        match format_source(file, source, options) {
            Ok(formatted) if formatted != source => changes.push((path, formatted)),
            Ok(_) => {}
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }
    if !diagnostics.is_empty() {
        render_diagnostics(&sources, &diagnostics);
        return ExitCode::FAILURE;
    }
    if arguments.check {
        if changes.is_empty() {
            return ExitCode::SUCCESS;
        }
        let diagnostics = changes
            .iter()
            .map(|(path, _)| {
                Diagnostic::new(
                    elamite::diagnostics::Category::Formatting,
                    format!("{} is not formatted", path.display()),
                )
            })
            .collect::<Vec<_>>();
        render_diagnostics(&sources, &diagnostics);
        return ExitCode::FAILURE;
    }

    for (path, formatted) in changes {
        if let Err(error) = replace_file_atomically(&path, formatted.as_bytes()) {
            render_diagnostics(
                &sources,
                &[Diagnostic::new(
                    elamite::diagnostics::Category::Formatting,
                    format!("cannot replace {}: {error}", path.display()),
                )],
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn replace_file_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("source");
    let permissions = fs::metadata(path)?.permissions();
    for attempt in 0..100u32 {
        let temporary = parent.join(format!(
            ".{file_name}.elamite-fmt-{}-{attempt}",
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            fs::set_permissions(&temporary, permissions.clone())?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a temporary formatter file",
    ))
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
    let input = arguments.package.input;
    let target = arguments
        .package
        .target
        .map_or_else(Target::host, Target::from);
    let mut sources = SourceManager::new();
    let graph = match PackageGraph::resolve_input(&input, &mut sources) {
        Ok(graph) => graph,
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            return ExitCode::FAILURE;
        }
    };
    match dump_with_features(
        &graph,
        &mut sources,
        target,
        arguments.stage.into(),
        arguments.package.features.into(),
    ) {
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
    let output = resolve_with_features(&graph, &mut sources, arguments.features.into());
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
        features: arguments.features.into(),
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
    let PackageTestArgs {
        package_dir,
        target,
        features,
        native,
        filter,
    } = arguments;
    let request = CompileRequest::native(
        CompileCommand::Build,
        PackageArgs {
            input: package_dir,
            target,
            features,
        },
        native,
        None,
    );
    let manifest_path = request.input.join("elamite.toml");
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
                features: request.features,
                output_directory: request.output_directory.join("tests"),
                keep_generated_c: request.keep_generated_c,
                c_compiler: request.c_compiler,
                c_flags: request.c_flags,
            },
            filter,
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
    let mut sources = SourceManager::new();
    match PackageGraph::resolve_input(&request.input, &mut sources) {
        Ok(graph) => {
            if request.command != CompileCommand::Check {
                let output_file = request.output_file;
                let options = BuildOptions {
                    target: request.target,
                    optimization: request.optimization,
                    features: request.features,
                    output_directory: request.output_directory,
                    keep_generated_c: request.keep_generated_c,
                    c_compiler: request.c_compiler,
                    c_flags: request.c_flags,
                };
                let result = match output_file {
                    Some(output) => build_to(&graph, &mut sources, &options, &output),
                    None => build(&graph, &mut sources, &options),
                };
                let artifact = match result {
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
                return ExitCode::SUCCESS;
            }
            let root = &graph.packages[&graph.root];
            let frontend = match check_frontend_with_features(
                &graph,
                &mut sources,
                request.target,
                request.features,
            ) {
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
