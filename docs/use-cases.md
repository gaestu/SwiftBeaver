# Use Cases & Examples

This guide demonstrates real-world forensic scenarios and how to accomplish them with SwiftBeaver.

## Table of Contents

1. [Basic File Recovery](#basic-file-recovery)
2. [Deleted Photo Recovery](#deleted-photo-recovery)
3. [Email Investigation](#email-investigation)
4. [Browser Database Carving](#browser-database-carving)
5. [Document Discovery](#document-discovery)
6. [Large-Scale Image Processing](#large-scale-image-processing)
7. [Encrypted Container Analysis](#encrypted-container-analysis)
8. [Mobile Device Forensics](#mobile-device-forensics)
9. [Malware Sample Extraction](#malware-sample-extraction)
10. [Data Breach Investigation](#data-breach-investigation)

---

## Basic File Recovery

**Scenario**: User accidentally deleted photos and wants to recover them.

### Step 1: Create Disk Image

```bash
# Linux - create image of USB drive
sudo dd if=/dev/sdb of=usb_recovery.dd bs=4M status=progress

# macOS - create image of disk
sudo dd if=/dev/disk2 of=usb_recovery.dd bs=4m
```

### Step 2: Scan for Images

```bash
swiftbeaver \
    --input usb_recovery.dd \
    --output ./recovered \
    --enable-types jpeg,png,gif,bmp,tiff,webp
```

### Step 3: Review Results

```bash
cd recovered/20250104T120000Z_*/

# Count recovered images
find carved/ -type f | wc -l

# View metadata
cat metadata/carved_files.jsonl | jq '.file_type' | sort | uniq -c
#     142 jpeg
#      58 png
#      12 gif

# Find large images (likely photos)
cat metadata/carved_files.jsonl | jq 'select(.size > 500000) | .path'
```

---

## Deleted Photo Recovery

**Scenario**: Forensic examiner needs to recover deleted photos from suspect's phone.

### Step 1: Extract Phone Image

```bash
# Using Android Debug Bridge (ADB)
adb root
adb shell dd if=/dev/block/mmcblk0p30 | dd of=phone_userdata.dd
```

### Step 2: Comprehensive Image Scan

```bash
swiftbeaver \
    --input phone_userdata.dd \
    --output ./phone_recovery \
    --enable-types jpeg,png,gif,webp,mp4,heic \
    --scan-strings \
    --scan-utf16 \
    --min-size 10000  # Skip thumbnails
```

### Step 3: Extract EXIF Metadata

```bash
cd phone_recovery/20250104T*/carved/jpeg/

# Extract EXIF from all JPEGs
for img in *.jpg; do
    exiftool "$img" >> ../../../exif_report.txt
    echo "---" >> ../../../exif_report.txt
done

# Find images with GPS coordinates
exiftool -csv -r -GPS* . > ../../../gps_locations.csv
```

### Step 4: Timeline Analysis

```bash
# Create timeline from EXIF dates
exiftool -csv -r -DateTimeOriginal -FileName . | \
    sort -t',' -k2 > timeline.csv

# Or use SwiftBeaver metadata
cat ../../metadata/carved_files.jsonl | \
    jq -r '[.global_start, .file_type, .path, .size] | @csv' | \
    sort -n > offset_timeline.csv
```

---

## Email Investigation

**Scenario**: Investigate employee's sent emails from disk image.

### Step 1: Scan for Email Artifacts

```bash
swiftbeaver \
    --input employee_laptop.E01 \
    --output ./email_investigation \
    --enable-types eml,pst,msg,sqlite \
    --scan-strings \
    --scan-emails \
    --scan-urls
```

### Step 2: Analyze Email Addresses

```bash
cd email_investigation/20250104T*/

# Extract all email addresses found
cat metadata/string_artefacts.jsonl | \
    jq -r 'select(.artefact_type == "email") | .value' | \
    sort | uniq > all_emails.txt

# Find high-frequency correspondents
cat all_emails.txt | \
    sed 's/.*@//' | \
    sort | uniq -c | sort -rn | head -20
```

### Step 3: Search for Keywords

```bash
# Extract URLs containing keywords
cat metadata/string_artefacts.jsonl | \
    jq -r 'select(.artefact_type == "url" and (.value | contains("confidential"))) | .value' \
    > suspicious_urls.txt

# Find email content with keywords (from carved EML files)
grep -ri "confidential\|secret\|internal" carved/eml/ > keyword_hits.txt
```

---

## Browser Database Carving

**Scenario**: Recover browser SQLite databases, WAL files, and page fragments for offline analysis.

### Step 1: Comprehensive Scan

```bash
swiftbeaver \
    --input suspect_disk.dd \
    --output ./browser_analysis \
    --enable-types sqlite,sqlite_wal,sqlite_page
```

### Step 2: Review Carved SQLite Outputs

```bash
cd browser_analysis/20250104T*/

# Count carved sqlite artefacts by type
cat metadata/carved_files.jsonl | \
    jq -r '.file_type' | grep -E '^sqlite(_wal|_page)?$' | sort | uniq -c

# List carved WAL outputs
cat metadata/carved_files.jsonl | \
    jq -r 'select(.file_type=="sqlite_wal") | [.path,.global_start,.size,.validated] | @tsv'

# List carved page fragments
cat metadata/carved_files.jsonl | \
    jq -r 'select(.file_type=="sqlite_page") | [.path,.global_start,.size,.validated] | @tsv'
```

### Step 3: Handoff to External SQLite Tooling

Use your preferred SQLite/WAL/page parser against files in `carved/sqlite/`, `carved/sqlite_wal/`, and `carved/sqlite_page/`.
See `docs/sqlite_carve_handoff.md` for a tool-agnostic workflow.

---

## Document Discovery

**Scenario**: Find all Word documents, PDFs, and Excel files.

### Step 1: Target Document Formats

```bash
swiftbeaver \
    --input corporate_server.dd \
    --output ./document_discovery \
    --enable-types pdf,docx,xlsx,pptx,doc,xls,ppt,rtf,odt,ods
```

### Step 2: Organize by Type

```bash
cd document_discovery/20250104T*/carved/

# Count documents by type
find . -type f | sed 's|.*/||' | sed 's/.*\.//' | sort | uniq -c
#    1243 pdf
#     892 docx
#     456 xlsx
#     123 pptx
#      89 doc

# Find recent documents (by offset, assumes chronological writes)
cat ../../metadata/carved_files.jsonl | \
    jq 'select(.file_type | IN("pdf", "docx", "xlsx"))' | \
    jq -s 'sort_by(.global_start) | reverse | .[0:100]'
```

### Step 3: Extract Text from PDFs

```bash
# Extract text from all PDFs
for pdf in pdf/*.pdf; do
    pdftotext "$pdf" "${pdf%.pdf}.txt"
done

# Search for keywords across all text
grep -i "confidential\|proprietary" pdf/*.txt > sensitive_docs.txt
```

---

## Large-Scale Image Processing

**Scenario**: Process 500GB disk image efficiently.

### Step 1: Initial Scan with Limits

```bash
# Scan with resource limits
swiftbeaver \
    --input large_disk.dd \
    --output ./large_scan \
    --gpu \
    --max-memory-mib 8192 \
    --max-open-files 1024 \
    --progress-interval-secs 60
```

### Step 2: Use Checkpointing

```bash
# Scan with checkpointing (in case of interruption)
swiftbeaver \
    --input large_disk.dd \
    --output ./large_scan \
    --gpu \
    --checkpoint-path ./checkpoint.json \
    --progress-interval-secs 30
```

If interrupted, resume:

```bash
swiftbeaver \
    --input large_disk.dd \
    --output ./large_scan \
    --gpu \
    --resume-from ./checkpoint.json
```

### Step 3: Parallel Analysis with Parquet

```bash
# Use Parquet for efficient querying
swiftbeaver \
    --input large_disk.dd \
    --output ./large_scan \
    --metadata-backend parquet \
    --gpu

# Query with DuckDB
duckdb -c "
SELECT file_type, COUNT(*) as count, SUM(size) as total_bytes
FROM parquet_scan('metadata/carved_files.parquet')
GROUP BY file_type
ORDER BY count DESC;
"
```

---

## Encrypted Container Analysis

**Scenario**: Analyze unencrypted portions of partially encrypted disk.

### Step 1: Scan with Entropy Detection

```bash
swiftbeaver \
    --input encrypted_disk.dd \
    --output ./entropy_analysis \
    --scan-entropy \
    --entropy-threshold 7.5 \
    --entropy-window-bytes 4096
```

### Step 2: Review Entropy Regions

```bash
cd entropy_analysis/20250104T*/

# View high-entropy regions (likely encrypted)
cat metadata/entropy_regions.jsonl | \
    jq 'select(.entropy > 7.8)'

# Calculate encrypted vs unencrypted ratio
total_bytes=$(cat metadata/run_summary.jsonl | jq '.bytes_scanned')
encrypted_bytes=$(cat metadata/entropy_regions.jsonl | jq -s 'map(.length) | add')

echo "Encrypted: $((encrypted_bytes * 100 / total_bytes))%"
```

### Step 3: Focus on Unencrypted Data

```bash
# Extract files found outside encrypted regions
# (SwiftBeaver automatically carves from unencrypted areas)

# Find patterns in unencrypted data
cat metadata/string_artefacts.jsonl | \
    jq 'select(.value | contains("password") or contains("key"))'
```

---

## Mobile Device Forensics

**Scenario**: Extract artifacts from Android device image.

### Step 1: Scan for Mobile Formats

```bash
swiftbeaver \
    --input android_userdata.dd \
    --output ./mobile_forensics \
    --enable-types sqlite,jpeg,png,webp,mp4,3gp,amr \
    --scan-strings \
    --scan-utf16 \
    --scan-phones \
    --scan-emails
```

### Step 2: Analyze SQLite Databases

```bash
cd mobile_forensics/20250104T*/carved/sqlite/

# Identify database types
for db in *.sqlite; do
    echo "=== $db ==="
    sqlite3 "$db" ".schema" | head -20
done

# Extract SMS messages (if mmssms.db found)
sqlite3 mmssms.db "SELECT address, body, date FROM sms ORDER BY date DESC LIMIT 100;"
```

### Step 3: Phone Number Analysis

```bash
# Extract all phone numbers
cat ../../metadata/string_artefacts.jsonl | \
    jq -r 'select(.artefact_type == "phone") | .value' | \
    sort | uniq -c | sort -rn > phone_frequency.txt

# Correlate with call logs (if found in SQLite)
# Look for contacts.db, calllog.db, etc.
```

---

## Malware Sample Extraction

**Scenario**: Extract suspicious executables from infected system.

### Step 1: Scan for Executables

```bash
swiftbeaver \
    --input infected_system.dd \
    --output ./malware_extraction \
    --enable-types exe,dll,elf,pe,zip,rar,7z
```

### Step 2: Identify Suspicious Files

```bash
cd malware_extraction/20250104T*/

# Find executables in unusual locations (by offset)
cat metadata/carved_files.jsonl | \
    jq 'select(.file_type == "elf" or .file_type == "exe")'

# Extract file hashes
cat metadata/carved_files.jsonl | \
    jq -r '[.md5, .sha256, .path] | @csv' > file_hashes.csv
```

### Step 3: VirusTotal Lookup

```bash
# Check hashes against VirusTotal
while IFS=',' read -r md5 sha256 path; do
    echo "Checking: $path"
    # Use VirusTotal API or web interface
    curl -s "https://www.virustotal.com/api/v3/files/$sha256" \
        -H "x-apikey: YOUR_API_KEY"
done < file_hashes.csv
```

---

## Data Breach Investigation

**Scenario**: Investigate suspected data exfiltration.

### Step 1: Comprehensive Scan

```bash
swiftbeaver \
    --input employee_laptop.dd \
    --output ./data_breach \
    --scan-strings \
    --scan-urls \
    --scan-emails \
    --enable-types zip,rar,7z,pdf,docx,xlsx,sqlite
```

### Step 2: Search for Exfiltration Indicators

```bash
cd data_breach/20250104T*/

# Find URLs to file-sharing sites
cat metadata/string_artefacts.jsonl | \
    jq -r 'select(.artefact_type == "url" and 
           (.value | contains("dropbox") or 
                    contains("wetransfer") or 
                    contains("mega.nz"))) | .value'

# Find large archives (potential data packages)
cat metadata/carved_files.jsonl | \
    jq 'select((.file_type | IN("zip", "rar", "7z")) and .size > 10000000)'
```

### Step 3: Timeline Analysis

```bash
# Create timeline of archive creation (by disk offset)
cat metadata/carved_files.jsonl | \
    jq -r 'select(.file_type | IN("zip", "rar", "7z")) | 
           [.global_start, .size, .path] | @csv' | \
    sort -n > archive_timeline.csv

# Correlate with browser history
cat metadata/browser_history.jsonl | \
    jq -r 'select(.url | contains("cloud") or contains("transfer")) | 
           [.visit_time, .url] | @csv'
```

---

## Performance Optimization Tips

### For Large Images (>500GB)

```bash
swiftbeaver \
    --input huge_disk.dd \
    --output ./output \
    --gpu \
    --max-memory-mib 16384 \
    --metadata-backend parquet \
    --progress-interval-secs 60 \
    --checkpoint-path checkpoint.json
```

### For Fast Triage

```bash
# Scan only high-value formats
swiftbeaver \
    --input quick_scan.dd \
    --output ./triage \
    --enable-types jpeg,pdf,docx,xlsx,sqlite \
    --dry-run  # Don't write files, just report counts
```

### For Detailed Analysis

```bash
# Enable all features
swiftbeaver \
    --input detailed_scan.dd \
    --output ./detailed \
    --gpu \
    --scan-strings \
    --scan-utf16 \
    --scan-urls \
    --scan-emails \
    --scan-phones \
    --scan-entropy \
    --metadata-backend parquet
```

---

## Next Steps

- **[Performance Tuning](performance.md)** - Optimize scans for your hardware
- **[Output Interpretation](output-interpretation.md)** - Understand metadata fields
- **[Troubleshooting](troubleshooting.md)** - Common issues and solutions
- **[File Format Support](file-formats.md)** - Complete format reference
