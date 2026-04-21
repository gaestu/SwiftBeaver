#!/bin/bash
# Generate missing sample files for the golden test image
# 
# Requirements:
#   - ImageMagick (convert command)
#   - sqlite3
#   - rar (optional, for RAR files)
#   - p7zip (7z command, for 7z files)
#   - LibreOffice (optional, for PPTX)
#   - ffmpeg (optional, for tiny media files)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=== Generating missing sample files ==="
echo "Working directory: $SCRIPT_DIR"
echo ""

# Create temp directory for intermediate files
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

#------------------------------------------------------------------------------
# BMP - Missing from images/
#------------------------------------------------------------------------------
echo "[1/9] Generating BMP..."
if command -v convert &> /dev/null; then
    convert -size 50x50 xc:purple BMP3:images/test.bmp
    echo "  ✓ Created images/test.bmp"
else
    echo "  ✗ ImageMagick not found, skipping BMP"
fi

#------------------------------------------------------------------------------
# RAR - Missing from archives/
#------------------------------------------------------------------------------
echo "[2/9] Generating RAR..."
mkdir -p archives
echo "This is test content for RAR archive." > "$TEMP_DIR/test_content.txt"
echo "Created: 2025-01-01T00:00:00Z" >> "$TEMP_DIR/test_content.txt"

if command -v rar &> /dev/null; then
    rar a -ep archives/test.rar "$TEMP_DIR/test_content.txt" > /dev/null
    echo "  ✓ Created archives/test.rar"
elif command -v unrar &> /dev/null; then
    echo "  ✗ 'rar' not found (unrar cannot create). Install: sudo dnf install rar"
else
    echo "  ✗ RAR tools not found. Install: sudo dnf install rar"
fi

#------------------------------------------------------------------------------
# 7z - Missing from archives/
#------------------------------------------------------------------------------
echo "[3/9] Generating 7z..."
echo "This is test content for 7z archive." > "$TEMP_DIR/test_content_7z.txt"
echo "Created: 2025-01-01T00:00:00Z" >> "$TEMP_DIR/test_content_7z.txt"

if command -v 7z &> /dev/null; then
    7z a archives/test.7z "$TEMP_DIR/test_content_7z.txt" > /dev/null
    echo "  ✓ Created archives/test.7z"
else
    echo "  ✗ 7z not found. Install: sudo dnf install p7zip p7zip-plugins"
fi

#------------------------------------------------------------------------------
# TAR variants - Missing from archives/
#------------------------------------------------------------------------------
echo "[4/9] Generating TAR archives..."
echo "Test content for tar archive" > "$TEMP_DIR/tarfile.txt"

tar -cf archives/test.tar -C "$TEMP_DIR" tarfile.txt 2>/dev/null && echo "  ✓ Created archives/test.tar"
tar -czf archives/test.tar.gz -C "$TEMP_DIR" tarfile.txt 2>/dev/null && echo "  ✓ Created archives/test.tar.gz"
tar -cjf archives/test.tar.bz2 -C "$TEMP_DIR" tarfile.txt 2>/dev/null && echo "  ✓ Created archives/test.tar.bz2"

if command -v xz &> /dev/null; then
    tar -cJf archives/test.tar.xz -C "$TEMP_DIR" tarfile.txt 2>/dev/null && echo "  ✓ Created archives/test.tar.xz"
fi

# Standalone compressed files
gzip -c "$TEMP_DIR/tarfile.txt" > archives/test.txt.gz && echo "  ✓ Created archives/test.txt.gz"
bzip2 -c "$TEMP_DIR/tarfile.txt" > archives/test.txt.bz2 && echo "  ✓ Created archives/test.txt.bz2"

if command -v xz &> /dev/null; then
    xz -c "$TEMP_DIR/tarfile.txt" > archives/test.txt.xz && echo "  ✓ Created archives/test.txt.xz"
fi

#------------------------------------------------------------------------------
# PPTX - Missing from documents/
#------------------------------------------------------------------------------
echo "[5/9] Generating PPTX..."
# Create minimal PPTX (ZIP with specific structure)
PPTX_DIR="$TEMP_DIR/pptx_build"
mkdir -p "$PPTX_DIR/_rels" "$PPTX_DIR/ppt/_rels" "$PPTX_DIR/ppt/slides/_rels" "$PPTX_DIR/ppt/slides" "$PPTX_DIR/ppt/slideLayouts" "$PPTX_DIR/ppt/slideMasters"

# [Content_Types].xml
cat > "$PPTX_DIR/[Content_Types].xml" << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>
EOF

# _rels/.rels
cat > "$PPTX_DIR/_rels/.rels" << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>
EOF

# ppt/presentation.xml
cat > "$PPTX_DIR/ppt/presentation.xml" << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst>
</p:presentation>
EOF

# ppt/_rels/presentation.xml.rels
cat > "$PPTX_DIR/ppt/_rels/presentation.xml.rels" << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>
EOF

# ppt/slides/slide1.xml
cat > "$PPTX_DIR/ppt/slides/slide1.xml" << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
    <p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
      <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="2000000"/></a:xfrm></p:spPr>
      <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Test Slide</a:t></a:r></a:p></p:txBody>
    </p:sp>
  </p:spTree></p:cSld>
</p:sld>
EOF

(cd "$PPTX_DIR" && zip -r "$SCRIPT_DIR/documents/test.pptx" . > /dev/null 2>&1)
echo "  ✓ Created documents/test.pptx"

#------------------------------------------------------------------------------
# SQLite - Missing from databases/
#------------------------------------------------------------------------------
echo "[6/9] Generating SQLite databases..."
mkdir -p databases

# Generic test SQLite
sqlite3 databases/test.sqlite << 'EOF'
CREATE TABLE test_data (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    value TEXT,
    created_at INTEGER
);
INSERT INTO test_data VALUES (1, 'entry1', 'test value 1', 1735500000);
INSERT INTO test_data VALUES (2, 'entry2', 'test value 2', 1735500100);
INSERT INTO test_data VALUES (3, 'entry3', 'test value 3', 1735500200);
EOF
echo "  ✓ Created databases/test.sqlite"

# Chrome-style History database
sqlite3 databases/History << 'EOF'
CREATE TABLE urls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL,
    title TEXT,
    visit_count INTEGER DEFAULT 0,
    typed_count INTEGER DEFAULT 0,
    last_visit_time INTEGER NOT NULL,
    hidden INTEGER DEFAULT 0
);
CREATE TABLE visits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url INTEGER NOT NULL,
    visit_time INTEGER NOT NULL,
    from_visit INTEGER,
    transition INTEGER DEFAULT 0,
    segment_id INTEGER,
    visit_duration INTEGER DEFAULT 0
);
CREATE INDEX urls_url_index ON urls (url);
CREATE INDEX visits_url_index ON visits (url);
CREATE INDEX visits_time_index ON visits (visit_time);

INSERT INTO urls (id, url, title, visit_count, last_visit_time) VALUES 
    (1, 'https://www.google.com/', 'Google', 15, 13380000000000000),
    (2, 'https://github.com/', 'GitHub', 8, 13379900000000000),
    (3, 'https://example.com/test/page?query=1', 'Example Test Page', 3, 13379800000000000),
    (4, 'https://stackoverflow.com/questions/12345', 'Stack Overflow Question', 2, 13379700000000000),
    (5, 'http://192.168.1.1/admin', 'Router Admin', 1, 13379600000000000);

INSERT INTO visits (url, visit_time, transition) VALUES 
    (1, 13380000000000000, 805306368),
    (1, 13379950000000000, 805306368),
    (2, 13379900000000000, 805306376),
    (3, 13379800000000000, 805306368),
    (4, 13379700000000000, 805306376),
    (5, 13379600000000000, 805306368);
EOF
echo "  ✓ Created databases/History (Chrome-style)"

# Chrome-style Cookies database
sqlite3 databases/Cookies << 'EOF'
CREATE TABLE cookies (
    creation_utc INTEGER NOT NULL,
    host_key TEXT NOT NULL,
    top_frame_site_key TEXT NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    encrypted_value BLOB DEFAULT '',
    path TEXT NOT NULL,
    expires_utc INTEGER NOT NULL,
    is_secure INTEGER NOT NULL,
    is_httponly INTEGER NOT NULL,
    last_access_utc INTEGER NOT NULL,
    has_expires INTEGER NOT NULL DEFAULT 1,
    is_persistent INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 1,
    samesite INTEGER NOT NULL DEFAULT -1,
    source_scheme INTEGER NOT NULL DEFAULT 0,
    source_port INTEGER NOT NULL DEFAULT -1,
    is_same_party INTEGER NOT NULL DEFAULT 0,
    last_update_utc INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX cookies_unique_index ON cookies(host_key, top_frame_site_key, name, path);

INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, path, expires_utc, is_secure, is_httponly, last_access_utc) VALUES
    (13380000000000000, '.google.com', '', 'NID', 'test_value_1', '/', 13395000000000000, 1, 1, 13380000000000000),
    (13379000000000000, '.github.com', '', 'logged_in', 'yes', '/', 13410000000000000, 1, 1, 13380000000000000),
    (13378000000000000, 'example.com', '', 'session_id', 'abc123xyz', '/', 13390000000000000, 0, 0, 13379500000000000);
EOF
echo "  ✓ Created databases/Cookies (Chrome-style)"

# Firefox-style places.sqlite
sqlite3 databases/places.sqlite << 'EOF'
CREATE TABLE moz_places (
    id INTEGER PRIMARY KEY,
    url TEXT,
    title TEXT,
    rev_host TEXT,
    visit_count INTEGER DEFAULT 0,
    hidden INTEGER DEFAULT 0,
    typed INTEGER DEFAULT 0,
    frecency INTEGER DEFAULT -1,
    last_visit_date INTEGER,
    guid TEXT,
    foreign_count INTEGER DEFAULT 0,
    url_hash INTEGER DEFAULT 0
);
CREATE TABLE moz_historyvisits (
    id INTEGER PRIMARY KEY,
    from_visit INTEGER,
    place_id INTEGER,
    visit_date INTEGER,
    visit_type INTEGER,
    session INTEGER
);

INSERT INTO moz_places (id, url, title, rev_host, visit_count, last_visit_date, guid) VALUES
    (1, 'https://www.mozilla.org/', 'Mozilla', 'gro.allizom.www.', 10, 1735500000000000, 'abc123'),
    (2, 'https://developer.mozilla.org/en-US/', 'MDN Web Docs', 'gro.allizom.repoleved.', 5, 1735400000000000, 'def456'),
    (3, 'https://forensic-test.example.com/evidence', 'Forensic Evidence', 'moc.elpmaxe.tset-cisnrof.', 2, 1735300000000000, 'ghi789');

INSERT INTO moz_historyvisits (place_id, visit_date, visit_type) VALUES
    (1, 1735500000000000, 1),
    (1, 1735450000000000, 2),
    (2, 1735400000000000, 1),
    (3, 1735300000000000, 1);
EOF
echo "  ✓ Created databases/places.sqlite (Firefox-style)"

#------------------------------------------------------------------------------
# Browser forensic fixtures for golden image (downloads, deleted rows, WAL)
#------------------------------------------------------------------------------
echo "[7/9] Generating browser forensic fixtures..."

# Chromium-style downloads fixture
sqlite3 databases/browser_downloads.sqlite << 'EOF'
CREATE TABLE downloads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_path TEXT NOT NULL,
    start_time INTEGER,
    end_time INTEGER,
    total_bytes INTEGER,
    state INTEGER
);
CREATE TABLE downloads_url_chains (
    id INTEGER NOT NULL,
    chain_index INTEGER NOT NULL,
    url TEXT NOT NULL
);
INSERT INTO downloads (id, target_path, start_time, end_time, total_bytes, state) VALUES
    (1, '/home/user/Downloads/sample.zip', 13380600000000000, 13380600001000000, 4096, 1);
INSERT INTO downloads_url_chains (id, chain_index, url) VALUES
    (1, 0, 'https://downloads.example.com/sample.zip');
EOF
echo "  ✓ Created databases/browser_downloads.sqlite"

# Deleted-row fixture (keeps deleted content in free pages/freelist)
sqlite3 databases/browser_history_deleted.sqlite << 'EOF'
PRAGMA secure_delete=OFF;
CREATE TABLE urls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL,
    title TEXT,
    last_visit_time INTEGER NOT NULL
);
CREATE TABLE visits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url INTEGER NOT NULL,
    visit_time INTEGER NOT NULL,
    transition INTEGER DEFAULT 0
);
INSERT INTO urls (id, url, title, last_visit_time) VALUES
    (1, 'https://kept.example.com/', 'Kept Row', 13380700000000000),
    (2, 'https://deleted.example.com/', 'Deleted Row', 13380750000000000);
INSERT INTO visits (url, visit_time, transition) VALUES
    (1, 13380700000000000, 805306368),
    (2, 13380750000000000, 805306376);
DELETE FROM visits WHERE url = 2;
DELETE FROM urls WHERE id = 2;
EOF
echo "  ✓ Created databases/browser_history_deleted.sqlite"

# WAL fixture (contains -wal sidecar for future WAL parsing tests)
rm -f databases/browser_history_wal.sqlite databases/browser_history_wal.sqlite-wal databases/browser_history_wal.sqlite-shm databases/browser_history_wal_snapshot.bin
sqlite3 databases/browser_history_wal.sqlite << 'EOF'
PRAGMA journal_mode=WAL;
PRAGMA synchronous=OFF;
PRAGMA wal_autocheckpoint=0;
CREATE TABLE urls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL,
    title TEXT,
    last_visit_time INTEGER NOT NULL
);
CREATE TABLE visits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url INTEGER NOT NULL,
    visit_time INTEGER NOT NULL,
    transition INTEGER DEFAULT 0
);
INSERT INTO urls (id, url, title, last_visit_time) VALUES
    (1, 'https://wal.example.com/base', 'WAL Base', 13380800000000000);
INSERT INTO visits (url, visit_time, transition) VALUES
    (1, 13380800000000000, 805306368);
BEGIN;
INSERT INTO urls (id, url, title, last_visit_time) VALUES
    (2, 'https://wal.example.com/uncheckpointed', 'WAL Pending', 13380850000000000);
INSERT INTO visits (url, visit_time, transition) VALUES
    (2, 13380850000000000, 805306376);
COMMIT;
.system cp databases/browser_history_wal.sqlite-wal databases/browser_history_wal_snapshot.bin
EOF
if [[ -f databases/browser_history_wal_snapshot.bin ]]; then
    mv -f databases/browser_history_wal_snapshot.bin databases/browser_history_wal.sqlite-wal
fi
if [[ -f databases/browser_history_wal.sqlite-wal ]]; then
    echo "  ✓ Created databases/browser_history_wal.sqlite (+ -wal)"
else
    echo "  ! Created databases/browser_history_wal.sqlite (no -wal sidecar on this sqlite build)"
fi

# Synthetic isolated SQLite page fragment fixture
# (single validated 4096-byte table-leaf page for sqlite_page carving tests)
dd if=/dev/zero of=databases/sqlite_orphan_page.bin bs=4096 count=1 2>/dev/null
echo -n "0d000000010ff0000ff0" | xxd -r -p | dd of=databases/sqlite_orphan_page.bin bs=1 seek=0 conv=notrunc 2>/dev/null
echo -n "01" | xxd -r -p | dd of=databases/sqlite_orphan_page.bin bs=1 seek=4080 conv=notrunc 2>/dev/null
echo "  ✓ Created databases/sqlite_orphan_page.bin"

#------------------------------------------------------------------------------
# strings.txt - Test data for string scanning
#------------------------------------------------------------------------------
echo "[8/9] Generating strings.txt..."
cat > other/strings.txt << 'EOF'
# Golden Image String Test Data
# This file contains test patterns for URL/email/phone extraction

# URLs - Various formats
https://www.example.com/path/to/page?query=value&foo=bar
http://test-server.local:8080/api/v1/users
https://subdomain.domain.co.uk/resource.html
ftp://files.example.net/downloads/archive.zip
https://192.168.1.100/admin/login.php
http://user:password@internal.corp.local/secure
https://cdn.jsdelivr.net/npm/package@1.0.0/dist/file.min.js

# Email addresses - Various formats
user@example.com
admin.test@test-domain.org
john.doe+newsletter@company.example.net
support_ticket@help.example.com
info@münchen.de
contact@日本語.jp

# Phone numbers - International formats
+1-555-123-4567
+1 (800) 555-0199
+44 20 7946 0958
+49 30 12345678
+33 1 23 45 67 89
555-123-4567
(555) 123-4567

# Credit card numbers (test/fake numbers)
4111111111111111
5500000000000004
378282246310005
6011111111111117

# File paths - Windows and Unix
C:\Users\TestUser\Documents\evidence.docx
C:\Windows\System32\config\SYSTEM
D:\Backups\2025\full_backup.zip
/home/user/forensics/case_001/image.dd
/var/log/auth.log
/etc/passwd
\\\\server\\share\\folder\\file.txt
\\\\192.168.1.50\\c$\\Windows\\System32

# IP addresses
192.168.1.1
10.0.0.254
172.16.0.1
8.8.8.8
2001:0db8:85a3:0000:0000:8a2e:0370:7334

# MAC addresses
00:1A:2B:3C:4D:5E
AA-BB-CC-DD-EE-FF
001A.2B3C.4D5E

# Registry paths (Windows)
HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run
HKCU\Software\Classes\.txt

# Misc identifiers
UUID: 550e8400-e29b-41d4-a716-446655440000
SHA256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
MD5: d41d8cd98f00b204e9800998ecf8427e

# Plain ASCII text for printable string detection
The quick brown fox jumps over the lazy dog.
ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789
Pack my box with five dozen liquor jugs.
How vexingly quick daft zebras jump!

# Base64 encoded data (for entropy detection)
SGVsbG8gV29ybGQhIFRoaXMgaXMgYSB0ZXN0IG1lc3NhZ2UgZW5jb2RlZCBpbiBiYXNlNjQu

# JSON-like content
{"user_id": 12345, "email": "test@example.com", "api_key": "sk_live_abc123xyz"}

# SQL-like content
SELECT * FROM users WHERE email = 'admin@example.com';
INSERT INTO logs VALUES (1, '2025-12-30', 'login_attempt');

# END OF TEST DATA
EOF
echo "  ✓ Created other/strings.txt"

#------------------------------------------------------------------------------
# Tiny media files (optional, for smaller golden image)
#------------------------------------------------------------------------------
echo "[9/9] Generating tiny media files (optional)..."
mkdir -p media_tiny

if command -v ffmpeg &> /dev/null; then
    # Tiny MP4 (~20KB)
    ffmpeg -y -f lavfi -i color=c=blue:s=16x16:d=0.5 -f lavfi -i anullsrc=r=8000:cl=mono \
        -c:v libx264 -preset ultrafast -crf 51 -c:a aac -b:a 8k -t 0.5 \
        media_tiny/tiny.mp4 2>/dev/null && echo "  ✓ Created media_tiny/tiny.mp4"
    
    # Tiny AVI
    ffmpeg -y -f lavfi -i color=c=red:s=16x16:d=0.5 \
        -c:v mjpeg -q:v 31 -t 0.5 \
        media_tiny/tiny.avi 2>/dev/null && echo "  ✓ Created media_tiny/tiny.avi"
    
    # Tiny MP3
    ffmpeg -y -f lavfi -i "sine=frequency=440:duration=0.5" \
        -c:a libmp3lame -b:a 8k \
        media_tiny/tiny.mp3 2>/dev/null && echo "  ✓ Created media_tiny/tiny.mp3"
    
    # Tiny WAV
    ffmpeg -y -f lavfi -i "sine=frequency=440:duration=0.5" \
        -ar 8000 -ac 1 \
        media_tiny/tiny.wav 2>/dev/null && echo "  ✓ Created media_tiny/tiny.wav"
    
    # Tiny WebM
    ffmpeg -y -f lavfi -i color=c=green:s=16x16:d=0.5 \
        -c:v libvpx -b:v 10k -t 0.5 \
        media_tiny/tiny.webm 2>/dev/null && echo "  ✓ Created media_tiny/tiny.webm"
    
    # Tiny MKV
    ffmpeg -y -f lavfi -i color=c=yellow:s=16x16:d=0.5 \
        -c:v libx264 -preset ultrafast -crf 51 -t 0.5 \
        media_tiny/tiny.mkv 2>/dev/null && echo "  ✓ Created media_tiny/tiny.mkv"
else
    echo "  ✗ ffmpeg not found, skipping tiny media files"
fi

#------------------------------------------------------------------------------
# Windows Prefetch files (.pf) — synthetic, all versions + MAM-compressed
#------------------------------------------------------------------------------
echo "[10/10] Generating Windows Prefetch files..."
mkdir -p windows/prefetch

if command -v python3 &> /dev/null; then
python3 - <<'PYEOF'
import struct, os, sys

def utf16le_padded(s, length=60):
    """Encode s as UTF-16LE, zero-padded (null-terminated) to exactly length bytes."""
    encoded = s.encode('utf-16-le')
    # include null terminator then pad the rest with zeros
    return (encoded + b'\x00\x00')[:length].ljust(length, b'\x00')

def filetime_from_unix(unix_ts):
    """Convert Unix timestamp to Windows FILETIME (100-ns intervals since 1601-01-01 UTC)."""
    return (unix_ts + 11_644_473_600) * 10_000_000

def make_scca_header(version, exe_name, pf_hash, unknown1=0):
    """Build the 84-byte SCCA Prefetch header (file_size field left as 0 for caller to fill)."""
    h = bytearray(84)
    struct.pack_into('<I', h, 0, version)
    h[4:8] = b'SCCA'
    struct.pack_into('<I', h, 8, unknown1)
    # [12-15]: file_size — filled by each make_vXX function
    h[16:76] = utf16le_padded(exe_name)
    struct.pack_into('<I', h, 76, pf_hash)
    # [80-83]: flags = 0
    return h

def make_v17(exe_name, pf_hash, run_count, last_run_unix):
    """v17 = Windows XP.  File-info block: 36 B section ptrs + 8 B timestamp + 4 B run_count + 4 B pad."""
    header = make_scca_header(17, exe_name, pf_hash, unknown1=0x0F000000)
    info = bytearray(52)
    struct.pack_into('<Q', info, 36, filetime_from_unix(last_run_unix))  # offset 120
    struct.pack_into('<I', info, 44, run_count)                          # offset 128
    file_size = len(header) + len(info)
    struct.pack_into('<I', header, 12, file_size)
    return bytes(header) + bytes(info)

def make_v23(exe_name, pf_hash, run_count, last_run_unix):
    """v23 = Windows Vista/7.  Same section block as v17 + 80 B extra unknown fields."""
    header = make_scca_header(23, exe_name, pf_hash)
    info = bytearray(132)  # 36 + 8 + 4 + 4 + 80
    struct.pack_into('<Q', info, 36, filetime_from_unix(last_run_unix))  # offset 120
    struct.pack_into('<I', info, 44, run_count)                          # offset 128
    file_size = len(header) + len(info)
    struct.pack_into('<I', header, 12, file_size)
    return bytes(header) + bytes(info)

def make_v26(exe_name, pf_hash, run_count, last_run_unix):
    """v26 = Windows 8/8.1.  Same layout as v23."""
    header = make_scca_header(26, exe_name, pf_hash)
    info = bytearray(132)
    struct.pack_into('<Q', info, 36, filetime_from_unix(last_run_unix))  # offset 120
    struct.pack_into('<I', info, 44, run_count)                          # offset 128
    file_size = len(header) + len(info)
    struct.pack_into('<I', header, 12, file_size)
    return bytes(header) + bytes(info)

def make_v30(exe_name, pf_hash, run_count, last_run_times_unix):
    """v30 = Windows 10/11.  Section block + 8×uint64 last-run timestamps + run_count + extra."""
    header = make_scca_header(30, exe_name, pf_hash)
    # 36 B section ptrs | 64 B (8 × uint64 timestamps) | 4 B run_count | 4 B unknown | 80 B extra
    info = bytearray(188)
    for i, ts in enumerate(last_run_times_unix[:8]):
        if ts:
            struct.pack_into('<Q', info, 36 + i * 8, filetime_from_unix(ts))  # offsets 120–183
    struct.pack_into('<I', info, 100, run_count)                               # offset 184
    file_size = len(header) + len(info)
    struct.pack_into('<I', header, 12, file_size)
    return bytes(header) + bytes(info)

def lzxpress_huffman_compress(data):
    """
    LZXPRESS Huffman compression (MS-XCA), all-literal encoding.

    Assigns code length 8 to every literal symbol (0-255) and length 0 to all
    match symbols (256-511).  Canonical codes are therefore symbol == code.
    The bitstream is LSB-first, so each input byte V maps to bit_reverse(V)
    in the output byte stream.  The 256-byte Huffman symbol table precedes
    each 65536-byte uncompressed chunk.
    """
    result = bytearray()
    offset = 0
    while offset < len(data):
        chunk = data[offset:offset + 65536]
        offset += 65536

        # Symbol table: 512 symbols × 4-bit code-length entries = 256 bytes
        #   bytes  0-127: symbols   0-255 — code length 8 (nibble 0x8)
        #   bytes 128-255: symbols 256-511 — code length 0 (nibble 0x0)
        table = bytearray(256)
        for i in range(128):
            table[i] = 0x88  # low nibble = 8, high nibble = 8
        result.extend(table)

        # Compressed stream: canonical code for symbol V is V (8 bits),
        # written MSB-first into the LSB-first bitstream → bit_reverse(V).
        for b in chunk:
            result.append(int('{:08b}'.format(b)[::-1], 2))

    return bytes(result)

def make_mam(scca_data):
    """Wrap SCCA data in the MAM header and LZXPRESS Huffman-compress it."""
    compressed = lzxpress_huffman_compress(scca_data)
    hdr = bytearray(8)
    hdr[0:4] = b'MAM\x04'
    struct.pack_into('<I', hdr, 4, len(scca_data))  # uncompressed size
    return bytes(hdr) + compressed

os.makedirs('windows/prefetch', exist_ok=True)

# v17 — Windows XP
data = make_v17('NOTEPAD.EXE', 0xABCD1234, 3, 1609459200)
with open('windows/prefetch/NOTEPAD.EXE-ABCD1234.pf', 'wb') as f:
    f.write(data)
print('  \u2713 windows/prefetch/NOTEPAD.EXE-ABCD1234.pf  (v17 XP, uncompressed SCCA)')

# v23 — Windows Vista/7
data = make_v23('NOTEPAD.EXE', 0xB1234567, 5, 1609459200)
with open('windows/prefetch/NOTEPAD.EXE-B1234567.pf', 'wb') as f:
    f.write(data)
print('  \u2713 windows/prefetch/NOTEPAD.EXE-B1234567.pf  (v23 Vista/7, uncompressed SCCA)')

# v26 — Windows 8/8.1
data = make_v26('NOTEPAD.EXE', 0xC2345678, 2, 1609459200)
with open('windows/prefetch/NOTEPAD.EXE-C2345678.pf', 'wb') as f:
    f.write(data)
print('  \u2713 windows/prefetch/NOTEPAD.EXE-C2345678.pf  (v26 Win8, uncompressed SCCA)')

# v30 — Windows 10/11 uncompressed (plain SCCA, no MAM wrapper)
data = make_v30('NOTEPAD.EXE', 0xD3456789, 7,
                [1609459200, 1609372800, 1609286400, 0, 0, 0, 0, 0])
with open('windows/prefetch/NOTEPAD.EXE-D3456789.pf', 'wb') as f:
    f.write(data)
print('  \u2713 windows/prefetch/NOTEPAD.EXE-D3456789.pf  (v30 Win10 uncompressed SCCA)')

# v30 — Windows 10/11 MAM-compressed (LZXPRESS Huffman, realistic format)
inner = make_v30('CMD.EXE', 0xE4567890, 12,
                 [1609459200, 1609372800, 0, 0, 0, 0, 0, 0])
mam_data = make_mam(inner)
with open('windows/prefetch/CMD.EXE-E4567890.pf', 'wb') as f:
    f.write(mam_data)
print('  \u2713 windows/prefetch/CMD.EXE-E4567890.pf      (v30 Win10 MAM/LZXPRESS Huffman)')

print('  Total Prefetch files: 5')
PYEOF
else
    echo "  ✗ python3 not found, skipping Prefetch file generation"
fi

#------------------------------------------------------------------------------
# Summary
#------------------------------------------------------------------------------
echo ""
echo "=== Generation Complete ==="
echo ""
echo "Generated files:"
find . -newer "$0" -type f ! -name "*.sh" ! -name "*.md" ! -name "source.txt" 2>/dev/null | sort | while read f; do
    SIZE=$(stat -c%s "$f" 2>/dev/null || stat -f%z "$f" 2>/dev/null || echo "?")
    printf "  %-50s %10s bytes\n" "$f" "$SIZE"
done

echo ""
echo "To use smaller media files instead of the large downloaded ones:"
echo "  cp media_tiny/* video/  # Replace large video files"
echo "  cp media_tiny/tiny.mp3 audio/test.mp3"
echo "  cp media_tiny/tiny.wav audio/test.wav"
