#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# Profiled Benchmark: SwiftBeaver
#
# Captures time-series CPU, memory, and I/O metrics alongside
# the carve run for performance analysis.
#
# Each run creates a timestamped subdirectory:
#   <image_dir>/benchmarks/<RUN_ID>/
#     summary.txt           — human-readable summary
#     system_metrics.csv    — time-series system samples
#     process_metrics.csv   — time-series per-process samples
#     io_metrics.csv        — time-series disk I/O samples
#     swiftbeaver_log.txt   — SwiftBeaver stdout/stderr
#
# Usage:
#   ./scripts/run_benchmark_profiled.sh --image images/U63395/U63395.E01
#   ./scripts/run_benchmark_profiled.sh --image /path/to/evidence.E01
#   SAMPLE_INTERVAL=10 ./scripts/run_benchmark_profiled.sh --image ...
# ============================================================

# --- Configuration ---
SAMPLE_INTERVAL="${SAMPLE_INTERVAL:-5}"  # seconds between samples

# Resolve paths: script lives in scripts/
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Parse flags ---
IMAGE=""
for arg in "$@"; do
    case "$arg" in
        --image=*) IMAGE="${arg#--image=}" ;;
        --help|-h)
            echo "Usage: $0 --image <path-to-E01>"
            echo "  --image   Path to EWF evidence file (relative to repo root or absolute)"
            echo "  Environment: SAMPLE_INTERVAL=N (default: 5 seconds)"
            exit 0
            ;;
    esac
done
# Support --image <value> (two-arg form)
while [[ $# -gt 0 ]]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --image=*) IMAGE="${1#--image=}"; shift ;;
        --help|-h) shift ;;
        *) shift ;;
    esac
done

if [[ -z "$IMAGE" ]]; then
    echo "ERROR: --image is required"
    echo "Usage: $0 --image <path-to-E01>"
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
# e.g. images/U63395/U63395.E01 → outputs go in images/U63395/benchmarks/<RUN_ID>/
IMAGE_DIR="$(dirname "$IMAGE")"
IMAGE_NAME="$(basename "$IMAGE" | sed 's/\.[^.]*$//')"
SB_OUT="$IMAGE_DIR/swiftbeaver_output"

# Unique run directory with timestamp
RUN_ID="$(date +%Y%m%dT%H%M%S)"
OUT_DIR="$IMAGE_DIR/benchmarks/$RUN_ID"
mkdir -p "$OUT_DIR"

SUMMARY="$OUT_DIR/summary.txt"
SYSTEM_CSV="$OUT_DIR/system_metrics.csv"
PROCESS_CSV="$OUT_DIR/process_metrics.csv"
IO_CSV="$OUT_DIR/io_metrics.csv"

# --- Detect system info ---
NUM_CPUS=$(nproc)
TOTAL_MEM_KB=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
TOTAL_MEM_MB=$((TOTAL_MEM_KB / 1024))

# --- Identify the disk device for I/O tracking ---
# Try to find the block device backing the output directory
IO_DEVICE=""
if command -v lsblk &>/dev/null; then
    MOUNT_DEV=$(df "$OUT_DIR" 2>/dev/null | awk 'NR==2 {print $1}')
    IO_DEVICE=$(lsblk -ndo NAME "$MOUNT_DEV" 2>/dev/null || basename "$MOUNT_DEV" 2>/dev/null || echo "")
fi
# Fallback: try to find a common device name from /proc/diskstats
if [[ -z "$IO_DEVICE" ]]; then
    IO_DEVICE=$(awk '{print $3}' /proc/diskstats | grep -E '^(nvme[0-9]+n[0-9]+|sd[a-z]+|vd[a-z]+)$' | head -1 || echo "sda")
fi

echo "========================================" | tee "$SUMMARY"
echo " Profiled Benchmark: SwiftBeaver"       | tee -a "$SUMMARY"
echo " Image: $IMAGE_NAME ($IMAGE)"           | tee -a "$SUMMARY"
echo " Started: $(date)"                      | tee -a "$SUMMARY"
echo " CPUs: $NUM_CPUS"                       | tee -a "$SUMMARY"
echo " Total RAM: ${TOTAL_MEM_MB} MiB"        | tee -a "$SUMMARY"
echo " I/O device: $IO_DEVICE"                | tee -a "$SUMMARY"
echo " Sample interval: ${SAMPLE_INTERVAL}s"  | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

# --- Clean up previous run ---
rm -rf "$SB_OUT"
mkdir -p "$SB_OUT"

# ============================================================
# Background monitor: system-level metrics (CPU, memory, load)
# ============================================================
collect_system_metrics() {
    echo "elapsed_s,cpu_user_pct,cpu_sys_pct,cpu_iowait_pct,cpu_idle_pct,load_1m,load_5m,load_15m,mem_used_mb,mem_avail_mb,mem_cached_mb,mem_buffers_mb" > "$SYSTEM_CSV"

    local start_time=$1
    # Read initial CPU counters
    local prev_user prev_nice prev_system prev_idle prev_iowait prev_irq prev_softirq prev_steal
    read -r _ prev_user prev_nice prev_system prev_idle prev_iowait prev_irq prev_softirq prev_steal _ < /proc/stat

    while true; do
        sleep "$SAMPLE_INTERVAL"
        local now
        now=$(date +%s)
        local elapsed=$((now - start_time))

        # CPU: delta from /proc/stat
        local cur_user cur_nice cur_system cur_idle cur_iowait cur_irq cur_softirq cur_steal
        read -r _ cur_user cur_nice cur_system cur_idle cur_iowait cur_irq cur_softirq cur_steal _ < /proc/stat

        local d_user=$((cur_user - prev_user + cur_nice - prev_nice))
        local d_sys=$((cur_system - prev_system + cur_irq - prev_irq + cur_softirq - prev_softirq))
        local d_iowait=$((cur_iowait - prev_iowait))
        local d_idle=$((cur_idle - prev_idle))
        local d_steal=$((cur_steal - prev_steal))
        local d_total=$((d_user + d_sys + d_iowait + d_idle + d_steal))

        local cpu_user=0 cpu_sys=0 cpu_iowait=0 cpu_idle=0
        if (( d_total > 0 )); then
            cpu_user=$((d_user * 100 / d_total))
            cpu_sys=$((d_sys * 100 / d_total))
            cpu_iowait=$((d_iowait * 100 / d_total))
            cpu_idle=$((d_idle * 100 / d_total))
        fi

        prev_user=$cur_user; prev_nice=$cur_nice; prev_system=$cur_system
        prev_idle=$cur_idle; prev_iowait=$cur_iowait; prev_irq=$cur_irq
        prev_softirq=$cur_softirq; prev_steal=$cur_steal

        # Load average
        local load_1 load_5 load_15
        read -r load_1 load_5 load_15 _ < /proc/loadavg

        # Memory from /proc/meminfo
        local mem_total mem_free mem_avail mem_buffers mem_cached
        mem_total=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
        mem_free=$(awk '/^MemFree:/ {print $2}' /proc/meminfo)
        mem_avail=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
        mem_buffers=$(awk '/^Buffers:/ {print $2}' /proc/meminfo)
        mem_cached=$(awk '/^Cached:/ {print $2}' /proc/meminfo)

        local mem_used_mb=$(( (mem_total - mem_free) / 1024 ))
        local mem_avail_mb=$((mem_avail / 1024))
        local mem_cached_mb=$((mem_cached / 1024))
        local mem_buffers_mb=$((mem_buffers / 1024))

        echo "${elapsed},${cpu_user},${cpu_sys},${cpu_iowait},${cpu_idle},${load_1},${load_5},${load_15},${mem_used_mb},${mem_avail_mb},${mem_cached_mb},${mem_buffers_mb}" >> "$SYSTEM_CSV"
    done
}

# ============================================================
# Background monitor: per-process metrics (SwiftBeaver PID)
# ============================================================
collect_process_metrics() {
    echo "elapsed_s,pid,cpu_pct,rss_mb,vsz_mb,threads,voluntary_cs,nonvoluntary_cs,read_bytes,write_bytes" > "$PROCESS_CSV"

    local start_time=$1
    local sb_pid=$2

    while kill -0 "$sb_pid" 2>/dev/null; do
        sleep "$SAMPLE_INTERVAL"
        # Check again in case process exited during sleep
        kill -0 "$sb_pid" 2>/dev/null || break

        local now
        now=$(date +%s)
        local elapsed=$((now - start_time))

        # Per-process CPU from pidstat (1-second sample)
        local cpu_pct
        cpu_pct=$(pidstat -p "$sb_pid" 1 1 2>/dev/null | awk 'NR==4 {print $8}' || echo "0")
        [[ -z "$cpu_pct" ]] && cpu_pct="0"

        # Process memory and threads from /proc
        if [[ -r "/proc/$sb_pid/status" ]]; then
            local rss_kb vsz_kb threads vol_cs nonvol_cs
            rss_kb=$(awk '/^VmRSS:/ {print $2}' /proc/"$sb_pid"/status 2>/dev/null || echo "0")
            vsz_kb=$(awk '/^VmSize:/ {print $2}' /proc/"$sb_pid"/status 2>/dev/null || echo "0")
            threads=$(awk '/^Threads:/ {print $2}' /proc/"$sb_pid"/status 2>/dev/null || echo "0")
            vol_cs=$(awk '/^voluntary_ctxt_switches:/ {print $2}' /proc/"$sb_pid"/status 2>/dev/null || echo "0")
            nonvol_cs=$(awk '/^nonvoluntary_ctxt_switches:/ {print $2}' /proc/"$sb_pid"/status 2>/dev/null || echo "0")
            local rss_mb=$((rss_kb / 1024))
            local vsz_mb=$((vsz_kb / 1024))
        else
            local rss_mb=0 vsz_mb=0 threads=0 vol_cs=0 nonvol_cs=0
        fi

        # Per-process I/O from /proc/pid/io
        local read_bytes=0 write_bytes=0
        if [[ -r "/proc/$sb_pid/io" ]]; then
            read_bytes=$(awk '/^read_bytes:/ {print $2}' /proc/"$sb_pid"/io 2>/dev/null || echo "0")
            write_bytes=$(awk '/^write_bytes:/ {print $2}' /proc/"$sb_pid"/io 2>/dev/null || echo "0")
        fi

        echo "${elapsed},${sb_pid},${cpu_pct},${rss_mb},${vsz_mb},${threads},${vol_cs},${nonvol_cs},${read_bytes},${write_bytes}" >> "$PROCESS_CSV"
    done
}

# ============================================================
# Background monitor: disk I/O metrics
# ============================================================
collect_io_metrics() {
    echo "elapsed_s,device,reads_completed,sectors_read,read_ms,writes_completed,sectors_written,write_ms,io_in_progress,io_ms" > "$IO_CSV"

    local start_time=$1
    local device=$2

    while true; do
        sleep "$SAMPLE_INTERVAL"
        local now
        now=$(date +%s)
        local elapsed=$((now - start_time))

        # Parse /proc/diskstats for our device
        local line
        line=$(awk -v dev="$device" '$3 == dev {print}' /proc/diskstats 2>/dev/null || echo "")
        if [[ -n "$line" ]]; then
            local reads_completed sectors_read read_ms writes_completed sectors_written write_ms io_inflight io_ms
            reads_completed=$(echo "$line" | awk '{print $4}')
            sectors_read=$(echo "$line" | awk '{print $6}')
            read_ms=$(echo "$line" | awk '{print $7}')
            writes_completed=$(echo "$line" | awk '{print $8}')
            sectors_written=$(echo "$line" | awk '{print $10}')
            write_ms=$(echo "$line" | awk '{print $11}')
            io_inflight=$(echo "$line" | awk '{print $12}')
            io_ms=$(echo "$line" | awk '{print $13}')

            echo "${elapsed},${device},${reads_completed},${sectors_read},${read_ms},${writes_completed},${sectors_written},${write_ms},${io_inflight},${io_ms}" >> "$IO_CSV"
        fi
    done
}

# ============================================================
# Cleanup function: kill all background monitors
# ============================================================
MONITOR_PIDS=()
cleanup_monitors() {
    for pid in "${MONITOR_PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
}
trap cleanup_monitors EXIT

# ============================================================
# RUN SWIFTBEAVER
# ============================================================
echo "" | tee -a "$SUMMARY"
echo ">>> Running SwiftBeaver..." | tee -a "$SUMMARY"
echo "    Started: $(date)" | tee -a "$SUMMARY"

SB_START=$(date +%s)

# Start system-level and I/O monitors (before SwiftBeaver, so we capture startup)
collect_system_metrics "$SB_START" &
MONITOR_PIDS+=($!)

collect_io_metrics "$SB_START" "$IO_DEVICE" &
MONITOR_PIDS+=($!)

# Launch SwiftBeaver in background so we can grab its PID
"$REPO_ROOT/target/release/swiftbeaver" \
    --input "$IMAGE" \
    --output "$SB_OUT" \
    --metadata-backend parquet \
    --workers 16 \
    --chunk-size-mib 64 \
    --scan-strings \
    --scan-entropy \
    --dedupe \
    --skip-duplicates \
    --progress-interval-secs 30 \
    2>&1 | tee "$OUT_DIR/swiftbeaver_log.txt" &
SB_PIPE_PID=$!
# Symlink latest run for convenience
ln -sfn "benchmarks/$RUN_ID" "$IMAGE_DIR/latest_benchmark"

# Wait a moment for swiftbeaver to start, then find its actual PID
sleep 1
SB_PID=$(pgrep -f "swiftbeaver.*--input" | head -1 || echo "$SB_PIPE_PID")

echo "    SwiftBeaver PID: $SB_PID" | tee -a "$SUMMARY"

# Start per-process monitor
collect_process_metrics "$SB_START" "$SB_PID" &
MONITOR_PIDS+=($!)

# Wait for SwiftBeaver to finish
wait "$SB_PIPE_PID" || true

SB_END=$(date +%s)
SB_ELAPSED=$((SB_END - SB_START))
SB_MINS=$((SB_ELAPSED / 60))
SB_SECS=$((SB_ELAPSED % 60))

# Stop monitors
cleanup_monitors
MONITOR_PIDS=()

# Disable strict error mode for post-processing — these are non-critical
# reporting steps and should not abort the script.
set +e

echo "    Finished: $(date)" | tee -a "$SUMMARY"
echo "    Wall time: ${SB_MINS}m ${SB_SECS}s ($SB_ELAPSED seconds)" | tee -a "$SUMMARY"

# ============================================================
# Collect output stats
# ============================================================
SB_SIZE=$(du -sh "$SB_OUT" 2>/dev/null | cut -f1)
SB_FILES=$(find "$SB_OUT" -type f 2>/dev/null | wc -l)
echo "    Output size: ${SB_SIZE:-unknown}" | tee -a "$SUMMARY"
echo "    Output files: ${SB_FILES:-0}" | tee -a "$SUMMARY"

# Per-type carved file counts
echo "" | tee -a "$SUMMARY"
echo "    SwiftBeaver carved files by type:" | tee -a "$SUMMARY"
for d in "$SB_OUT"/*/; do
    [ -d "$d" ] || continue
    name=$(basename "$d")
    count=$(find "$d" -type f 2>/dev/null | wc -l)
    size=$(du -sh "$d" 2>/dev/null | cut -f1)
    echo "      $name: ${count:-0} files (${size:-?})" | tee -a "$SUMMARY"
done

# Disk size info
echo "" | tee -a "$SUMMARY"
if command -v ewfinfo &>/dev/null; then
    DISK_SIZE=$(ewfinfo "$IMAGE" 2>/dev/null | grep "Media size" || echo "unknown")
else
    DISK_SIZE="unknown (ewfinfo not found)"
fi
echo "    Evidence disk size: $DISK_SIZE" | tee -a "$SUMMARY"

# ============================================================
# Analyze collected metrics
# ============================================================
echo "" | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"
echo " RESOURCE USAGE ANALYSIS"               | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

# --- System CPU ---
if [[ -f "$SYSTEM_CSV" ]] && (( $(wc -l < "$SYSTEM_CSV") > 1 )); then
    echo "" | tee -a "$SUMMARY"
    echo "  System CPU (% of all $NUM_CPUS cores):" | tee -a "$SUMMARY"
    awk -F, 'NR>1 {
        if ($2+0 > max_user) max_user=$2+0;
        if ($3+0 > max_sys) max_sys=$3+0;
        if ($4+0 > max_iowait) max_iowait=$4+0;
        sum_user+=$2; sum_sys+=$3; sum_iowait+=$4; sum_idle+=$5; n++
    } END {
        if (n>0) {
            printf "    Avg user: %d%%  sys: %d%%  iowait: %d%%  idle: %d%%\n", sum_user/n, sum_sys/n, sum_iowait/n, sum_idle/n
            printf "    Peak user: %d%%  sys: %d%%  iowait: %d%%\n", max_user, max_sys, max_iowait
        }
    }' "$SYSTEM_CSV" | tee -a "$SUMMARY"

    # Load average
    echo "" | tee -a "$SUMMARY"
    echo "  Load average:" | tee -a "$SUMMARY"
    awk -F, 'NR>1 {
        if ($6+0 > max_1m) max_1m=$6+0;
        sum_1m+=$6; n++
    } END {
        if (n>0) printf "    Avg 1m: %.2f  Peak 1m: %.2f  (CPUs: '"$NUM_CPUS"')\n", sum_1m/n, max_1m
    }' "$SYSTEM_CSV" | tee -a "$SUMMARY"
fi

# --- Process CPU ---
if [[ -f "$PROCESS_CSV" ]] && (( $(wc -l < "$PROCESS_CSV") > 1 )); then
    echo "" | tee -a "$SUMMARY"
    echo "  SwiftBeaver process:" | tee -a "$SUMMARY"
    awk -F, 'NR>1 {
        if ($3+0 > max_cpu) max_cpu=$3+0;
        if ($4+0 > max_rss) max_rss=$4+0;
        if ($6+0 > max_threads) max_threads=$6+0;
        sum_cpu+=$3; sum_rss+=$4; n++;
        last_read=$9; last_write=$10; last_threads=$6
    } END {
        if (n>0) {
            printf "    Avg CPU: %.1f%%  Peak CPU: %.1f%%  (of %d00%%)\n", sum_cpu/n, max_cpu, '"$NUM_CPUS"'
            printf "    Avg RSS: %d MiB  Peak RSS: %d MiB  (of %d MiB)\n", sum_rss/n, max_rss, '"$TOTAL_MEM_MB"'
            printf "    Peak threads: %d\n", max_threads
            printf "    Total read: %.1f GiB  Total written: %.1f GiB\n", last_read/1073741824, last_write/1073741824
        }
    }' "$PROCESS_CSV" | tee -a "$SUMMARY"

    # CPU utilization ratio
    echo "" | tee -a "$SUMMARY"
    echo "  Utilization efficiency:" | tee -a "$SUMMARY"
    awk -F, 'NR>1 { sum_cpu+=$3; n++ } END {
        if (n>0) {
            avg=sum_cpu/n;
            max_possible='"$NUM_CPUS"' * 100;
            printf "    Avg CPU / Max possible: %.1f%% / %d%% = %.1f%% utilization\n", avg, max_possible, avg*100/max_possible
        }
    }' "$PROCESS_CSV" | tee -a "$SUMMARY"
fi

# --- Memory timeline ---
if [[ -f "$SYSTEM_CSV" ]] && (( $(wc -l < "$SYSTEM_CSV") > 1 )); then
    echo "" | tee -a "$SUMMARY"
    echo "  System memory:" | tee -a "$SUMMARY"
    awk -F, 'NR>1 {
        if ($9+0 > max_used) max_used=$9+0;
        sum_used+=$9; sum_avail+=$10; n++
    } END {
        if (n>0) {
            printf "    Avg used: %d MiB  Peak used: %d MiB  (of %d MiB)\n", sum_used/n, max_used, '"$TOTAL_MEM_MB"'
            printf "    Avg available: %d MiB\n", sum_avail/n
        }
    }' "$SYSTEM_CSV" | tee -a "$SUMMARY"
fi

# --- I/O throughput ---
if [[ -f "$IO_CSV" ]] && (( $(wc -l < "$IO_CSV") > 1 )); then
    echo "" | tee -a "$SUMMARY"
    echo "  Disk I/O ($IO_DEVICE):" | tee -a "$SUMMARY"
    # Compute deltas between samples for throughput
    awk -F, 'NR==2 { first_t=$1; first_r=$4; first_w=$7; prev_t=$1; prev_r=$4; prev_w=$7; next }
    NR>2 {
        dt=$1-prev_t;
        if (dt>0) {
            # sectors are 512 bytes
            read_mbs=($4-prev_r)*512/1048576/dt;
            write_mbs=($7-prev_w)*512/1048576/dt;
            if (read_mbs > max_read) max_read=read_mbs;
            if (write_mbs > max_write) max_write=write_mbs;
            sum_read+=read_mbs; sum_write+=write_mbs; n++;
        }
        last_t=$1; last_r=$4; last_w=$7;
        prev_t=$1; prev_r=$4; prev_w=$7;
    } END {
        if (n>0) {
            total_dt=last_t-first_t;
            total_read_mb=(last_r-first_r)*512/1048576;
            total_write_mb=(last_w-first_w)*512/1048576;
            printf "    Avg read: %.1f MiB/s  Peak read: %.1f MiB/s\n", sum_read/n, max_read
            printf "    Avg write: %.1f MiB/s  Peak write: %.1f MiB/s\n", sum_write/n, max_write
            if (total_dt>0) {
                printf "    Total read: %.1f GiB  Total written: %.1f GiB (over %ds)\n", total_read_mb/1024, total_write_mb/1024, total_dt
            }
        }
    }' "$IO_CSV" | tee -a "$SUMMARY"
fi

# ============================================================
# Final Summary
# ============================================================
echo "" | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"
echo " FINAL SUMMARY"                          | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"
echo " Wall time: ${SB_MINS}m ${SB_SECS}s ($SB_ELAPSED seconds)" | tee -a "$SUMMARY"
echo " Output size: $SB_SIZE"                  | tee -a "$SUMMARY"
echo " Total carved files: $SB_FILES"          | tee -a "$SUMMARY"
echo " Metrics files:"                         | tee -a "$SUMMARY"
echo "   $SYSTEM_CSV"                          | tee -a "$SUMMARY"
echo "   $PROCESS_CSV"                         | tee -a "$SUMMARY"
echo "   $IO_CSV"                              | tee -a "$SUMMARY"
echo " Completed: $(date)"                     | tee -a "$SUMMARY"
echo "========================================" | tee -a "$SUMMARY"

echo ""
echo ">>> DONE. Analyze the CSV files for detailed time-series data."
echo ">>> Summary: $SUMMARY"
echo ">>> System metrics: $SYSTEM_CSV (CPU, memory, load — ${SAMPLE_INTERVAL}s intervals)"
echo ">>> Process metrics: $PROCESS_CSV (per-process CPU, RSS, threads, I/O)"
echo ">>> I/O metrics: $IO_CSV (disk read/write throughput)"
