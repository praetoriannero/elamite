use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::{COptions, emit_c};
use elamite::check::{check_borrows, check_for_target, check_moves, infer_provenance_signatures};
use elamite::config::{SemanticRevision, Target};
use elamite::diagnostics::Diagnostic;
use elamite::ir::{lower_control_flow, lower_typed_ir};
use elamite::package::PackageGraph;
use elamite::resolution::resolve;
use elamite::source::SourceManager;
use elamite::types::resolve_types;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct OwnedPackage {
    root: PathBuf,
}

impl OwnedPackage {
    fn new(name: &str, source: &str, header: Option<&str>) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-owned-ffi-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create fixture package");
        if let Some(header) = header {
            fs::create_dir_all(root.join("native/include")).expect("create include directory");
            fs::write(root.join("native/include/owned_ffi.h"), header).expect("write C header");
        }
        fs::write(
            root.join("elamite.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n\n[native]\ninclude_paths = [\"native/include\"]\n"
            ),
        )
        .expect("write manifest");
        fs::write(root.join("src/main.elx"), source).expect("write source");
        Self { root }
    }

    fn graph(&self, sources: &mut SourceManager) -> PackageGraph {
        PackageGraph::resolve_with_revision(
            &self.root.join("elamite.toml"),
            sources,
            SemanticRevision::V0_11,
        )
        .expect("resolve package graph")
    }
}

impl Drop for OwnedPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn diagnostics(name: &str, source: &str) -> Vec<Diagnostic> {
    let package = OwnedPackage::new(name, source, None);
    let mut sources = SourceManager::new();
    let resolution = resolve(&package.graph(&mut sources), &mut sources);
    let mut diagnostics = resolution.diagnostics;
    let mut types = resolve_types(&resolution.program);
    diagnostics.append(&mut types.diagnostics);
    diagnostics
        .extend(elamite::traits::check_traits(&resolution.program, &mut types.program).diagnostics);
    diagnostics.extend(check_for_target(&resolution.program, &mut types.program, 64).diagnostics);
    diagnostics
}

fn generated_c(package: &OwnedPackage, entry: Option<&str>, target: Target) -> String {
    let mut sources = SourceManager::new();
    let resolution = resolve(&package.graph(&mut sources), &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types.program, target.pointer_bits());
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types.program);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(typed.diagnostics.is_empty(), "{:#?}", typed.diagnostics);
    let moves = check_moves(&resolved, &mut types.program, &typed.program);
    assert!(moves.is_empty(), "{moves:#?}");
    let borrows = check_borrows(&resolved, &mut types.program, &typed.program);
    assert!(borrows.is_empty(), "{borrows:#?}");
    let control_flow = lower_control_flow(&typed.program, &types.program);
    let entry = entry.map(|name| {
        resolved
            .declarations
            .iter()
            .find(|declaration| resolved.symbol_text(declaration.name) == name)
            .map(|declaration| declaration.id)
            .expect("entry declaration")
    });
    let generated = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target,
            entry,
            test_entries: None,
        },
    );
    assert!(
        generated.diagnostics.is_empty(),
        "{:#?}",
        generated.diagnostics
    );
    generated.source
}

fn compile_and_run(
    root: &Path,
    generated: &str,
    harness: Option<&str>,
    optimization: &str,
) -> Output {
    let generated_path = root.join("program.c");
    fs::write(&generated_path, generated).expect("write generated C");
    let harness_path = harness.map(|harness| {
        let path = root.join("harness.c");
        fs::write(&path, harness).expect("write C harness");
        path
    });
    let executable = root.join("program");
    let mut compiler = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
    compiler.args([
        "-std=c99",
        "-pedantic-errors",
        "-Wall",
        "-Wextra",
        "-Werror",
        optimization,
        "-fsanitize=address,undefined",
        "-fno-omit-frame-pointer",
        "-pthread",
    ]);
    compiler.arg("-I").arg(root.join("native/include"));
    compiler.arg(&generated_path);
    if let Some(path) = harness_path {
        compiler.arg(path);
    }
    compiler.arg("-o").arg(&executable);
    let compiled = compiler.output().expect("run C compiler");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    Command::new(executable)
        // LeakSanitizer cannot inspect child processes under the test runner's
        // ptrace sandbox. ASan/UBSan still cover the native boundary; exact
        // foreign-resource release is asserted by the harness counter.
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run generated program")
}

#[test]
fn owned_foreign_signatures_and_uninitialized_storage_are_checked() {
    for (name, source, expected) in [
        (
            "owning_parameter",
            "@importc(\"bad\", \"bad.h\")\nfn bad(value: String) -> i32\n",
            "ABI-safe",
        ),
        (
            "safe_reference",
            "@exportc(\"bad\")\nfn bad(value: &i32) -> i32:\n    return *value\n",
            "ABI-safe",
        ),
        (
            "non_abi_output",
            "fn main() -> ():\n    let _ = std.ffi.MaybeUninit[String].new()\n",
            "ABI-safe value type",
        ),
        (
            "unchecked_output",
            "fn main() -> ():\n    let output = std.ffi.MaybeUninit[i32].new()\n    let _ = output.assume_init()\n",
            "requires an `unsafe",
        ),
        (
            "legacy_root",
            "fn main() -> ():\n    let value = 1\n    let _ = std.ffi.ForeignRoot[i32].retain(&value)\n",
            "no method named `retain`",
        ),
        (
            "capturing_callback",
            "fn main() -> ():\n    let value = 1\n    let callback = fn[value](input: i32) -> i32:\n        return input + value\n    let _ = callback as *fn(i32) -> i32\n",
            "capturing closure cannot convert",
        ),
    ] {
        let diagnostics = diagnostics(name, source);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{name}: {diagnostics:#?}"
        );
    }
}

#[test]
fn owned_output_transfer_callback_and_foreign_resource_cleanup_are_exact() {
    let header = r#"#ifndef OWNED_FFI_H
#define OWNED_FFI_H
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>

typedef int32_t (*owned_callback)(int32_t);
typedef int32_t (*owned_context_callback)(void *, int32_t);
typedef struct owned_callback_job {
    owned_callback callback;
    int32_t input;
    int32_t output;
} owned_callback_job;

typedef struct owned_resource {
    int32_t value;
} owned_resource;

static int32_t owned_resource_drops = 0;

static int32_t fill_i32(int32_t *output, int32_t value) {
    *output = value;
    return 0;
}

static int32_t *roundtrip_i32(int32_t *value) {
    return value;
}

static int32_t sum_i32(const int32_t *values, uintptr_t length) {
    uintptr_t index;
    int32_t result = 0;
    for (index = 0U; index < length; ++index) result += values[index];
    return result;
}

static int32_t call_with_context(
    owned_context_callback callback,
    void *context,
    int32_t input
) {
    return callback(context, input);
}

static owned_resource *resource_new(int32_t value) {
    owned_resource *resource = (owned_resource *)malloc(sizeof(*resource));
    if (resource != NULL) resource->value = value;
    return resource;
}

static void resource_drop(owned_resource *resource) {
    if (resource != NULL) {
        ++owned_resource_drops;
        free(resource);
    }
}

static int32_t resource_drop_count(void) {
    return owned_resource_drops;
}

static void *owned_callback_entry(void *opaque) {
    owned_callback_job *job = (owned_callback_job *)opaque;
    job->output = job->callback(job->input);
    return NULL;
}

static int32_t call_on_foreign_thread(owned_callback callback, int32_t input) {
    pthread_t thread;
    owned_callback_job job;
    job.callback = callback;
    job.input = input;
    job.output = 0;
    if (pthread_create(&thread, NULL, owned_callback_entry, &job) != 0) return -1;
    if (pthread_join(thread, NULL) != 0) return -2;
    return job.output;
}
#endif
"#;
    let source = r#"@importc("fill_i32", "owned_ffi.h")
fn fill_i32(output: *var i32, value: i32) -> i32

@importc("roundtrip_i32", "owned_ffi.h")
fn roundtrip_i32(value: *var i32) -> *var i32

@importc("sum_i32", "owned_ffi.h")
fn sum_i32(values: *i32, length: usize) -> i32

@importc("call_with_context", "owned_ffi.h")
fn call_with_context(
    callback: *fn(*var std.ffi.CVoid, i32) -> i32,
    context: *var std.ffi.CVoid,
    input: i32,
) -> i32

@importc("owned_resource", "owned_ffi.h")
type NativeResource

@importc("resource_new", "owned_ffi.h")
fn resource_new(value: i32) -> *var NativeResource

@importc("resource_drop", "owned_ffi.h")
fn resource_drop(resource: *var NativeResource) -> ()

@importc("resource_drop_count", "owned_ffi.h")
fn resource_drop_count() -> i32

@importc("call_on_foreign_thread", "owned_ffi.h")
fn call_on_foreign_thread(callback: *fn(i32) -> i32, input: i32) -> i32

struct Resource:
    pointer: *var NativeResource

impl Drop for Resource:
    fn drop(self: &var Self) -> ():
        unsafe:
            resource_drop(self.pointer)

fn add_context(context: *var std.ffi.CVoid, value: i32) -> i32:
    unsafe:
        let state = context as *var i32
        return *state + value

fn main() -> ():
    var output = std.ffi.MaybeUninit[i32].new()
    unsafe:
        let status = fill_i32(output.pointer(), 17)
        if status == 0:
            println(output.assume_init())

    let owner = Box[i32].new(23)
    let transferred = owner.into_raw()
    unsafe:
        let returned = roundtrip_i32(transferred)
        let recovered = Box[i32].from_raw(returned)
        println(*recovered)

    let borrowed = Box[i32].new(6)
    unsafe:
        println(sum_i32(borrowed.pointer(), 1usize))

    unsafe:
        let resource = Resource { pointer: resource_new(5) }
        println(resource_drop_count())
    unsafe:
        println(resource_drop_count())

    let callback = fn[](value: i32) -> i32:
        return value + 1
    unsafe:
        println(call_on_foreign_thread(callback as *fn(i32) -> i32, 41))

    var context = Box[i32].new(8)
    let context_callback: &fn(*var std.ffi.CVoid, i32) -> i32 = add_context
    unsafe:
        println(call_with_context(
            context_callback as *fn(*var std.ffi.CVoid, i32) -> i32,
            context.pointer_var() as *var std.ffi.CVoid,
            34,
        ))
"#;
    let package = OwnedPackage::new("owned_boundary", source, Some(header));
    let generated = generated_c(&package, Some("main"), Target::X86_64);
    assert!(!generated.contains("_Static_assert"));
    assert!(!generated.contains("_Atomic"));
    for optimization in ["-O0", "-O2"] {
        let output = compile_and_run(&package.root, &generated, None, optimization);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "17\n23\n6\n0\n1\n42\n42\n"
        );
    }
}

#[test]
fn a_foreign_created_thread_may_enter_through_an_exported_boundary() {
    let source = include_str!("fixtures/owned_model_design/ffi/callback.elx");
    let harness = include_str!("fixtures/owned_model_design/ffi/callback_harness.c");
    let package = OwnedPackage::new("exported_callback", source, None);
    let generated = generated_c(&package, None, Target::X86_64);
    let output = compile_and_run(&package.root, &generated, Some(harness), "-O2");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn owned_ffi_emission_remains_pointer_width_neutral() {
    let source = r#"@importc("consume_size", "owned_ffi.h")
fn consume_size(value: usize) -> usize

@exportc("produce_size")
fn produce_size(value: usize) -> usize:
    return value
"#;
    let package = OwnedPackage::new("target_width", source, None);
    for target in [Target::X86, Target::X86_64] {
        let generated = generated_c(&package, None, target);
        assert!(generated.contains("uintptr_t"));
        assert!(!generated.contains("uint64_t produce_size"));
        assert!(!generated.contains("_Static_assert"));
    }
}
