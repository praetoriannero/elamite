//! Drives the two adversarial regression packages under `tests/fixtures/regression`.
//!
//! Both packages were built by auditing `docs/spec.md` against the implementation.
//! Every observation the runtime package prints is a specified rule, so a
//! change to any line of `expected.stdout` means a specified behavior moved.
//! The `regressions/` directory beside each package holds the minimal
//! reproductions the audit produced; the resolved ones are pinned by
//! `tests/backend.rs` and `tests/check.rs`, and the remainder are pinned here.

use std::process::{Command, Output};

const RUNTIME_PACKAGE: &str = "tests/fixtures/regression/adversarial";
const COMPILE_TIME_PACKAGE: &str = "tests/fixtures/regression/adversarial_macros";

fn elamc(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("run elamc {arguments:?}: {error}"))
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn compile_time_package_checks_cleanly() {
    let output = elamc(&["check", COMPILE_TIME_PACKAGE]);
    assert!(output.status.success(), "{}", stderr_of(&output));
}

#[test]
fn rejected_reproductions_still_produce_their_settled_diagnostics() {
    let cases = [
        (
            "check",
            "static_arithmetic_evidence.elx",
            vec![
                "this statically evident integer operation overflows `i32`",
                "a statically evident shift count is outside the value type's width",
                "statically evident division by zero is invalid",
            ],
        ),
        (
            // Not a defect: `docs/toolchain.md` records the deferred backend.
            "build",
            "int128_support.elx",
            vec![
                "128-bit integer constants are deferred",
                "128-bit values cannot yet be displayed",
            ],
        ),
    ];
    for (command, name, expected) in cases {
        let path = format!("{RUNTIME_PACKAGE}/regressions/{name}");
        let output = elamc(&[command, &path]);
        assert!(
            !output.status.success(),
            "{name} was accepted by `elamc {command}`"
        );
        let stderr = stderr_of(&output);
        for fragment in expected {
            assert!(
                stderr.contains(fragment),
                "{name} missing {fragment:?}\n{stderr}"
            );
        }
    }
}

/// `i128` is admitted by the type system even though the backend defers it.
/// If this ever starts failing, the deferred-lowering note in
/// `docs/toolchain.md` needs to move with it.
#[test]
fn deferred_128_bit_lowering_is_a_backend_limit_not_a_checking_one() {
    let path = format!("{RUNTIME_PACKAGE}/regressions/int128_support.elx");
    let output = elamc(&["check", &path]);
    assert!(output.status.success(), "{}", stderr_of(&output));
}

/// SPEC 12.1 restates SPEC 5's variadic placement rule for compile-time
/// declarations and forbids a variadic derive outright. `tests/parser.rs`
/// covers the ordinary-function half; this pins the compile-time half along
/// with the rest of the admitted-signature contract.
#[test]
fn compile_time_signature_violations_are_all_diagnosed() {
    let path = format!("{COMPILE_TIME_PACKAGE}/regressions/signature_validation.elx");
    let output = elamc(&["check", &path]);
    assert!(
        !output.status.success(),
        "signature violations were accepted"
    );
    let stderr = stderr_of(&output);
    for fragment in [
        "a variadic parameter must be final",
        "only the final compile-time parameter may be variadic",
        "a macro must return an expandable `std.ast` role",
        "compile-time parameters cannot contain references, pointers, functions, or runtime-only types",
        "a derive requires exactly one fixed target parameter",
    ] {
        assert!(stderr.contains(fragment), "missing {fragment:?}\n{stderr}");
    }
}

/// The three attachment and invocation forms from SPEC 12.4–12.6 that the
/// audit found unimplemented. They check cleanly now; a regression here means
/// a stable surface stopped parsing or resolving.
#[test]
fn macro_bodies_are_checked_and_cannot_call_runtime_functions() {
    let directory = std::env::temp_dir().join(format!("elamite-macro-body-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create scratch directory");

    let cases = [
        (
            "body_type_error.elx",
            "macro bad() -> std.ast.Expression:\n    let wrong: i32 = \"not an integer\"\n    let result: std.ast.Expression = quote:\n        1\n    return result\n\nfn main():\n    let value = @bad()\n    println(f\"{value}\")\n",
            "compile-time binding value does not match its annotation",
        ),
        (
            "body_calls_runtime.elx",
            "fn helper() -> i32:\n    return 1\n\nmacro bad() -> std.ast.Expression:\n    let borrowed = helper()\n    let result: std.ast.Expression = quote:\n        1\n    return result\n\nfn main():\n    let value = @bad()\n    println(f\"{value}\")\n",
            "compile-time calls are limited to deterministic `std.ast` intrinsics and value methods",
        ),
    ];
    for (name, source, expected) in cases {
        let path = directory.join(name);
        std::fs::write(&path, source).expect("write scratch source");
        let output = elamc(&["check", path.to_str().expect("UTF-8 path")]);
        assert!(!output.status.success(), "{name} was accepted");
        let stderr = stderr_of(&output);
        assert!(
            stderr.contains(expected),
            "{name} missing {expected:?}\n{stderr}"
        );
    }

    std::fs::remove_dir_all(&directory).ok();
}
