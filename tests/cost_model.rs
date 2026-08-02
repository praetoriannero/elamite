use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

#[test]
fn checked_in_memory_baseline_tracks_the_fixed_workload_sources() {
    let baseline = fs::read_to_string("benchmarks/memory-cost-baseline.tsv")
        .expect("read checked-in memory baseline");
    assert!(
        baseline.starts_with("# schema=elamite-memory-cost-v1\n"),
        "the baseline schema must be explicit"
    );
    let mut cases = 0;
    for line in baseline.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with("case\t") {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 12, "malformed baseline row: {line}");
        let source = format!("benchmarks/memory-costs/{}.elx", fields[0]);
        assert!(Path::new(&source).is_file(), "missing workload {source}");
        let output = Command::new("sha256sum")
            .arg(&source)
            .output()
            .expect("run sha256sum");
        assert!(output.status.success(), "hash workload {source}");
        let actual = String::from_utf8(output.stdout)
            .expect("UTF-8 sha256sum output")
            .split_whitespace()
            .next()
            .expect("sha256sum emits a digest")
            .to_string();
        assert_eq!(actual, fields[1], "stale baseline for {source}");
        for field in &fields[2..] {
            assert!(
                field.parse::<f64>().is_ok(),
                "baseline metric is not numeric: {field}"
            );
        }
        cases += 1;
    }
    assert_eq!(cases, 6, "every fixed workload needs one baseline row");
}

#[test]
fn memory_baseline_runner_is_executable_and_valid_shell() {
    let script = Path::new("benchmarks/memory-cost-baseline.sh");
    let metadata = fs::metadata(script).expect("read baseline runner metadata");
    assert_ne!(
        metadata.permissions().mode() & 0o111,
        0,
        "baseline runner must be executable"
    );
    let status = Command::new("bash")
        .arg("-n")
        .arg(script)
        .status()
        .expect("validate baseline runner shell");
    assert!(status.success());
}
