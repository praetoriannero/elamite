//! Milestone 8 package-to-C and native-artifact driver.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::backend::{COptions, emit_c};
use crate::check::check_for_target;
pub use crate::config::Optimization;
use crate::config::Target;
use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{ControlFlowProgram, TypedIrProgram, lower_control_flow, lower_typed_ir};
use crate::manifest::TargetKind;
use crate::package::PackageGraph;
use crate::resolution::{DeclarationId, DeclarationKind, ResolvedProgram, resolve};
use crate::source::SourceManager;
use crate::types::{TypedProgram, resolve_types};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpStage {
    Tokens,
    Syntax,
    Resolution,
    Types,
    TypedIr,
    ControlFlow,
    Monomorphized,
    GeneratedC,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub target: Target,
    pub optimization: Optimization,
    pub output_directory: PathBuf,
    pub keep_generated_c: bool,
    pub c_compiler: Option<OsString>,
    /// Extra hardening or instrumentation flags. Driver-owned C99 and warning
    /// flags remain present in every invocation.
    pub c_flags: Vec<OsString>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            target: Target::host(),
            optimization: Optimization::Debug,
            output_directory: PathBuf::from("build"),
            keep_generated_c: false,
            c_compiler: None,
            c_flags: Vec::new(),
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
    pub metadata_path: PathBuf,
    pub dependency_metadata_paths: Vec<PathBuf>,
}

pub struct RunResult {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TestCase {
    pub declaration: DeclarationId,
    pub name: String,
}

pub struct TestBuild {
    pub artifact: BuildArtifact,
    pub cases: Vec<TestCase>,
}

/// Frontend result for `check`: everything needed to report diagnostics or
/// summarize a package, without lowering or invoking a toolchain.
pub struct FrontendOutput {
    pub resolved: ResolvedProgram,
    pub typed: TypedProgram,
    pub checked: crate::check::CheckedProgram,
}

/// Runs resolution, canonical typing, trait checking, and body checking.
///
/// `check` and `build` share this so the two commands cannot diverge: a
/// package that checks clean is one whose frontend `build` will also accept.
pub fn check_frontend(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    target: Target,
) -> Result<FrontendOutput, Vec<Diagnostic>> {
    let resolution = resolve(graph, sources);
    if !resolution.diagnostics.is_empty() {
        return Err(resolution.diagnostics);
    }
    let resolved = resolution.program;
    let mut type_output = resolve_types(&resolved);
    if !type_output.diagnostics.is_empty() {
        return Err(type_output.diagnostics);
    }
    let trait_output = crate::traits::check_traits(&resolved, &mut type_output.program);
    if !trait_output.diagnostics.is_empty() {
        return Err(trait_output.diagnostics);
    }
    let checked = check_for_target(&resolved, &mut type_output.program, target.pointer_bits());
    if !checked.diagnostics.is_empty() {
        return Err(checked.diagnostics);
    }
    Ok(FrontendOutput {
        resolved,
        typed: type_output.program,
        checked: checked.program,
    })
}

/// Produces one deterministic intermediate representation without writing
/// build artifacts or invoking the C compiler.
pub fn dump(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    target: Target,
    stage: DumpStage,
) -> Result<String, Vec<Diagnostic>> {
    match stage {
        DumpStage::Tokens | DumpStage::Syntax => dump_source_stage(graph, sources, stage),
        DumpStage::Resolution => {
            let output = resolve(graph, sources);
            if output.diagnostics.is_empty() {
                Ok(format!(
                    "{}{}",
                    source_identity_table(sources),
                    output.program.dump()
                ))
            } else {
                Err(output.diagnostics)
            }
        }
        DumpStage::Types => {
            let frontend = check_frontend(graph, sources, target)?;
            Ok(format!(
                "{}{}",
                source_identity_table(sources),
                frontend.typed.dump()
            ))
        }
        DumpStage::TypedIr
        | DumpStage::ControlFlow
        | DumpStage::Monomorphized
        | DumpStage::GeneratedC => {
            let compilation = compile(graph, sources, target)?;
            let body = match stage {
                DumpStage::TypedIr => compilation.high_level_ir.dump(),
                DumpStage::ControlFlow => compilation.control_flow_ir.dump(),
                DumpStage::Monomorphized => compilation.control_flow_ir.monomorphization_dump(),
                DumpStage::GeneratedC => compilation.generated_c,
                DumpStage::Tokens
                | DumpStage::Syntax
                | DumpStage::Resolution
                | DumpStage::Types => unreachable!("handled above"),
            };
            Ok(format!("{}{}", source_identity_table(sources), body))
        }
    }
}

fn dump_source_stage(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    stage: DumpStage,
) -> Result<String, Vec<Diagnostic>> {
    let parsed = crate::parsed::parse_user_package(graph, sources);
    let mut output = String::new();
    for unit in parsed.package.units {
        output.push_str(&format!(
            "file {} {}\n",
            unit.file.index(),
            unit.path.display()
        ));
        match stage {
            DumpStage::Tokens => {
                for token in unit.tokens {
                    let position = sources.line_col(token.span.file, token.span.start);
                    output.push_str(&format!(
                        "  {:?} @ {}:{}:{} {}..{}\n",
                        token.kind,
                        unit.path.display(),
                        position.line,
                        position.column,
                        token.span.start,
                        token.span.end
                    ));
                }
            }
            DumpStage::Syntax => {
                for line in unit.tree.dump().lines() {
                    output.push_str("  ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
            _ => unreachable!("only token and syntax stages use source dumping"),
        }
    }
    if parsed.diagnostics.is_empty() {
        Ok(output)
    } else {
        Err(parsed.diagnostics)
    }
}

fn source_identity_table(sources: &SourceManager) -> String {
    let mut output = String::new();
    for (file, path) in sources.files() {
        output.push_str(&format!("source {} {}\n", file.index(), path.display()));
    }
    output
}

/// Runs the complete Milestone 8 frontend and lowering pipeline without
/// writing files or invoking an external toolchain.
pub fn compile(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    target: Target,
) -> Result<Compilation, Vec<Diagnostic>> {
    let frontend = check_frontend(graph, sources, target)?;
    let resolved = frontend.resolved;
    let mut typed = frontend.typed;
    let checked = frontend.checked;
    let high_level = lower_typed_ir(&resolved, &mut typed, &checked);
    if !high_level.diagnostics.is_empty() {
        return Err(high_level.diagnostics);
    }
    let control_flow = lower_control_flow(&high_level.program, &typed);
    let root = &graph.packages[&graph.root];
    let entry = if root.manifest.target_kind == TargetKind::Executable {
        Some(find_entry(&resolved, graph)?)
    } else {
        None
    };
    let c_output = emit_c(
        &control_flow,
        &resolved,
        &typed,
        sources,
        &COptions {
            target,
            entry,
            test_entries: None,
        },
    );
    if !c_output.diagnostics.is_empty() {
        return Err(c_output.diagnostics);
    }
    Ok(Compilation {
        resolved,
        typed,
        high_level_ir: high_level.program,
        control_flow_ir: control_flow,
        generated_c: c_output.source,
        entry,
        native_libraries: c_output.native_libraries,
    })
}

/// Compiles test bodies owned by the selected root package and emits a
/// test-only entry shim. Production compilation continues to use [`compile`].
pub fn compile_tests(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    target: Target,
) -> Result<(Compilation, Vec<TestCase>), Vec<Diagnostic>> {
    let resolution = crate::resolution::resolve_for_tests(graph, sources);
    if !resolution.diagnostics.is_empty() {
        return Err(resolution.diagnostics);
    }
    let resolved = resolution.program;
    let mut type_output = resolve_types(&resolved);
    if !type_output.diagnostics.is_empty() {
        return Err(type_output.diagnostics);
    }
    let trait_output = crate::traits::check_traits(&resolved, &mut type_output.program);
    if !trait_output.diagnostics.is_empty() {
        return Err(trait_output.diagnostics);
    }
    let checked = check_for_target(&resolved, &mut type_output.program, target.pointer_bits());
    if !checked.diagnostics.is_empty() {
        return Err(checked.diagnostics);
    }
    let mut typed = type_output.program;
    let high_level = lower_typed_ir(&resolved, &mut typed, &checked.program);
    if !high_level.diagnostics.is_empty() {
        return Err(high_level.diagnostics);
    }
    let control_flow = lower_control_flow(&high_level.program, &typed);
    let mut cases = resolved
        .declarations
        .iter()
        .filter(|declaration| {
            declaration.kind == DeclarationKind::Test && declaration.test_selected
        })
        .map(|declaration| {
            let module = &resolved.modules[declaration.module.index()];
            let mut components = module
                .path
                .iter()
                .map(|component| resolved.symbol_text(*component).to_string())
                .collect::<Vec<_>>();
            components.push(resolved.symbol_text(declaration.name).to_string());
            TestCase {
                declaration: declaration.id,
                name: components.join("."),
            }
        })
        .collect::<Vec<_>>();
    cases.sort_by(|left, right| left.name.cmp(&right.name));
    let c_output = emit_c(
        &control_flow,
        &resolved,
        &typed,
        sources,
        &COptions {
            target,
            entry: None,
            test_entries: Some(
                cases
                    .iter()
                    .map(|case| (case.declaration, case.name.clone()))
                    .collect(),
            ),
        },
    );
    if !c_output.diagnostics.is_empty() {
        return Err(c_output.diagnostics);
    }
    Ok((
        Compilation {
            resolved,
            typed,
            high_level_ir: high_level.program,
            control_flow_ir: control_flow,
            generated_c: c_output.source,
            entry: None,
            native_libraries: c_output.native_libraries,
        },
        cases,
    ))
}

/// Generates C, invokes the selected C99 compiler, and produces an executable
/// or a relocatable object for a library package.
pub fn build(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    options: &BuildOptions,
) -> Result<BuildArtifact, Vec<Diagnostic>> {
    let compilation = compile(graph, sources, options.target)?;
    build_compilation(graph, sources, options, compilation, false)
}

/// Builds one executable containing the selected package's test declarations.
pub fn build_tests(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    options: &BuildOptions,
) -> Result<TestBuild, Vec<Diagnostic>> {
    let (compilation, cases) = compile_tests(graph, sources, options.target)?;
    let artifact = build_compilation(graph, sources, options, compilation, true)?;
    Ok(TestBuild { artifact, cases })
}

fn build_compilation(
    graph: &PackageGraph,
    sources: &SourceManager,
    options: &BuildOptions,
    compilation: Compilation,
    force_executable: bool,
) -> Result<BuildArtifact, Vec<Diagnostic>> {
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

    let artifact_path = match (root.manifest.target_kind, force_executable) {
        (_, true) | (TargetKind::Executable, false) => options.output_directory.join(&stem),
        (TargetKind::Library, false) => options.output_directory.join(format!("{stem}.o")),
    };
    let (metadata, metadata_path, dependency_metadata_paths) =
        materialize_package_metadata(graph, &compilation.resolved, sources, options, &stem)?;
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
    command.args(&options.c_flags);
    if root.manifest.target_kind == TargetKind::Library && !force_executable {
        command.arg("-c");
    }
    for package in &metadata {
        for path in &package.include_paths {
            command.arg("-I").arg(path);
        }
        for path in &package.library_paths {
            command.arg("-L").arg(path);
        }
    }
    command.arg(&c_path).arg("-o").arg(&artifact_path);
    if root.manifest.target_kind == TargetKind::Executable || force_executable {
        for package in &metadata {
            for library in &package.native_libraries {
                command.arg(format!("-l{library}"));
            }
            command.args(&package.link_options);
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
        metadata_path,
        dependency_metadata_paths,
    })
}

fn materialize_package_metadata(
    graph: &PackageGraph,
    resolved: &ResolvedProgram,
    sources: &SourceManager,
    options: &BuildOptions,
    root_stem: &str,
) -> Result<(Vec<crate::artifact::PackageMetadata>, PathBuf, Vec<PathBuf>), Vec<Diagnostic>> {
    let dependency_directory = options.output_directory.join("deps");
    std::fs::create_dir_all(&dependency_directory).map_err(|error| {
        vec![Diagnostic::new(
            Category::Toolchain,
            format!(
                "cannot create dependency metadata directory {}: {error}",
                dependency_directory.display()
            ),
        )]
    })?;

    let root_path = options
        .output_directory
        .join(format!("{root_stem}.elamite-meta"));
    let mut dependency_paths = Vec::new();
    let mut consumed = Vec::new();
    for (index, package_id) in graph.dependency_order().into_iter().enumerate() {
        let package = &graph.packages[package_id];
        let path = if package_id == &graph.root {
            root_path.clone()
        } else {
            let stem = sanitize_file_name(&package.manifest.name);
            let path = dependency_directory.join(format!("{index:04}-{stem}.elamite-meta"));
            dependency_paths.push(path.clone());
            path
        };
        let metadata =
            crate::artifact::PackageMetadata::collect(graph, resolved, sources, package_id);
        metadata
            .write(&path)
            .map_err(|error| vec![Diagnostic::new(Category::Toolchain, error)])?;
        // Consume the serialized boundary immediately. This catches schema
        // drift and ensures native inputs come from the same public artifact a
        // downstream compiler invocation would read, rather than a parallel
        // manifest-only path.
        consumed.push(
            crate::artifact::PackageMetadata::read(&path)
                .map_err(|error| vec![Diagnostic::new(Category::Toolchain, error)])?,
        );
    }
    Ok((consumed, root_path, dependency_paths))
}

pub fn run(artifact: &BuildArtifact) -> Result<RunResult, Diagnostic> {
    run_with_environment(artifact, &[])
}

/// Runs an artifact with explicit environment overrides. Conformance uses
/// this for sanitizer runtime configuration without mutating the compiler
/// process's global environment.
pub fn run_with_environment(
    artifact: &BuildArtifact,
    environment: &[(OsString, OsString)],
) -> Result<RunResult, Diagnostic> {
    let output = Command::new(&artifact.path)
        .envs(environment.iter().cloned())
        .output()
        .map_err(|error| {
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
