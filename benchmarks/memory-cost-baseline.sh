#!/usr/bin/env bash
set -euo pipefail

compiler="${ELAMC_BIN:-target/release/elamc}"
target="${ELAMITE_BENCH_TARGET:-x86_64}"
if [[ -z "${ELAMC_BIN:-}" ]]; then
    cargo build --release
elif [[ ! -x "$compiler" ]]; then
    echo "error: ELAMC_BIN is not executable: $compiler" >&2
    exit 1
fi
if [[ ! -x /usr/bin/time ]]; then
    echo "error: /usr/bin/time is required" >&2
    exit 1
fi

output_root="$(mktemp -d)"
trap 'rm -rf -- "$output_root"' EXIT

echo "# schema=elamite-memory-cost-v1"
echo "# recorded_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "# host=$(uname -srmo)"
echo "# rustc=$(rustc --version)"
echo "# cc=$(cc --version | head -n 1)"
echo "# target=$target"
echo -e "case\tsource_sha256\tcompile_seconds\tcompile_peak_kib\truntime_seconds\truntime_peak_kib\tallocations\tallocated_bytes\tscanned_allocations\tscanned_bytes\tmemcpy_calls\tmemcpy_bytes"

measure() {
    local source="$1"
    local label
    local case_output
    local executable
    local compile_metrics
    local runtime_metrics
    local cost_output
    local cost_line
    local costs

    label="$(basename "$source" .elx)"
    case_output="$output_root/$label"
    executable="$case_output/$label"
    compile_metrics="$case_output.compile"
    runtime_metrics="$case_output.runtime"
    cost_output="$case_output.cost"
    mkdir -p "$case_output"

    /usr/bin/time -f '%e\t%M' -o "$compile_metrics" \
        "$compiler" "$source" --release --target="$target" --keep-c \
        --out-dir="$case_output" -o "$executable" \
        --c-flag=-DELAMITE_COST_INSTRUMENTATION=1 >/dev/null
    if ! /usr/bin/time -f '%e\t%M' -o "$runtime_metrics" \
        "$executable" >/dev/null 2>"$cost_output"; then
        cat "$cost_output" >&2
        echo "error: $label did not complete on target $target" >&2
        exit 1
    fi

    cost_line="$(sed -n '/^elamite-cost-v1/p' "$cost_output")"
    if [[ -z "$cost_line" ]]; then
        echo "error: $label produced no cost report" >&2
        exit 1
    fi
    costs="$(printf '%s\n' "$cost_line" | cut -f2- | sed 's/[a-z_]*=//g')"
    echo -e "$label\t$(sha256sum "$source" | cut -d' ' -f1)\t$(cat "$compile_metrics")\t$(cat "$runtime_metrics")\t$costs"
}

for source in benchmarks/memory-costs/*.elx; do
    measure "$source"
done
