#!/bin/bash

# A script to run benches and save their output to the results directory.
# This is useful for comparing the performance of different implementations.
# The script should be run from the root of the lumos crate.

# --- Configuration ---
RESULTS_DIR="benches/results"

# --- Helper Functions ---
function run_bench() {
    local bench_name=$1
    echo "Running bench: $bench_name"
    # Ensure the results directory exists
    mkdir -p "$RESULTS_DIR"
    # Run the benchmark and save the output
    cargo bench --bench "$bench_name" --features="bench" -- --save-baseline "$bench_name" > "$RESULTS_DIR/$bench_name.txt"
}

# --- Main Logic ---
# If arguments are provided, run those specific benches
if [ "$#" -gt 0 ]; then
    for bench in "$@"; do
        run_bench "$bench"
    done
else
    # If no arguments are provided, run all benches
    echo "No specific benchmarks provided. Running all..."

    # Check for jq only when it's needed
    if ! command -v jq &> /dev/null; then
        echo "jq is not installed. Please install it to list and run all benchmarks."
        exit 1
    fi

    all_benches=(
        $(cargo bench --bench '*' --no-run --message-format=json --features="bench" |
        jq -r 'select(.profile.test == true) | .target.name')
    )
    for bench in "${all_benches[@]}"; do
        run_bench "$bench"
    done
fi

echo "Benchmarking complete."
