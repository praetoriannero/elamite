//! Command-line interface for the Elamite compiler.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use codespan_reporting::diagnostic::{Diagnostic as CsDiagnostic, Label};
use codespan_reporting::term::{
    self,
    termcolor::{ColorChoice, StandardStream},
};
use elamite::backend::Target;
use elamite::diagnostics::Diagnostic;
use elamite::driver::{BuildOptions, Optimization, build, check_frontend, run};
use elamite::manifest::TargetKind;
use elamite::package::PackageGraph;
use elamite::scaffold::init_package;
use elamite::source::SourceManager;

#[derive(Debug, Parser)]
#[command(
    name = "elamite",
    version,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliTarget {
    #[value(name = "x86")]
    X86,
    #[value(name = "x86_64")]
    X86_64,
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
            println!("run with: elamite run {}", package.directory.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
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
                "elamite {} \u{2014} {} {} ({} source module{}, {} package{}, {} declaration{}, {} canonical types)",
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
