#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# Comparison Benchmark: SwiftBeaver vs bulk_extractor
#
# Runs both tools on the same evidence image and produces a
# normalized comparison of runtime, output counts, and throughput.
#
# Each run creates a timestamped subdirectory:
#   <image_dir>/comparisons/<RUN_ID>/
#     summary.txt         — human-readable comparison
#     comparison.json     — machine-readable results
#     sb_log.txt          — SwiftBeaver stdout/stderr
#     be_log.txt          — bulk_extractor stdout/stderr
#
# Usage:
#   ./scripts/run_comparison.sh --image images/U63395/U63395.E01
#   ./scripts/run_comparison.sh --image /path/to/evidence.E01 --sb-only
#   ./scripts/run_comparison.sh --image images/hackcase/4Dell\ Latitude\ CPi.E01 --be-only
# ============================================================

# --- Parse flags ---
RUN_SB=true
RUN_BE=true
IMAGE=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --sb-only) RUN_BE=false; shift ;;
        --be-only) RUN_SB=false; shift ;;
        --image) IMAGE="$2"; shift 2 ;;
        --image=*) IMAGE="${1#--image=}"; shift ;;
        --help|-h)
            echo "Usage: $0 --image <path-to-E01> [--sb-only|--be-only]"
            echo "  --image     Path to EWF evidence file (relative to repo root or absolute)"
            echo "  --sb-only   Run SwiftBeaver only"
            echo "  --be-only   Run bulk_extractor only"
            exit 0
            ;;
        *)
            echo "Unknown flag: $1"
            exit 1
            ;;
    esac
done

# --- Resolve paths ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [[ -z "$IMAGE" ]]; then
    echo "ERROR: --image is required"
    echo "Usage: $0 --image <path-to-E01> [--sb-only|--be-only]"
    exit 1
fi

# Resolve image to absolute path
if [[ ! "$IMAGE" = /* ]]; then
    IMAGE="$REPO_ROOT/$IMAGE"
fi
if [[ ! -f "$IMAGE" ]]; then
    echo "ERROR: Image file not found: $IMAGE"
    exit 1
fi

# Derive output directory from image location
IMAGE_DIR="$(dirname "$IMAGE")"
IMAGE_NAME="$(basename "$IMAGE" | sed 's/\.[^.]*$//')"

# Unique run directory
RUN_ID="$(date +%Y%m%dT%H%M%S)"
OUT_DIR="$IMAGE_DIR/comparisons/$RUN_ID"
mkdir -p "$OUT_DIR"

SB_OUT="$OUT_DIR/swiftbeaver_output"
BE_OUT="$OUT_DIR/bulk_extractor_output"
SUMMARY="$OUT_DIR/summary.txt"
JSON_OUT="$OUT_DIR/comparison.json"

# --- Detect system info ---
NUM_CPUS=$(nproc)
TOTAL_MEM_KB=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
TOTAL_MEM_MB=$((TOTAL_MEM_KB / 1024))

# --- Evidence info ---
DISK_SIZE=$(ewfinfo "$IMAGE" 2>/dev/null | grep "Media size" || echo "unknown")

# --- Check tool availability ---
SB_BIN="$REPO_ROOT/target/release/swiftbeaver"
if $RUN_SB && [[ ! -x "$SB_BIN" ]]; then
    echo "ERROR: SwiftBeaver binary not found at $SB_BIN"
    echo "       Run: cargo build --release"
    exit 1
fi
if $RUN_BE && ! command -v bulk_extractor &>/dev/null; then
    echo "ERROR: bulk_extractor not found in PATH"
    exit 1
fi

echo "========================================" | tee "$SUMMARY"
echo " Comparison Benchmark"                   | tee -a "$SUMMARY"
echo " Image: $IMAGE_NAME ($IMAGE)"         | tee -a "$SUMMARY"
echo " Evidence: $DISK_SIZE"                   | tee -a "$SUMMARY"
echo " CPUs: $NUM_CPUS  RAM: ${TOTAL_MEM_MB} MiB" | tee -a "$SUMMARY"
echo " Run ID: $RUN_ID"                        | tee -a "$SUMMARY"
echo " Tools: $($RUN_SB && echo "SwiftBeaver") $($RUN_BE && echo "bulk_extractor")" | tee -a "$SUMMARY"
echo " Started: $(date)"                       | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

# ============================================================
# Helper: count lines in a file (0 if missing)
# ============================================================
count_lines() {
    local f="$1"
    if [[ -f "$f" ]]; then
        wc -l < "$f" | tr -d ' '
    else
        echo "0"
    fi
}

# ============================================================
# Helper: count files in a directory (0 if missing)
# ============================================================
count_files() {
    local d="$1"
    if [[ -d "$d" ]]; then
        find "$d" -type f | wc -l | tr -d ' '
    else
        echo "0"
    fi
}

# ============================================================
# RUN SWIFTBEAVER
# ============================================================
SB_ELAPSED=0
SB_TOTAL_FILES=0
SB_TOTAL_SIZE="0"

if $RUN_SB; then
    mkdir -p "$SB_OUT"
    echo "" | tee -a "$SUMMARY"
    echo ">>> SwiftBeaver" | tee -a "$SUMMARY"
    echo "    Started: $(date)" | tee -a "$SUMMARY"

    SB_START=$(date +%s)

    "$SB_BIN" \
        --input "$IMAGE" \
        --output "$SB_OUT" \
        --metadata-backend parquet \
        --workers 16 \
        --chunk-size-mib 64 \
        --scan-strings \
        --scan-entropy \
        --dedupe \
        --skip-duplicates \
        --progress-interval-secs 60 \
        2>&1 | tee "$OUT_DIR/sb_log.txt"

    SB_END=$(date +%s)
    SB_ELAPSED=$((SB_END - SB_START))
    SB_MINS=$((SB_ELAPSED / 60))
    SB_SECS=$((SB_ELAPSED % 60))

    SB_TOTAL_SIZE=$(du -sh "$SB_OUT" | cut -f1)
    SB_TOTAL_FILES=$(count_files "$SB_OUT")

    echo "    Finished: $(date)" | tee -a "$SUMMARY"
    echo "    Wall time: ${SB_MINS}m ${SB_SECS}s ($SB_ELAPSED seconds)" | tee -a "$SUMMARY"
    echo "    Output size: $SB_TOTAL_SIZE" | tee -a "$SUMMARY"
    echo "    Total files: $SB_TOTAL_FILES" | tee -a "$SUMMARY"

    # Per-type counts
    echo "    Carved files by type:" | tee -a "$SUMMARY"
    for d in "$SB_OUT"/*/; do
        [ -d "$d" ] || continue
        # Skip the run-id directory, look inside it
        for sub in "$d"*/; do
            [ -d "$sub" ] || continue
            name=$(basename "$sub")
            count=$(count_files "$sub")
            size=$(du -sh "$sub" 2>/dev/null | cut -f1)
            echo "      $name: $count files ($size)" | tee -a "$SUMMARY"
        done
        # Also count files directly in the run dir
        direct=$(find "$d" -maxdepth 1 -type f | wc -l | tr -d ' ')
        if (( direct > 0 )); then
            name=$(basename "$d")
            size=$(du -sh "$d" 2>/dev/null | cut -f1)
            echo "      $name: $direct files (direct) ($size total)" | tee -a "$SUMMARY"
        fi
    done
fi

# ============================================================
# RUN BULK_EXTRACTOR
# ============================================================
BE_ELAPSED=0
BE_TOTAL_FILES=0
BE_TOTAL_SIZE="0"

if $RUN_BE; then
    mkdir -p "$BE_OUT"
    echo "" | tee -a "$SUMMARY"
    echo ">>> bulk_extractor" | tee -a "$SUMMARY"
    echo "    Started: $(date)" | tee -a "$SUMMARY"

    BE_START=$(date +%s)

    # Enable carving for comparable output (mode 2 = carve everything)
    # Use same thread count as SwiftBeaver for fair comparison
    bulk_extractor \
        -o "$BE_OUT" \
        -j 16 \
        -S jpeg_carve_mode=2 \
        -S sqlite_carved_carve_mode=2 \
        -S zip_carve_mode=2 \
        -S rar_carve_mode=2 \
        "$IMAGE" \
        2>&1 | tee "$OUT_DIR/be_log.txt"

    BE_END=$(date +%s)
    BE_ELAPSED=$((BE_END - BE_START))
    BE_MINS=$((BE_ELAPSED / 60))
    BE_SECS=$((BE_ELAPSED % 60))

    BE_TOTAL_SIZE=$(du -sh "$BE_OUT" | cut -f1)
    BE_TOTAL_FILES=$(count_files "$BE_OUT")

    echo "    Finished: $(date)" | tee -a "$SUMMARY"
    echo "    Wall time: ${BE_MINS}m ${BE_SECS}s ($BE_ELAPSED seconds)" | tee -a "$SUMMARY"
    echo "    Output size: $BE_TOTAL_SIZE" | tee -a "$SUMMARY"
    echo "    Total output files: $BE_TOTAL_FILES" | tee -a "$SUMMARY"
fi

# ============================================================
# COLLECT PER-CATEGORY COUNTS
# ============================================================
echo "" | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"
echo " PER-CATEGORY COMPARISON"                | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

# --- bulk_extractor feature counts ---
BE_EMAILS=0; BE_URLS=0; BE_PHONES=0; BE_DOMAINS=0
BE_CARVED_JPEG=0; BE_CARVED_ZIP=0; BE_CARVED_RAR=0; BE_CARVED_SQLITE=0

if $RUN_BE && [[ -d "$BE_OUT" ]]; then
    # Feature files: one line per finding (skip comment lines starting with #)
    BE_EMAILS=$(grep -c -v '^#' "$BE_OUT/email.txt" 2>/dev/null || echo "0")
    BE_URLS=$(grep -c -v '^#' "$BE_OUT/url.txt" 2>/dev/null || echo "0")
    BE_PHONES=$(grep -c -v '^#' "$BE_OUT/telephone.txt" 2>/dev/null || echo "0")
    BE_DOMAINS=$(grep -c -v '^#' "$BE_OUT/domain.txt" 2>/dev/null || echo "0")

    # Carved files in subdirectories
    BE_CARVED_JPEG=$(count_files "$BE_OUT/jpeg")
    BE_CARVED_ZIP=$(count_files "$BE_OUT/zip")
    BE_CARVED_RAR=$(count_files "$BE_OUT/rar")
    BE_CARVED_SQLITE=$(count_files "$BE_OUT/sqlite")
fi

# --- SwiftBeaver counts ---
# SwiftBeaver output is organized by run_id/type/
SB_CARVED_JPEG=0; SB_CARVED_ZIP=0; SB_CARVED_RAR=0; SB_CARVED_SQLITE=0
SB_CARVED_PDF=0; SB_CARVED_PNG=0; SB_CARVED_MP4=0; SB_CARVED_MP3=0
SB_TOTAL_BY_TYPE=""

if $RUN_SB && [[ -d "$SB_OUT" ]]; then
    # SwiftBeaver organizes as: output/<run_id>/<type>/
    # Find all type directories
    for type_dir in "$SB_OUT"/*/*/; do
        [ -d "$type_dir" ] || continue
        type_name=$(basename "$type_dir")
        type_count=$(count_files "$type_dir")
        type_size=$(du -sh "$type_dir" 2>/dev/null | cut -f1)

        # Map to variables for comparison
        case "$type_name" in
            jpeg|jpg)        SB_CARVED_JPEG=$((SB_CARVED_JPEG + type_count)) ;;
            zip)             SB_CARVED_ZIP=$((SB_CARVED_ZIP + type_count)) ;;
            rar)             SB_CARVED_RAR=$((SB_CARVED_RAR + type_count)) ;;
            sqlite|sqlite_page|sqlite_wal) SB_CARVED_SQLITE=$((SB_CARVED_SQLITE + type_count)) ;;
            pdf)             SB_CARVED_PDF=$type_count ;;
            png)             SB_CARVED_PNG=$type_count ;;
            mp4)             SB_CARVED_MP4=$type_count ;;
            mp3)             SB_CARVED_MP3=$type_count ;;
        esac
    done
fi

# --- Print comparison table ---
printf "\n  %-20s %12s %12s\n" "Category" "SwiftBeaver" "bulk_extractor" | tee -a "$SUMMARY"
printf "  %-20s %12s %12s\n"   "--------" "-----------" "--------------" | tee -a "$SUMMARY"

if $RUN_SB && $RUN_BE; then
    printf "  %-20s %12s %12s\n" "Wall time (s)" "$SB_ELAPSED" "$BE_ELAPSED" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Output size" "$SB_TOTAL_SIZE" "$BE_TOTAL_SIZE" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Total output files" "$SB_TOTAL_FILES" "$BE_TOTAL_FILES" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Emails" "-" "$BE_EMAILS" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "URLs" "-" "$BE_URLS" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Phone numbers" "-" "$BE_PHONES" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Domains" "-" "$BE_DOMAINS" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved JPEG" "$SB_CARVED_JPEG" "$BE_CARVED_JPEG" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved ZIP" "$SB_CARVED_ZIP" "$BE_CARVED_ZIP" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved RAR" "$SB_CARVED_RAR" "$BE_CARVED_RAR" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved SQLite" "$SB_CARVED_SQLITE" "$BE_CARVED_SQLITE" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved PDF" "$SB_CARVED_PDF" "-" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved PNG" "$SB_CARVED_PNG" "-" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved MP4" "$SB_CARVED_MP4" "-" | tee -a "$SUMMARY"
    printf "  %-20s %12s %12s\n" "Carved MP3" "$SB_CARVED_MP3" "-" | tee -a "$SUMMARY"
elif $RUN_SB; then
    printf "  %-20s %12s\n" "Wall time (s)" "$SB_ELAPSED" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Output size" "$SB_TOTAL_SIZE" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Total files" "$SB_TOTAL_FILES" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved JPEG" "$SB_CARVED_JPEG" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved ZIP" "$SB_CARVED_ZIP" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved RAR" "$SB_CARVED_RAR" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved SQLite" "$SB_CARVED_SQLITE" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved PDF" "$SB_CARVED_PDF" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved PNG" "$SB_CARVED_PNG" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved MP4" "$SB_CARVED_MP4" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved MP3" "$SB_CARVED_MP3" | tee -a "$SUMMARY"
elif $RUN_BE; then
    printf "  %-20s %12s\n" "Wall time (s)" "$BE_ELAPSED" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Output size" "$BE_TOTAL_SIZE" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Total files" "$BE_TOTAL_FILES" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Emails" "$BE_EMAILS" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "URLs" "$BE_URLS" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Phone numbers" "$BE_PHONES" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Domains" "$BE_DOMAINS" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved JPEG" "$BE_CARVED_JPEG" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved ZIP" "$BE_CARVED_ZIP" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved RAR" "$BE_CARVED_RAR" | tee -a "$SUMMARY"
    printf "  %-20s %12s\n" "Carved SQLite" "$BE_CARVED_SQLITE" | tee -a "$SUMMARY"
fi

# ============================================================
# MACHINE-READABLE JSON OUTPUT
# ============================================================
cat > "$JSON_OUT" <<EOF
{
  "run_id": "$RUN_ID",
  "image": "$IMAGE",
  "evidence_size": "$(echo "$DISK_SIZE" | sed 's/.*:\s*//' | xargs)",
  "system": {
    "cpus": $NUM_CPUS,
    "ram_mib": $TOTAL_MEM_MB
  },
  "swiftbeaver": {
    "ran": $RUN_SB,
    "wall_time_s": $SB_ELAPSED,
    "output_files": $SB_TOTAL_FILES,
    "output_size": "$SB_TOTAL_SIZE",
    "carved": {
      "jpeg": $SB_CARVED_JPEG,
      "zip": $SB_CARVED_ZIP,
      "rar": $SB_CARVED_RAR,
      "sqlite": $SB_CARVED_SQLITE,
      "pdf": $SB_CARVED_PDF,
      "png": $SB_CARVED_PNG,
      "mp4": $SB_CARVED_MP4,
      "mp3": $SB_CARVED_MP3
    }
  },
  "bulk_extractor": {
    "ran": $RUN_BE,
    "wall_time_s": $BE_ELAPSED,
    "output_files": $BE_TOTAL_FILES,
    "output_size": "$BE_TOTAL_SIZE",
    "features": {
      "emails": $BE_EMAILS,
      "urls": $BE_URLS,
      "phones": $BE_PHONES,
      "domains": $BE_DOMAINS
    },
    "carved": {
      "jpeg": $BE_CARVED_JPEG,
      "zip": $BE_CARVED_ZIP,
      "rar": $BE_CARVED_RAR,
      "sqlite": $BE_CARVED_SQLITE
    }
  },
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

# ============================================================
# THROUGHPUT COMPARISON
# ============================================================
echo "" | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"
echo " THROUGHPUT"                              | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

# Extract evidence size in bytes for throughput calculation
EVIDENCE_BYTES=$(ewfinfo "$IMAGE" 2>/dev/null | grep "Media size" | grep -oP '\d+(?= bytes)' || echo "0")

if $RUN_SB && (( SB_ELAPSED > 0 )) && (( EVIDENCE_BYTES > 0 )); then
    SB_THROUGHPUT_MBS=$(( EVIDENCE_BYTES / SB_ELAPSED / 1048576 ))
    echo "  SwiftBeaver: ${SB_THROUGHPUT_MBS} MiB/s" | tee -a "$SUMMARY"
fi

if $RUN_BE && (( BE_ELAPSED > 0 )) && (( EVIDENCE_BYTES > 0 )); then
    BE_THROUGHPUT_MBS=$(( EVIDENCE_BYTES / BE_ELAPSED / 1048576 ))
    echo "  bulk_extractor: ${BE_THROUGHPUT_MBS} MiB/s" | tee -a "$SUMMARY"
fi

if $RUN_SB && $RUN_BE && (( BE_ELAPSED > 0 )); then
    echo "" | tee -a "$SUMMARY"
    if (( SB_ELAPSED < BE_ELAPSED )); then
        SPEEDUP_PCT=$(( (BE_ELAPSED - SB_ELAPSED) * 100 / BE_ELAPSED ))
        echo "  SwiftBeaver is ${SPEEDUP_PCT}% faster" | tee -a "$SUMMARY"
    elif (( SB_ELAPSED > BE_ELAPSED )); then
        SLOWDOWN_PCT=$(( (SB_ELAPSED - BE_ELAPSED) * 100 / BE_ELAPSED ))
        echo "  SwiftBeaver is ${SLOWDOWN_PCT}% slower" | tee -a "$SUMMARY"
    else
        echo "  Same wall time" | tee -a "$SUMMARY"
    fi
fi

# Symlink latest comparison
ln -sfn "comparisons/$RUN_ID" "$IMAGE_DIR/latest_comparison"

# ============================================================
# FINAL
# ============================================================
echo "" | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"
echo " Completed: $(date)"                     | tee -a "$SUMMARY"
echo " Results: $OUT_DIR/"                     | tee -a "$SUMMARY"
echo " JSON: $JSON_OUT"                        | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

echo ""
echo ">>> DONE."
echo ">>> Summary: $SUMMARY"
echo ">>> JSON: $JSON_OUT"
