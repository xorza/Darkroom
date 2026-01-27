#!/bin/bash

# A script to run benches and save their output to the results directory.
# This is useful for comparing the performance of different implementations.
# The script should be run from the root of the lumos crate.

# --- Configuration ---
RESULTS_DIR="benches/results"

# --- Benchmark Categories ---
# These arrays define benchmarks by category for selective running
STAR_DETECTION_BENCHES=(
    "star_detection_convolution"
    "star_detection_background"
    "star_detection_threshold"
    "star_detection_deblend"
    "star_detection_centroid"
    "star_detection_cosmic_ray"
)

IMAGE_PROCESSING_BENCHES=(
    "demosaic_bayer"
    "demosaic_xtrans"
    "hot_pixels"
    "median_filter"
)

REGISTRATION_BENCHES=(
    "registration"
)

STACKING_BENCHES=(
    "stack_mean"
    "stack_median"
    "stack_sigma_clipped"
)

MATH_BENCHES=(
    "math"
)

PIPELINE_BENCHES=(
    "full_pipeline"
)

# --- Helper Functions ---
function run_bench() {
    local bench_name=$1
    echo "Running bench: $bench_name"
    # Ensure the results directory exists
    mkdir -p "$RESULTS_DIR"
    # Run the benchmark and save the output
    cargo bench --bench "$bench_name" --features="bench" -- --save-baseline "$bench_name" > "$RESULTS_DIR/$bench_name.txt"
}

function run_category() {
    local category=$1
    shift
    local benches=("$@")

    echo "=== Running $category benchmarks ==="
    for bench in "${benches[@]}"; do
        run_bench "$bench"
    done
    echo ""
}

function print_usage() {
    echo "Usage: $0 [category|benchmark_name...]"
    echo ""
    echo "Categories:"
    echo "  star_detection   - Star detection pipeline benchmarks"
    echo "  image_processing - Image processing benchmarks (demosaic, hot pixels, etc.)"
    echo "  registration     - Registration pipeline benchmarks"
    echo "  stacking         - Stacking algorithm benchmarks"
    echo "  math             - Core math operation benchmarks"
    echo "  pipeline         - Full pipeline benchmarks"
    echo "  all              - Run all benchmarks"
    echo ""
    echo "You can also specify individual benchmark names directly."
    echo ""
    echo "Examples:"
    echo "  $0 star_detection           # Run all star detection benchmarks"
    echo "  $0 stacking math            # Run stacking and math benchmarks"
    echo "  $0 star_detection_convolution  # Run a specific benchmark"
}

# --- Main Logic ---
if [ "$#" -eq 0 ]; then
    print_usage
    exit 0
fi

for arg in "$@"; do
    case "$arg" in
        star_detection)
            run_category "Star Detection" "${STAR_DETECTION_BENCHES[@]}"
            ;;
        image_processing)
            run_category "Image Processing" "${IMAGE_PROCESSING_BENCHES[@]}"
            ;;
        registration)
            run_category "Registration" "${REGISTRATION_BENCHES[@]}"
            ;;
        stacking)
            run_category "Stacking" "${STACKING_BENCHES[@]}"
            ;;
        math)
            run_category "Math" "${MATH_BENCHES[@]}"
            ;;
        pipeline)
            run_category "Pipeline" "${PIPELINE_BENCHES[@]}"
            ;;
        all)
            run_category "Star Detection" "${STAR_DETECTION_BENCHES[@]}"
            run_category "Image Processing" "${IMAGE_PROCESSING_BENCHES[@]}"
            run_category "Registration" "${REGISTRATION_BENCHES[@]}"
            run_category "Stacking" "${STACKING_BENCHES[@]}"
            run_category "Math" "${MATH_BENCHES[@]}"
            run_category "Pipeline" "${PIPELINE_BENCHES[@]}"
            ;;
        -h|--help)
            print_usage
            exit 0
            ;;
        *)
            # Assume it's a specific benchmark name
            run_bench "$arg"
            ;;
    esac
done

echo "Benchmarking complete."
