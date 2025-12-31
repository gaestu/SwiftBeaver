#!/usr/bin/env bash
# Generate golden.raw and golden.E01 from all sample files.
#
# Usage: ./generate.sh [--no-e01]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES_DIR_DEFAULT="$SCRIPT_DIR/samples"
SAMPLES_DIR_FALLBACK="$SCRIPT_DIR/sample"
SAMPLES_DIR="${SAMPLES_DIR:-$SAMPLES_DIR_DEFAULT}"
if [[ ! -d "$SAMPLES_DIR" && -d "$SAMPLES_DIR_FALLBACK" ]]; then
    SAMPLES_DIR="$SAMPLES_DIR_FALLBACK"
fi
IGNORE_FILE="$SCRIPT_DIR/.goldenignore"
OUTPUT_RAW="$SCRIPT_DIR/golden.raw"
OUTPUT_E01="$SCRIPT_DIR/golden"
MANIFEST="$SCRIPT_DIR/manifest.json"

SKIP_E01=false
if [[ "${1:-}" == "--no-e01" ]]; then
    SKIP_E01=true
fi

ALIGNMENT=4096

file_size() {
    local path="$1"
    if stat -c%s "$path" >/dev/null 2>&1; then
        stat -c%s "$path"
    else
        stat -f%z "$path"
    fi
}

sha256_file() {
    local path="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print $1}'
    else
        echo "ERROR: sha256sum or shasum is required" >&2
        exit 1
    fi
}

collect_files() {
    if command -v rg >/dev/null 2>&1 && [[ -f "$IGNORE_FILE" ]]; then
        mapfile -t ALL_FILES < <(
            rg --files --ignore-file "$IGNORE_FILE" "$SAMPLES_DIR" \
                | sed "s|^$SAMPLES_DIR/||" | sort
        )
        return
    fi

    if [[ -f "$IGNORE_FILE" ]]; then
        local python_bin=""
        if command -v python3 >/dev/null 2>&1; then
            python_bin="python3"
        elif command -v python >/dev/null 2>&1; then
            python_bin="python"
        fi
        if [[ -z "$python_bin" ]]; then
            mapfile -t ALL_FILES < <(
                find "$SAMPLES_DIR" -type f -print \
                    | sed "s|^$SAMPLES_DIR/||" | sort
            )
            return
        fi
        mapfile -t ALL_FILES < <(
            "$python_bin" - "$SAMPLES_DIR" "$IGNORE_FILE" <<'PY'
import fnmatch
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
ignore_path = pathlib.Path(sys.argv[2])
patterns = []
for line in ignore_path.read_text().splitlines():
    line = line.strip()
    if not line or line.startswith("#"):
        continue
    patterns.append(line)

def ignored(rel):
    for pat in patterns:
        if fnmatch.fnmatch(rel, pat) or fnmatch.fnmatch(pathlib.Path(rel).name, pat):
            return True
    return False

files = []
for path in root.rglob("*"):
    if not path.is_file():
        continue
    rel = path.relative_to(root).as_posix()
    if ignored(rel):
        continue
    files.append(rel)

for rel in sorted(files):
    print(rel)
PY
        )
        return
    fi

    mapfile -t ALL_FILES < <(
        find "$SAMPLES_DIR" -type f -print \
            | sed "s|^$SAMPLES_DIR/||" | sort
    )
}

get_category() {
    local path="$1"
    echo "${path%%/*}"
}

get_extension() {
    local path="$1"
    local filename="${path##*/}"
    if [[ "$filename" == *.* ]]; then
        echo "${filename##*.}" | tr '[:upper:]' '[:lower:]'
    else
        echo ""
    fi
}

echo "=== Golden Image Generator (All Files) ==="
echo "Samples dir: $SAMPLES_DIR"
echo ""

collect_files

TOTAL_FILES=${#ALL_FILES[@]}
echo "Found $TOTAL_FILES files to include"
echo ""

if [[ $TOTAL_FILES -eq 0 ]]; then
    echo "ERROR: No sample files found in $SAMPLES_DIR" >&2
    exit 1
fi

echo "Building raw image..."
OFFSET=$ALIGNMENT
dd if=/dev/zero of="$OUTPUT_RAW" bs=$ALIGNMENT count=1 2>/dev/null

cat > "$MANIFEST" << EOF
{
  "description": "Golden test image - ALL sample files for fastcarve testing",
  "generated": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "alignment": $ALIGNMENT,
  "sample_dir": "$(basename "$SAMPLES_DIR")",
  "files": [
EOF

declare -A CATEGORY_COUNTS
declare -A CATEGORY_SIZES
TOTAL_SIZE=0

FIRST=true
for rel_path in "${ALL_FILES[@]}"; do
    full_path="$SAMPLES_DIR/$rel_path"

    FILE_SIZE=$(file_size "$full_path")
    FILE_SHA256=$(sha256_file "$full_path")
    CATEGORY=$(get_category "$rel_path")
    EXTENSION=$(get_extension "$rel_path")

    dd if="$full_path" of="$OUTPUT_RAW" bs=1 seek=$OFFSET conv=notrunc 2>/dev/null

    CATEGORY_COUNTS["$CATEGORY"]=$(( ${CATEGORY_COUNTS[$CATEGORY]:-0} + 1 ))
    CATEGORY_SIZES["$CATEGORY"]=$(( ${CATEGORY_SIZES[$CATEGORY]:-0} + FILE_SIZE ))
    TOTAL_SIZE=$((TOTAL_SIZE + FILE_SIZE))

    if [[ "$FIRST" != "true" ]]; then
        printf ',\n' >> "$MANIFEST"
    fi
    FIRST=false

    cat >> "$MANIFEST" << EOF
    {
      "path": "$rel_path",
      "category": "$CATEGORY",
      "extension": "$EXTENSION",
      "offset": $OFFSET,
      "offset_hex": "0x$(printf '%X' $OFFSET)",
      "size": $FILE_SIZE,
      "sha256": "$FILE_SHA256"
    }
EOF

    printf "  %-50s @ 0x%08X (%d bytes)\n" "$rel_path" "$OFFSET" "$FILE_SIZE"

    OFFSET=$(( ((OFFSET + FILE_SIZE + ALIGNMENT - 1) / ALIGNMENT) * ALIGNMENT ))
done

FINAL_SIZE=$OFFSET
truncate -s $FINAL_SIZE "$OUTPUT_RAW"

RAW_SHA256=$(sha256_file "$OUTPUT_RAW")

cat >> "$MANIFEST" << EOF

  ],
  "summary": {
    "total_files": $TOTAL_FILES,
    "total_data_size": $TOTAL_SIZE,
    "image_size": $FINAL_SIZE,
    "categories": {
EOF

FIRST_CAT=true
for cat in $(echo "${!CATEGORY_COUNTS[@]}" | tr ' ' '\n' | sort); do
    if [[ "$FIRST_CAT" != "true" ]]; then
        printf ',\n' >> "$MANIFEST"
    fi
    FIRST_CAT=false
    printf '      "%s": {"files": %d, "bytes": %d}' \
        "$cat" "${CATEGORY_COUNTS[$cat]}" "${CATEGORY_SIZES[$cat]}" >> "$MANIFEST"
done

cat >> "$MANIFEST" << EOF

    }
  },
  "raw_sha256": "$RAW_SHA256"
}
EOF

echo ""
echo "Created $OUTPUT_RAW"
echo "  Files: $TOTAL_FILES"
echo "  Data:  $TOTAL_SIZE bytes"
echo "  Image: $FINAL_SIZE bytes (with alignment padding)"
echo "  SHA256: $RAW_SHA256"
echo ""
echo "Manifest: $MANIFEST"

if [[ "$SKIP_E01" == "true" ]]; then
    echo ""
    echo "Skipping E01 generation (--no-e01 flag)"
elif command -v ewfacquire >/dev/null 2>&1; then
    echo ""
    echo "Converting to E01 format..."
    rm -f "${OUTPUT_E01}.E01"

    ewfacquire -t "$OUTPUT_E01" \
               -u \
               -c best \
               -S 0 \
               -C "golden_test" \
               -D "Golden test image - all fastcarve samples" \
               -e "automated" \
               -E "golden_001" \
               "$OUTPUT_RAW"

    E01_SIZE=$(file_size "${OUTPUT_E01}.E01")
    E01_SHA256=$(sha256_file "${OUTPUT_E01}.E01")

    echo ""
    echo "Created ${OUTPUT_E01}.E01"
    echo "  Size: $E01_SIZE bytes ($(( E01_SIZE * 100 / FINAL_SIZE ))% of raw)"
    echo "  SHA256: $E01_SHA256"

    if command -v ewfverify >/dev/null 2>&1; then
        echo ""
        echo "Verifying E01..."
        if ewfverify "${OUTPUT_E01}.E01"; then
            echo "E01 verification passed"
        else
            echo "E01 verification failed" >&2
            exit 1
        fi
    fi
else
    echo ""
    echo "WARNING: ewfacquire not found"
    echo "Install libewf-tools to generate E01:"
    echo "  Fedora/RHEL: sudo dnf install libewf-tools"
    echo "  Debian/Ubuntu: sudo apt install ewf-tools"
    echo ""
    echo "Raw image created; run with ewfacquire installed for E01."
fi

echo ""
echo "=== Done ==="
echo ""
echo "Test commands:"
echo "  cargo test golden                    # Raw image tests"
echo "  cargo test golden --features ewf     # Include E01 tests"
