use std::path::Path;
use std::process::Command;

#[test]
fn native_tests_cover_assertions_builtin_and_custom_traps() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["test", "tests/fixtures/package_tests/basic"])
        .output()
        .expect("run native package tests");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test assertions_pass"), "{stdout}");
    assert!(stdout.contains("test bare_return_passes"), "{stdout}");
    assert!(stdout.contains("test builtin_trap_is_expected"), "{stdout}");
    assert!(stdout.contains("test compiler_trap_is_typed"), "{stdout}");
    assert!(
        stdout.contains("test expected_body_state_is_isolated"),
        "{stdout}"
    );
    assert!(stdout.contains("test custom_trap_is_expected"), "{stdout}");
    assert!(stdout.contains("test nested.nested_test"), "{stdout}");
    assert!(!stdout.contains("\u{1b}["), "{stdout}");
}

#[test]
fn production_check_does_not_check_test_bodies() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["check", "tests/fixtures/package_tests/ignored_invalid"])
        .output()
        .expect("check package with an invalid unselected test");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["test", "tests/fixtures/package_tests/ignored_invalid"])
        .output()
        .expect("select invalid test body");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing_name"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn failures_are_isolated_and_do_not_stop_later_tests() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["test", "tests/fixtures/package_tests/failures"])
        .output()
        .expect("run failing package tests");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test assertion_fails"), "{stdout}");
    assert!(
        stdout.contains("test explicit_failure_formats_once"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test custom_failure_uses_display"),
        "{stdout}"
    );
    assert!(stdout.contains("test later_test_still_runs"), "{stdout}");
    assert!(
        stdout.contains("test normal_completion_does_not_match"),
        "{stdout}"
    );
    assert!(stdout.contains("test assertion_does_not_match"), "{stdout}");
    assert!(
        stdout.contains("test a_different_trap_does_not_match"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test equal_codes_from_different_types_do_not_match"),
        "{stdout}"
    );
    assert!(stdout.contains(" ... FAILED"), "{stdout}");
    assert!(stdout.contains(" ... ok"), "{stdout}");
    assert!(stdout.contains("later\n"), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("user failure 7"), "{stderr}");
}

#[test]
fn an_explicit_empty_filter_is_a_command_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args([
            "test",
            "tests/fixtures/package_tests/basic",
            "--filter=no-such-test",
        ])
        .output()
        .expect("run filtered package tests");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("matched no tests"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_filter_works_from_outside_the_package_directory() {
    let package = std::env::current_dir()
        .expect("current directory")
        .join("tests/fixtures/package_tests/basic");
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .current_dir(std::env::temp_dir())
        .arg("test")
        .arg(package)
        .arg("--filter=custom_trap")
        .output()
        .expect("run one filtered package test");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test custom_trap_is_expected"), "{stdout}");
    assert!(stdout.contains("1 passed; 0 failed"), "{stdout}");
    assert!(!stdout.contains("assertions_pass"), "{stdout}");
}

#[test]
fn zero_tests_pass_and_dependency_tests_are_not_discovered() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["test", "tests/fixtures/package_tests/dependency/root"])
        .output()
        .expect("run a root package with only dependency tests");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("0 passed; 0 failed"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn invalid_test_control_and_callability_are_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["test", "tests/fixtures/package_tests/invalid_rules"])
        .output()
        .expect("compile invalid test rules");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("valid only inside a test"), "{stderr}");
    assert!(stderr.contains("not callable"), "{stderr}");
    assert!(
        stderr.contains("cannot contain another `expect`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("must implement `std.testing.RuntimeTrap`"),
        "{stderr}"
    );
    assert!(stderr.contains("postfix `?`"), "{stderr}");
}

#[test]
fn conformance_is_a_distinct_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["conformance", "tests/fixtures/conformance/01_overview"])
        .output()
        .expect("run conformance fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fixture_paths_exist() {
    assert!(Path::new("tests/fixtures/package_tests/basic").is_dir());
}
