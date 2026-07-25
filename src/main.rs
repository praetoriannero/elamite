//! Initial command-line entry point through the Milestone 8 native backend.
//! The polished `clap` surface and IR dump modes remain Milestone 18.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use codespan_reporting::diagnostic::{Diagnostic as CsDiagnostic, Label};
use codespan_reporting::term::{
    self,
    termcolor::{ColorChoice, StandardStream},
};
use elamite::backend::Target;
use elamite::check::check_for_target;
use elamite::diagnostics::Diagnostic;
use elamite::driver::{BuildOptions, Optimization, build, run};
use elamite::package::PackageGraph;
use elamite::resolution::resolve;
use elamite::source::SourceManager;
use elamite::types::resolve_types;

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args_os().skip(1)) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let package_dir = cli.package_dir.clone();
    let manifest_path = package_dir.join("elamite.toml");

    let mut sources = SourceManager::new();
    match PackageGraph::resolve(&manifest_path, &mut sources) {
        Ok(graph) => {
            if cli.command != CommandKind::Check {
                let artifact = match build(
                    &graph,
                    &mut sources,
                    &BuildOptions {
                        target: cli.target,
                        optimization: cli.optimization,
                        output_directory: cli.output_directory,
                        keep_generated_c: cli.keep_generated_c,
                        c_compiler: cli.c_compiler,
                    },
                ) {
                    Ok(artifact) => artifact,
                    Err(diagnostics) => {
                        render_diagnostics(&sources, &diagnostics);
                        return ExitCode::FAILURE;
                    }
                };
                if cli.command == CommandKind::Run {
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
            let resolution = resolve(&graph, &mut sources);
            if !resolution.diagnostics.is_empty() {
                render_diagnostics(&sources, &resolution.diagnostics);
                return ExitCode::FAILURE;
            }
            let mut typed = resolve_types(&resolution.program);
            if !typed.diagnostics.is_empty() {
                render_diagnostics(&sources, &typed.diagnostics);
                return ExitCode::FAILURE;
            }
            let checked = check_for_target(
                &resolution.program,
                &mut typed.program,
                cli.target.pointer_bits(),
            );
            if !checked.diagnostics.is_empty() {
                render_diagnostics(&sources, &checked.diagnostics);
                return ExitCode::FAILURE;
            }
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
                resolution.program.declarations.len(),
                if resolution.program.declarations.len() == 1 {
                    ""
                } else {
                    "s"
                },
                typed.program.types.len(),
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            render_diagnostics(&sources, &diagnostics);
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Check,
    Build,
    Run,
}

struct Cli {
    command: CommandKind,
    package_dir: PathBuf,
    target: Target,
    optimization: Optimization,
    output_directory: PathBuf,
    keep_generated_c: bool,
    c_compiler: Option<OsString>,
}

impl Cli {
    fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut command = CommandKind::Check;
        let mut package_dir = None;
        let mut target = Target::host();
        let mut optimization = Optimization::Debug;
        let mut output_directory = None;
        let mut keep_generated_c = false;
        let mut c_compiler = None;
        for (index, argument) in arguments.enumerate() {
            let text = argument.to_string_lossy();
            match text.as_ref() {
                "check" if index == 0 => command = CommandKind::Check,
                "build" if index == 0 => command = CommandKind::Build,
                "run" if index == 0 => command = CommandKind::Run,
                "--release" => optimization = Optimization::Release,
                "--keep-c" => keep_generated_c = true,
                "--target=x86" => target = Target::X86,
                "--target=x86_64" => target = Target::X86_64,
                _ if text.starts_with("--out-dir=") => {
                    output_directory = Some(PathBuf::from(&text["--out-dir=".len()..]));
                }
                _ if text.starts_with("--cc=") => {
                    c_compiler = Some(OsString::from(&text["--cc=".len()..]));
                }
                _ if text.starts_with('-') => {
                    return Err(format!(
                        "unknown option `{text}`\n\
                         usage: elamite [check|build|run] [package] \
                         [--target=x86|--target=x86_64] [--release] [--keep-c] \
                         [--out-dir=PATH] [--cc=PATH]"
                    ));
                }
                _ if package_dir.is_none() => package_dir = Some(PathBuf::from(argument)),
                _ => return Err("only one package path may be supplied".to_string()),
            }
        }
        let package_dir = package_dir.unwrap_or_else(|| PathBuf::from("."));
        let output_directory = output_directory.unwrap_or_else(|| package_dir.join("build"));
        Ok(Self {
            command,
            package_dir,
            target,
            optimization,
            output_directory,
            keep_generated_c,
            c_compiler,
        })
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
