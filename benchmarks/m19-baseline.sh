#!/usr/bin/env bash
set -euo pipefail

compiler="${ELAMC_BIN:-target/release/elamc}"
if [[ ! -x "$compiler" ]]; then
    cargo build --release
fi
if [[ ! -x /usr/bin/time ]]; then
    echo "error: /usr/bin/time is required" >&2
    exit 1
fi

output_root="$(mktemp -d)"
trap 'rm -rf -- "$output_root"' EXIT

echo -e "case\tcompile_seconds\tcompile_peak_kib\tgenerated_c_bytes\tnative_bytes\truntime_seconds\truntime_peak_kib"

measure() {
    local label="$1"
    local package="$2"
    local artifact_name="$3"
    local case_output="$output_root/$label"
    local compile_metrics="$case_output.compile"
    local runtime_metrics="$case_output.runtime"

    mkdir -p "$case_output"
    /usr/bin/time -f '%e\t%M' -o "$compile_metrics" \
        "$compiler" build "$package" --release --keep-c --out-dir "$case_output" \
        >/dev/null
    /usr/bin/time -f '%e\t%M' -o "$runtime_metrics" \
        "$case_output/$artifact_name" >/dev/null

    local compile
    local runtime
    local c_size
    local native_size
    compile="$(cat "$compile_metrics")"
    runtime="$(cat "$runtime_metrics")"
    c_size="$(wc -c < "$case_output/$artifact_name.c")"
    native_size="$(wc -c < "$case_output/$artifact_name")"
    echo -e "$label\t$compile\t$c_size\t$native_size\t$runtime"
}

measure "spec_demo" "examples/spec_demo" "spec_demo"
measure "runtime_stress" "tests/fixtures/conformance/12_runtime_stress" "runtime_stress"
measure "c_ffi" "examples/c_ffi" "c_ffi_demo"
