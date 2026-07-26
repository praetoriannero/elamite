//! Milestone 8 package-to-C and native-artifact driver.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::backend::{COptions, Target, emit_c};
use crate::check::check_for_target;
use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{ControlFlowProgram, TypedIrProgram, lower_control_flow, lower_typed_ir};
use crate::manifest::TargetKind;
use crate::package::PackageGraph;
use crate::resolution::{DeclarationId, DeclarationKind, ResolvedProgram, resolve};
use crate::source::SourceManager;
use crate::types::{TypedProgram, resolve_types};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optimization {
    Debug,
    Release,
}

impl Optimization {
    fn compiler_flag(self) -> &'static str {
        match self {
            Self::Debug => "-O0",
            Self::Release => "-O2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub target: Target,
    pub optimization: Optimization,
    pub output_directory: PathBuf,
    pub keep_generated_c: bool,
    pub c_compiler: Option<OsString>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            target: Target::host(),
            optimization: Optimization::Debug,
            output_directory: PathBuf::from("build"),
            keep_generated_c: false,
            c_compiler: None,
        }
    }
}

pub struct Compilation {
    pub resolved: ResolvedProgram,
    pub typed: TypedProgram,
    pub high_level_ir: TypedIrProgram,
    pub control_flow_ir: ControlFlowProgram,
    pub generated_c: String,
    pub entry: Option<DeclarationId>,
    /// Native libraries the generated unit requires, contributed by the
    /// managed-memory strategy. Empty unless lowering produced managed storage.
    pub native_libraries: Vec<String>,
}

pub struct BuildArtifact {
    pub path: PathBuf,
    pub generated_c_path: Option<PathBuf>,
}

pub struct RunResult {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs the complete Milestone 8 frontend and lowering pipeline without
/// writing files or invoking an external toolchain.
pub fn compile(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    target: Target,
) -> Result<Compilation, Vec<Diagnostic>> {
    let resolution = resolve(graph, sources);
    if !resolution.diagnostics.is_empty() {
        return Err(resolution.diagnostics);
    }
    let resolved = resolution.program;
    let mut type_output = resolve_types(&resolved);
    if !type_output.diagnostics.is_empty() {
        return Err(type_output.diagnostics);
    }
    let checked = check_for_target(&resolved, &mut type_output.program, target.pointer_bits());
    if !checked.diagnostics.is_empty() {
        return Err(checked.diagnostics);
    }
    let high_level = lower_typed_ir(&resolved, &type_output.program, &checked.program);
    if !high_level.diagnostics.is_empty() {
        return Err(high_level.diagnostics);
    }
    let control_flow = lower_control_flow(&high_level.program, &type_output.program);
    let root = &graph.packages[&graph.root];
    let entry = if root.manifest.target_kind == TargetKind::Executable {
        Some(find_entry(&resolved, graph)?)
    } else {
        None
    };
    let c_output = emit_c(
        &control_flow,
        &resolved,
        &type_output.program,
        sources,
        &COptions { target, entry },
    );
    if !c_output.diagnostics.is_empty() {
        return Err(c_output.diagnostics);
    }
    Ok(Compilation {
        resolved,
        typed: type_output.program,
        high_level_ir: high_level.program,
        control_flow_ir: control_flow,
        generated_c: c_output.source,
        entry,
        native_libraries: c_output.native_libraries,
    })
}

/// Generates C, invokes the selected C99 compiler, and produces an executable
/// or a relocatable object for a library package.
pub fn build(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    options: &BuildOptions,
) -> Result<BuildArtifact, Vec<Diagnostic>> {
    let compilation = compile(graph, sources, options.target)?;
    let root = &graph.packages[&graph.root];
    std::fs::create_dir_all(&options.output_directory).map_err(|error| {
        vec![Diagnostic::new(
            Category::Toolchain,
            format!(
                "cannot create output directory {}: {error}",
                options.output_directory.display()
            ),
        )]
    })?;

    let stem = sanitize_file_name(&root.manifest.name);
    let c_path = options.output_directory.join(format!("{stem}.c"));
    std::fs::write(&c_path, &compilation.generated_c).map_err(|error| {
        vec![Diagnostic::new(
            Category::Toolchain,
            format!("cannot write generated C {}: {error}", c_path.display()),
        )]
    })?;

    let artifact_path = match root.manifest.target_kind {
        TargetKind::Executable => options.output_directory.join(&stem),
        TargetKind::Library => options.output_directory.join(format!("{stem}.o")),
    };
    let compiler = options
        .c_compiler
        .clone()
        .or_else(|| std::env::var_os("CC"))
        .unwrap_or_else(|| OsString::from("cc"));
    let mut command = Command::new(&compiler);
    command
        .arg("-std=c99")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg(options.optimization.compiler_flag())
        .arg(options.target.compiler_flag());
    if root.manifest.target_kind == TargetKind::Library {
        command.arg("-c");
    }
    command.arg(&c_path).arg("-o").arg(&artifact_path);
    if root.manifest.target_kind == TargetKind::Executable {
        for package in graph.packages.values() {
            for library in &package.manifest.native_libraries {
                command.arg(format!("-l{library}"));
            }
            command.args(&package.manifest.link_options);
        }
        // Runtime libraries follow the manifest's own link inputs so a
        // dependency that references the collector still resolves.
        for library in &compilation.native_libraries {
            command.arg(format!("-l{library}"));
        }
    }
    let output = command.output().map_err(|error| {
        vec![Diagnostic::new(
            Category::Toolchain,
            format!(
                "failed to execute C compiler `{}`: {error}; generated C retained at {}",
                Path::new(&compiler).display(),
                c_path.display()
            ),
        )]
    })?;
    if !output.status.success() {
        return Err(vec![tool_failure(
            "C compiler or linker",
            &output,
            &c_path,
            &compilation.native_libraries,
        )]);
    }
    if !artifact_path.is_file() {
        return Err(vec![Diagnostic::new(
            Category::Toolchain,
            format!(
                "C compiler or linker reported success but did not create {}; generated C \
                 retained at {}",
                artifact_path.display(),
                c_path.display()
            ),
        )]);
    }

    let generated_c_path = if options.keep_generated_c {
        Some(c_path)
    } else {
        let _ = std::fs::remove_file(&c_path);
        None
    };
    Ok(BuildArtifact {
        path: artifact_path,
        generated_c_path,
    })
}

pub fn run(artifact: &BuildArtifact) -> Result<RunResult, Diagnostic> {
    let output = Command::new(&artifact.path).output().map_err(|error| {
        Diagnostic::new(
            Category::Toolchain,
            format!("cannot execute {}: {error}", artifact.path.display()),
        )
    })?;
    Ok(RunResult {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn find_entry(
    resolved: &ResolvedProgram,
    graph: &PackageGraph,
) -> Result<DeclarationId, Vec<Diagnostic>> {
    let root_module = resolved
        .root_module(&graph.root)
        .expect("resolved root package has a root module");
    let entries = resolved
        .declarations
        .iter()
        .filter(|declaration| {
            declaration.module == root_module
                && declaration.parent_declaration.is_none()
                && declaration.parent_impl.is_none()
                && declaration.kind == DeclarationKind::Function
                && resolved.symbol_text(declaration.name) == "main"
        })
        .collect::<Vec<_>>();
    match entries.as_slice() {
        [entry] => Ok(entry.id),
        [] => Err(vec![Diagnostic::new(
            Category::CodeGeneration,
            "an executable package requires `fn main() -> ()` in its root module",
        )]),
        _ => unreachable!("Milestone 4 rejects duplicate module declarations"),
    }
}

fn sanitize_file_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "elamite-output".to_string()
    } else {
        sanitized
    }
}

fn tool_failure(
    tool: &str,
    output: &Output,
    c_path: &Path,
    native_libraries: &[String],
) -> Diagnostic {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let details = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    // A missing collector development package is the likeliest cause once a
    // program needs managed storage, and the raw C error for it is opaque.
    let hint = if native_libraries.is_empty() {
        String::new()
    } else {
        format!(
            "\nthis program needs managed storage and links {}; \
             ensure the development package providing its headers and link \
             archive is installed",
            native_libraries
                .iter()
                .map(|library| format!("-l{library}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    Diagnostic::new(
        Category::Toolchain,
        format!(
            "{tool} failed with status {}; generated C retained at {}{}{hint}",
            output.status,
            c_path.display(),
            if details.is_empty() {
                String::new()
            } else {
                format!(":\n{details}")
            }
        ),
    )
}
