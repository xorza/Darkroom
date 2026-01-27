#!/bin/bash

# A script to run benches and save their output to the results directory.
# This is useful for comparing the performance of different implementations.
# The script should be run from the root of the lumos crate.

# --- Configuration ---
RESULTS_DIR="benches/results"

# --- Helper Functions ---
function get_all_benches() {
    cargo bench --bench '*' --no-run --message-format=json --features="bench" |
    jq -r 'select(.profile.test == true) | .target.name'
}

function run_bench() {
    local bench_name=$1
    echo "Running bench: $bench_name"
    # Ensure the results directory exists
    mkdir -p "$RESULTS_DIR"
    # Run the benchmark and save the output
    cargo bench --bench "$bench_name" --features="bench" -- --save-baseline "$bench_name" > "$RESULTS_DIR/$bench_name.txt"
}

# --- Main Logic ---
# Check if jq is installed
if ! command -v jq &> /dev/null; then
    echo "jq is not installed. Please install it to run this script."
    exit 1
fi

# If arguments are provided, run those specific benches
if [ "$#" -gt 0 ]; then
    for bench in "$@"; do
        run_bench "$bench"
    done
else
    # If no arguments are provided, run all benches
    echo "No specific benchmarks provided. Running all..."
    all_benches=$(get_all_benches)
    for bench in $all_benches;
 do
        run_bench "$bench"
    done
fi

echo "Benchmarking complete."
