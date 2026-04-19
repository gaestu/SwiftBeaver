#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# Carve Worker Scaling Benchmark
#
# Tests different carve-worker counts against a fixed scan-worker
# count to find the optimal I/O scaling factor for your storage.
#
# Each run creates a timestamped directory:
#   <image_dir>/carve_worker_bench/<RUN_ID>/
#     summary.txt           — human-readable comparison table
#     results.csv           — machine-readable results
#     run_<multiplier>/     — per-run SwiftBeaver output + log
#
# Usage:
#   ./scripts/bench_carve_workers.sh --image /path/to/evidence.E01
#   ./scripts/bench_carve_workers.sh --image images/U63395/U63395.E01 --multipliers "1.0 1.5 2.0 3.0"
#   ./scripts/bench_carve_workers.sh --image evidence.dd --scan-workers 8 --repeat 3
#
# Options:
#   --image         Path to evidence file (required)
#   --multipliers   Space-separated list of carve-worker multipliers (default: "1.0 1.5 2.0 3.0")
#   --scan-workers  Fixed scan worker count (default: nproc)
#   --repeat        Number of repetitions per multiplier (default: 3)
#   --chunk-mib     Chunk size in MiB (default: 64)
#   --warmup        Run one throwaway pass first to warm caches (default: off)
#   --help          Show this help
# ============================================================

# --- Defaults ---
MULTIPLIERS="1.0 1.5 2.0 3.0"
SCAN_WORKERS=""
REPEAT=3
CHUNK_MIB=64
WARMUP=false

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Parse flags ---
IMAGE=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --image=*) IMAGE="${1#--image=}"; shift ;;
        --multipliers) MULTIPLIERS="$2"; shift 2 ;;
        --multipliers=*) MULTIPLIERS="${1#--multipliers=}"; shift ;;
        --scan-workers) SCAN_WORKERS="$2"; shift 2 ;;
        --scan-workers=*) SCAN_WORKERS="${1#--scan-workers=}"; shift ;;
        --repeat) REPEAT="$2"; shift 2 ;;
        --repeat=*) REPEAT="${1#--repeat=}"; shift ;;
        --chunk-mib) CHUNK_MIB="$2"; shift 2 ;;
        --chunk-mib=*) CHUNK_MIB="${1#--chunk-mib=}"; shift ;;
        --warmup) WARMUP=true; shift ;;
        --help|-h)
            sed -n '/^# Usage:/,/^# ====/{ /^# ====/d; s/^# \?//; p }' "$0"
            exit 0
            ;;
        *)
            echo "Unknown flag: $1" >&2
            exit 1
            ;;
    esac
done

if [[ -z "$IMAGE" ]]; then
    echo "ERROR: --image is required" >&2
    echo "Usage: $0 --image <path-to-evidence> [--multipliers \"1.0 1.5 2.0 3.0\"] [--repeat N]" >&2
    exit 1
fi

# Resolve image to absolute path
if [[ ! "$IMAGE" = /* ]]; then
    IMAGE="$REPO_ROOT/$IMAGE"
fi
if [[ ! -f "$IMAGE" ]]; then
    echo "ERROR: Image file not found: $IMAGE" >&2
    exit 1
fi

# --- Build release binary ---
echo ">>> Building release binary..."
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" --quiet
SB_BIN="$REPO_ROOT/target/release/swiftbeaver"
if [[ ! -x "$SB_BIN" ]]; then
    echo "ERROR: Release binary not found at $SB_BIN" >&2
    exit 1
fi

# --- System info ---
NUM_CPUS=$(nproc)
if [[ -z "$SCAN_WORKERS" ]]; then
    SCAN_WORKERS=$NUM_CPUS
fi
TOTAL_MEM_KB=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
TOTAL_MEM_MB=$((TOTAL_MEM_KB / 1024))
IMAGE_DIR="$(dirname "$IMAGE")"
IMAGE_NAME="$(basename "$IMAGE" | sed 's/\.[^.]*$//')"
IMAGE_SIZE=$(stat -c%s "$IMAGE" 2>/dev/null || echo "unknown")

# --- Output directory ---
RUN_ID="$(date +%Y%m%dT%H%M%S)"
OUT_DIR="$IMAGE_DIR/carve_worker_bench/$RUN_ID"
mkdir -p "$OUT_DIR"

SUMMARY="$OUT_DIR/summary.txt"
RESULTS_CSV="$OUT_DIR/results.csv"

# --- Header ---
{
    echo "========================================================"
    echo " Carve Worker Scaling Benchmark"
    echo "========================================================"
    echo " Image:         $IMAGE_NAME ($IMAGE)"
    echo " Image size:    $(numfmt --to=iec-i --suffix=B "$IMAGE_SIZE" 2>/dev/null || echo "${IMAGE_SIZE} bytes")"
    echo " CPUs:          $NUM_CPUS"
    echo " Total RAM:     ${TOTAL_MEM_MB} MiB"
    echo " Scan workers:  $SCAN_WORKERS (fixed)"
    echo " Multipliers:   $MULTIPLIERS"
    echo " Repeats:       $REPEAT per multiplier"
    echo " Chunk size:    ${CHUNK_MIB} MiB"
    echo " Warmup:        $WARMUP"
    echo " Started:       $(date)"
    echo " SwiftBeaver:   $($SB_BIN --version 2>&1 || echo 'unknown')"
    echo "========================================================"
} | tee "$SUMMARY"

# CSV header
echo "multiplier,carve_workers,scan_workers,run,wall_seconds,hits_found,files_carved,throughput_mib_s" > "$RESULTS_CSV"

# --- Helper: extract metrics from SwiftBeaver log ---
extract_metric() {
    local log_file="$1"
    local pattern="$2"
    grep -oP "$pattern" "$log_file" | tail -1 || echo "0"
}

# --- Helper: compute carve workers from multiplier ---
compute_carve_workers() {
    local mult="$1"
    # Use awk for float math, round to nearest integer, minimum 1
    echo "$SCAN_WORKERS $mult" | awk '{v = $1 * $2; v = int(v + 0.5); if (v < 1) v = 1; print v}'
}

# --- Warmup pass ---
if [[ "$WARMUP" == "true" ]]; then
    echo "" | tee -a "$SUMMARY"
    echo ">>> Warmup pass (results discarded)..." | tee -a "$SUMMARY"
    WARMUP_OUT=$(mktemp -d)
    "$SB_BIN" \
        --input "$IMAGE" \
        --output "$WARMUP_OUT" \
        --metadata-backend jsonl \
        --workers "$SCAN_WORKERS" \
        --chunk-size-mib "$CHUNK_MIB" \
        --max-chunks 50 \
        --progress-interval-secs 0 \
        >/dev/null 2>&1 || true
    rm -rf "$WARMUP_OUT"
    echo "    Warmup complete." | tee -a "$SUMMARY"
fi

# --- Drop caches helper ---
drop_caches() {
    if [[ -w /proc/sys/vm/drop_caches ]]; then
        sync
        echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true
        sleep 1
    fi
}

# ==============================================================
# Main benchmark loop
# ==============================================================
echo "" | tee -a "$SUMMARY"
echo ">>> Starting benchmark runs..." | tee -a "$SUMMARY"
echo "" | tee -a "$SUMMARY"

TOTAL_RUNS=0
for mult in $MULTIPLIERS; do
    TOTAL_RUNS=$(( TOTAL_RUNS + REPEAT ))
done
CURRENT_RUN=0

for mult in $MULTIPLIERS; do
    CW=$(compute_carve_workers "$mult")
    echo "--- Multiplier ${mult}x → carve_workers=$CW, scan_workers=$SCAN_WORKERS ---" | tee -a "$SUMMARY"

    for ((i = 1; i <= REPEAT; i++)); do
        CURRENT_RUN=$((CURRENT_RUN + 1))
        RUN_DIR="$OUT_DIR/run_${mult}_r${i}"
        RUN_OUT="$RUN_DIR/output"
        RUN_LOG="$RUN_DIR/swiftbeaver.log"
        mkdir -p "$RUN_OUT"

        echo "  [$CURRENT_RUN/$TOTAL_RUNS] ${mult}x run $i/$REPEAT (cw=$CW, sw=$SCAN_WORKERS)..." | tee -a "$SUMMARY"

        # Drop page cache between runs for fair comparison
        drop_caches

        # Time the run
        START_TS=$(date +%s%N)

        "$SB_BIN" \
            --input "$IMAGE" \
            --output "$RUN_OUT" \
            --metadata-backend jsonl \
            --scan-workers "$SCAN_WORKERS" \
            --carve-workers "$CW" \
            --chunk-size-mib "$CHUNK_MIB" \
            --progress-interval-secs 0 \
            >"$RUN_LOG" 2>&1 || true

        END_TS=$(date +%s%N)
        ELAPSED_NS=$((END_TS - START_TS))
        ELAPSED_S=$(echo "$ELAPSED_NS" | awk '{printf "%.2f", $1 / 1000000000}')

        # Extract stats from log
        HITS=$(extract_metric "$RUN_LOG" 'hits=\K[0-9]+')
        FILES=$(extract_metric "$RUN_LOG" 'files=\K[0-9]+')

        # Compute throughput
        THROUGHPUT="0.00"
        if [[ -n "$ELAPSED_S" ]] && (( $(echo "$ELAPSED_S > 0" | bc -l) )); then
            THROUGHPUT=$(echo "$IMAGE_SIZE $ELAPSED_S" | awk '{printf "%.2f", ($1 / 1048576) / $2}')
        fi

        echo "    → ${ELAPSED_S}s, ${HITS} hits, ${FILES} files, ${THROUGHPUT} MiB/s" | tee -a "$SUMMARY"

        # Append to CSV
        echo "${mult},${CW},${SCAN_WORKERS},${i},${ELAPSED_S},${HITS},${FILES},${THROUGHPUT}" >> "$RESULTS_CSV"

        # Clean up carved files to save disk space (keep metadata + log)
        rm -rf "$RUN_OUT"/*/carved 2>/dev/null || true
    done

    echo "" | tee -a "$SUMMARY"
done

# ==============================================================
# Summary statistics
# ==============================================================
echo "========================================================" | tee -a "$SUMMARY"
echo " Results Summary (averaged over $REPEAT runs)"           | tee -a "$SUMMARY"
echo "========================================================" | tee -a "$SUMMARY"
printf "%-12s %-14s %-14s %-14s %-14s %-14s\n" \
    "Multiplier" "Carve Workers" "Avg Time (s)" "Avg MiB/s" "Avg Hits" "Avg Files" | tee -a "$SUMMARY"
printf "%-12s %-14s %-14s %-14s %-14s %-14s\n" \
    "----------" "-------------" "------------" "---------" "--------" "---------" | tee -a "$SUMMARY"

# Compute averages per multiplier from CSV
for mult in $MULTIPLIERS; do
    CW=$(compute_carve_workers "$mult")
    # Skip header, filter by multiplier, compute averages
    AVG_LINE=$(awk -F',' -v m="$mult" '
        NR > 1 && $1 == m {
            sum_time += $5
            sum_tp   += $8
            sum_hits += $6
            sum_files += $7
            n++
        }
        END {
            if (n > 0) {
                printf "%.2f %.2f %.0f %.0f", sum_time/n, sum_tp/n, sum_hits/n, sum_files/n
            } else {
                printf "0 0 0 0"
            }
        }
    ' "$RESULTS_CSV")
    read -r AVG_TIME AVG_TP AVG_HITS AVG_FILES <<< "$AVG_LINE"
    printf "%-12s %-14s %-14s %-14s %-14s %-14s\n" \
        "${mult}x" "$CW" "$AVG_TIME" "$AVG_TP" "$AVG_HITS" "$AVG_FILES" | tee -a "$SUMMARY"
done

# Compute speedup relative to 1.0x baseline
echo "" | tee -a "$SUMMARY"
BASELINE_TP=$(awk -F',' 'NR > 1 && $1 == "1.0" { sum += $8; n++ } END { if (n>0) printf "%.2f", sum/n; else print "0" }' "$RESULTS_CSV")
if (( $(echo "$BASELINE_TP > 0" | bc -l) )); then
    echo "Speedup relative to 1.0x baseline ($BASELINE_TP MiB/s):" | tee -a "$SUMMARY"
    for mult in $MULTIPLIERS; do
        MULT_TP=$(awk -F',' -v m="$mult" 'NR > 1 && $1 == m { sum += $8; n++ } END { if (n>0) printf "%.2f", sum/n; else print "0" }' "$RESULTS_CSV")
        SPEEDUP=$(echo "$MULT_TP $BASELINE_TP" | awk '{printf "%.2f", $1 / $2}')
        echo "  ${mult}x: ${SPEEDUP}x throughput (${MULT_TP} MiB/s)" | tee -a "$SUMMARY"
    done
fi

echo "" | tee -a "$SUMMARY"
echo "Finished: $(date)" | tee -a "$SUMMARY"
echo "Results: $OUT_DIR" | tee -a "$SUMMARY"
echo "CSV:     $RESULTS_CSV" | tee -a "$SUMMARY"

# Symlink latest bench for convenience
ln -sfn "carve_worker_bench/$RUN_ID" "$IMAGE_DIR/latest_carve_bench"

echo ""
echo "Done. View results: cat $SUMMARY"
