# Golden Image Sample Files

This document tracks sample files for the golden test image.

**Source:** Most files from [file-examples.com](https://file-examples.com) (free for testing)

**Scope:** SwiftBeaver currently carves jpeg/png/gif/pdf/zip/webp/sqlite/bmp/tiff/mp4/rar/7z; docx/xlsx/pptx are classified from ZIP content. Other formats below are optional/future.

**Placement:** the files should be placed in tests/golden_image in the according folders.

---

## Pictures / Images

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| jpeg/jpg | ✅ Have | file_example_JPG_100kB.jpg | 100 KB | |
| jpeg/jpg | ✅ Have | file_example_JPG_500kB.jpg | 500 KB | (pick smaller) |
| jpeg/jpg | ✅ Have | test_generated.jpg | small | Generated plasma |
| png | ✅ Have | file_example_PNG_500kB.png | 500 KB | |
| png | ✅ Have | file_example_PNG_1MB.png | 1 MB | (pick smaller) |
| png | ✅ Have | test_gradient.png | small | Generated gradient |
| gif | ✅ Have | file_example_GIF_500kB.gif | 500 KB | |
| gif | ✅ Have | 20251230_*.gif | ? | AI-generated |
| gif | ✅ Have | test_animated.gif | small | Generated animated |
| bmp | ✅ Have | test.bmp | small | Generated |
| bmp | ✅ Have | test_generated.bmp | small | Generated yellow |
| webp | ✅ Have | file_example_WEBP_250kB.webp | 250 KB | |
| webp | ✅ Have | test_generated.webp | small | Generated gradient |
| tiff/tif | ✅ Have | file_example_TIFF_1MB.tiff | 1 MB | Large, consider smaller |
| tiff/tif | ✅ Have | test_pattern.tiff | small | Generated checkerboard |
| ico | ✅ Have | file_example_favicon.ico | small | |
| svg | ✅ Have | file_example_SVG_30kB.svg | 30 KB | Text-based, no carver |
| heic | ❌ Missing | - | - | Need to source |
| raw (cr2/nef/arw/dng) | ❌ Missing | - | - | Low priority |

---

## Audio / Music

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| mp3 | ✅ Have | file_example_MP3_1MG.mp3 | 1 MB | |
| wav | ✅ Have | file_example_WAV_1MG.wav | 1 MB | |
| ogg | ✅ Have | file_example_OOG_1MG.ogg | 1 MB | |
| aac | ❌ Missing | - | - | Generate with ffmpeg |
| flac | ❌ Missing | - | - | Generate with ffmpeg |
| m4a | ❌ Missing | - | - | Generate with ffmpeg |

---

## Video

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| mp4 | ✅ Have | file_example_MP4_640_3MG.mp4 | 3 MB | Large! |
| avi | ✅ Have | file_example_AVI_480_750kB.avi | 750 KB | |
| mov | ✅ Have | file_example_MOV_640_800kB.mov | 800 KB | |
| webm | ✅ Have | file_example_WEBM_640_1_4MB.webm | 1.4 MB | |
| ogg (video) | ✅ Have | file_example_OGG_640_2_7mg.ogg | 2.7 MB | Large! |
| wmv | ✅ Have | file_example_WMV_640_1_6MB.wmv | 1.6 MB | |
| mkv | ❌ Missing | - | - | Generate with ffmpeg |
| flv | ❌ Missing | - | - | Low priority (legacy) |

---

## Documents (Office & Text)

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| pdf | ✅ Have | file-sample_150kB.pdf | 150 KB | |
| pdf | ✅ Have | test_minimal.pdf | small | Generated minimal PDF |
| doc | ✅ Have | file-sample_100kB.doc | 100 KB | OLE format |
| docx | ✅ Have | file-sample_100kB.docx | 100 KB | ZIP-based |
| xls | ✅ Have | file_example_XLS_50.xls | 50 KB | OLE format |
| xlsx | ✅ Have | file_example_XLSX_100.xlsx | 100 KB | ZIP-based |
| ppt | ✅ Have | file_example_PPT_250kB.ppt | 250 KB | OLE format |
| pptx | ✅ Have | test.pptx | small | Generated |
| odt | ✅ Have | file-sample_100kB.odt | 100 KB | ZIP-based |
| ods | ✅ Have | file_example_ODS_10.ods | 10 KB | ZIP-based |
| odp | ✅ Have | file_example_ODP_200kB.odp | 200 KB | ZIP-based |
| rtf | ✅ Have | file-sample_100kB.rtf | 100 KB | Text-based |
| txt | ✅ Have | documents/source.txt | small | Basic text sample |
| csv | ✅ Have | file_example_CSV_5000.csv | ~5 KB | |
| json | ✅ Have | file_example_JSON_1kb.json | 1 KB | |
| xml | ✅ Have | file_example_XML_24kb.xml.xml | 24 KB | |
| yaml | ✅ Have | other/test_config.yaml | small | Generated |
| html | ✅ Have | Title.html | small | |

---

## Archives / Containers

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| zip | ✅ Have | zip_2MB.zip | 2 MB | Large! |
| zip (nested) | ✅ Have | nested.zip | small | ZIP with inner ZIP |
| rar | ✅ Have | test.rar | small | RAR4? add RAR5 sample |
| rar (v5) | ❌ Missing | test.rar5 | small | Generate with `rar a -ma5 test.rar5 *.jpg` |
| 7z | ✅ Have | test.7z | small | |
| tar | ✅ Have | test.tar | small | |
| tar.gz | ✅ Have | test.tar.gz | small | |
| tar.bz2 | ✅ Have | test.tar.bz2 | small | |
| tar.xz | ✅ Have | test.tar.xz | small | |
| gz | ✅ Have | test.txt.gz | small | |
| bz2 | ✅ Have | test.txt.bz2 | small | |
| xz | ✅ Have | test.txt.xz | small | |
| iso | ❌ Missing | - | - | Low priority |

---

## Databases / Structured Data

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| sqlite | ✅ Have | databases/test.sqlite | small | |
| sqlite | ✅ Have | databases/test_forensic.sqlite | small | Users/logs/bookmarks tables |
| mdb/accdb | ❌ Missing | - | - | Low priority |

---

## eBooks (Bonus - you have these!)

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| epub | ✅ Have | sample1.epub | ? | ZIP-based |
| azw3 | ✅ Have | sample1.azw3 | ? | Kindle format |
| fb2 | ✅ Have | sample1.fb2 | ? | XML-based |
| lrf | ✅ Have | sample1.lrf | ? | Sony Reader |

---

## Executables / Binaries

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| exe | ❌ Missing | - | - | Get minimal PE |
| dll | ❌ Missing | - | - | Get minimal DLL |
| elf | ✅ Have | binaries/test_elf | small | Generated |
| so | ✅ Have | binaries/libtest.so | small | Generated |
| apk | ❌ Missing | - | - | Low priority |

---

## Email / Messaging

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| eml | ✅ Have | email/test_simple.eml | small | Generated |
| eml | ✅ Have | email/test_with_attachment.eml | small | With base64 attachment |
| msg | ❌ Missing | - | - | Hard to create |
| pst | ❌ Missing | - | - | Hard to create |
| mbox | ❌ Missing | - | - | Create manually |

---

## Windows Artefacts

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| lnk | ✅ Have | windows/lnk/*.lnk | various | Shell link files |
| prefetch (v17 XP) | ✅ Have | windows/prefetch/NOTEPAD.EXE-ABCD1234.pf | 136 B | Uncompressed SCCA |
| prefetch (v23 Vista/7) | ✅ Have | windows/prefetch/NOTEPAD.EXE-B1234567.pf | 216 B | Uncompressed SCCA |
| prefetch (v26 Win8) | ✅ Have | windows/prefetch/NOTEPAD.EXE-C2345678.pf | 216 B | Uncompressed SCCA |
| prefetch (v30 Win10 plain) | ✅ Have | windows/prefetch/NOTEPAD.EXE-D3456789.pf | 272 B | Uncompressed SCCA |
| prefetch (v30 Win10 MAM) | ✅ Have | windows/prefetch/CMD.EXE-E4567890.pf | 536 B | MAM/LZXPRESS Huffman |
| registry hive (NTUSER.DAT) | ✅ Have | windows/registry/NTUSER.DAT.synthetic | 8 KB | Synthetic regf+hbin, valid checksum |
| registry hive (SYSTEM) | ✅ Have | windows/registry/SYSTEM.synthetic | 8 KB | Synthetic regf+hbin, valid checksum |
| registry hive (SOFTWARE) | ✅ Have | windows/registry/SOFTWARE.synthetic | 8 KB | Synthetic regf+hbin, valid checksum |
| registry hive (SAM) | ✅ Have | windows/registry/SAM.synthetic | 8 KB | Synthetic regf+hbin, valid checksum |
| registry hive (dirty) | ✅ Have | windows/registry/DIRTY.synthetic | 8 KB | Mismatched primary/secondary sequence |
| registry hive (real BBI) | ✅ Have | windows/registry/BBI | 256 KB | Real Win10 hive, scrubbed VM |
| registry hive (real BCD-Template) | ✅ Have | windows/registry/BCD-Template | 28 KB | Real Win10 hive, scrubbed VM |
| registry hive (real BCD-Template.LOG) | ✅ Have | windows/registry/BCD-Template.LOG | 28 KB | Real .LOG file (regf magic) |
| registry hive (real DEFAULT) | ✅ Have | windows/registry/DEFAULT | 768 KB | Real Win10 hive, scrubbed VM |
| registry hive (real ELAM) | ✅ Have | windows/registry/ELAM | 8 KB | Real Win10 hive, scrubbed VM |
| registry hive (real NTUSER.DAT) | ✅ Have | windows/registry/NTUSER.DAT | 1 MB | Real user hive, account=`sample_account` |
| registry hive (real SAM) | ✅ Have | windows/registry/SAM | 64 KB | Real Win10 hive, scrubbed VM |
| registry hive (real SECURITY) | ✅ Have | windows/registry/SECURITY | 32 KB | Real Win10 hive, scrubbed VM |
| registry hive (real SYSTEM) | ✅ Have | windows/registry/SYSTEM | 12 MB | Real Win10 hive, exercises large-hive paths |
| registry hive (real userdiff) | ✅ Have | windows/registry/userdiff | 8 KB | Real Win10 hive, scrubbed VM |
| evtx | ❌ Missing | - | - | Future carver |

---

## Browser Artefacts

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| Chrome History | ✅ Have | History | small | SQLite |
| Chrome Cookies | ✅ Have | Cookies | small | SQLite |
| Firefox places.sqlite | ✅ Have | places.sqlite | small | |

---

## Other / Strings

| Format | Status | File | Size | Notes |
|--------|--------|------|------|-------|
| strings.txt | ✅ Have | other/strings.txt | small | URLs/emails/phones |
| forensic patterns | ✅ Have | other/forensic_patterns.txt | small | Emails/URLs/IPs/CCs/hashes |
| utf8 multilingual | ✅ Have | other/utf8_multilingual.txt | small | Multi-language text |
| utf16 LE | ✅ Have | other/utf16_le.txt | small | UTF-16 Little Endian |
| utf16 BE | ✅ Have | other/utf16_be.txt | small | UTF-16 Big Endian |
| json | ✅ Have | other/test_data.json | small | Generated JSON |
| jsonl/ndjson | ✅ Have | other/test_logs.jsonl | small | Log format |
| yaml | ✅ Have | other/test_config.yaml | small | Config file |

---

### Files to Add (High Priority)

- `test.rar5` - RAR5 format coverage (generate with `rar a -ma5`)
- `test_exif.jpg` - JPEG with EXIF/GPS metadata (generate with exiftool)
- `test_encrypted.rar` - Encrypted RAR for graceful skip testing

### Recently Generated ✅

- `windows/prefetch/NOTEPAD.EXE-ABCD1234.pf` — v17 (XP), uncompressed SCCA, 136 bytes
- `windows/prefetch/NOTEPAD.EXE-B1234567.pf` — v23 (Vista/7), uncompressed SCCA, 216 bytes
- `windows/prefetch/NOTEPAD.EXE-C2345678.pf` — v26 (Win8), uncompressed SCCA, 216 bytes
- `windows/prefetch/NOTEPAD.EXE-D3456789.pf` — v30 (Win10), uncompressed SCCA, 272 bytes
- `windows/prefetch/CMD.EXE-E4567890.pf` — v30 (Win10), MAM/LZXPRESS Huffman compressed, 536 bytes
- `nested.zip` - ZIP containing other files for nested carving
- `utf16_le.txt` / `utf16_be.txt` - UTF-16 string scan coverage
- `utf8_multilingual.txt` - Multi-language UTF-8 text
- `forensic_patterns.txt` - Emails/URLs/IPs/credit cards/hashes
- `test_data.json` / `test_logs.jsonl` - JSON test files
- `test_config.yaml` - YAML configuration
- `test_simple.eml` / `test_with_attachment.eml` - Email files
- `test_forensic.sqlite` - SQLite with users/logs/bookmarks
- `test_elf` / `libtest.so` - ELF binaries
- `test_generated.*` - Various generated images
- `test_minimal.pdf` - Minimal valid PDF

---

## Files to Generate (Self-Created Test Data)

All test files should be generated ourselves to avoid licensing concerns. Below are generation commands.

### Archive Files

```bash
# RAR5 archive (requires rar/unrar package)
rar a -ma5 test.rar5 some_files/

# RAR4 archive  
rar a -ma4 test.rar4 some_files/

# Encrypted RAR (for skip testing)
rar a -hp"password123" test_encrypted.rar some_files/

# ZIP with nested content
zip -r nested.zip folder_with_images/

# Encrypted ZIP
zip -e -P "password123" test_encrypted.zip some_files/
```

### Images with EXIF/GPS Metadata

```bash
# JPEG with EXIF GPS data
convert -size 640x480 plasma:fractal \
  -set EXIF:GPSLatitude "37/1,46/1,26/1" \
  -set EXIF:GPSLatitudeRef "N" \
  -set EXIF:GPSLongitude "122/1,25/1,9/1" \
  -set EXIF:GPSLongitudeRef "W" \
  -set EXIF:Make "TestCamera" \
  -set EXIF:Model "TestModel" \
  -set EXIF:DateTimeOriginal "2025:01:01 12:00:00" \
  test_exif.jpg

# TIFF with metadata
convert -size 320x240 gradient:blue-red \
  -set EXIF:Make "TestCamera" \
  test_meta.tiff

# PNG (no EXIF but can have text chunks)
convert -size 200x200 xc:green -set png:Title "Test PNG" test_meta.png
```

### Text Files with Various Encodings

```bash
# UTF-16 LE text
echo "Test UTF-16 string with special chars: äöü 你好 🎉" | iconv -t UTF-16LE > utf16_le.txt

# UTF-16 BE text  
echo "Test UTF-16 BE string" | iconv -t UTF-16BE > utf16_be.txt

# UTF-8 multilingual text
cat > utf8_multilingual.txt << 'EOF'
English: Hello World
German: Größe, Äpfel, Übung
Russian: Привет мир
Chinese: 你好世界
Japanese: こんにちは
Arabic: مرحبا بالعالم
Emoji: 🎉🔥💻🚀
EOF

# Text with forensic patterns (emails, URLs, IPs)
cat > forensic_strings.txt << 'EOF'
Email addresses:
user@example.com
admin@test-domain.org
john.doe+tag@company.co.uk

URLs:
https://www.example.com/path?query=value
http://192.168.1.1:8080/admin
ftp://files.example.org/download.zip

IP addresses:
192.168.1.1
10.0.0.1
2001:0db8:85a3:0000:0000:8a2e:0370:7334

Phone numbers:
+1-555-123-4567
(202) 555-0123
+44 20 7946 0958

Credit card test numbers (Luhn-valid test numbers):
4111111111111111
5500000000000004
340000000000009

Bitcoin addresses (example format):
1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2
3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy
EOF
```

### PDF Files

```bash
# Simple PDF with text (using enscript + ps2pdf)
echo "This is a test PDF document for carving tests." | enscript -B -o - | ps2pdf - test_simple.pdf

# Or using pdflatex
cat > test_doc.tex << 'EOF'
\documentclass{article}
\begin{document}
Test PDF document for SwiftBeaver testing.
\end{document}
EOF
pdflatex test_doc.tex

# PDF with streams (more complex)
# Use LibreOffice to convert a doc to PDF with images
```

### SQLite Databases

```bash
# Basic SQLite database
sqlite3 test_basic.sqlite << 'EOF'
CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT);
INSERT INTO users VALUES (1, 'Alice', 'alice@example.com');
INSERT INTO users VALUES (2, 'Bob', 'bob@test.org');
CREATE TABLE logs (id INTEGER PRIMARY KEY, timestamp TEXT, action TEXT);
INSERT INTO logs VALUES (1, '2025-01-01 12:00:00', 'login');
INSERT INTO logs VALUES (2, '2025-01-01 12:05:00', 'logout');
EOF

# Browser-style history database
sqlite3 test_history.sqlite << 'EOF'
CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER);
INSERT INTO urls VALUES (1, 'https://www.google.com', 'Google', 100, 13350000000000000);
INSERT INTO urls VALUES (2, 'https://github.com', 'GitHub', 50, 13350000000000001);
CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER);
INSERT INTO visits VALUES (1, 1, 13350000000000000);
INSERT INTO visits VALUES (2, 2, 13350000000000001);
EOF
```

### Office Documents (ZIP-based)

```bash
# DOCX - use LibreOffice command line
echo "Test document content" > /tmp/test.txt
libreoffice --headless --convert-to docx /tmp/test.txt --outdir .

# Or create minimal DOCX manually (it's just a ZIP)
mkdir -p docx_tmp/word
cat > docx_tmp/word/document.xml << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:t>Test DOCX document</w:t></w:r></w:p></w:body>
</w:document>
EOF
cat > docx_tmp/[Content_Types].xml << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>
EOF
cd docx_tmp && zip -r ../test_generated.docx . && cd ..
rm -rf docx_tmp
```

### ELF Binaries (Linux)

```bash
# Minimal hello world
cat > hello.c << 'EOF'
int main() { return 0; }
EOF
gcc -o test_elf hello.c
rm hello.c

# Shared library
cat > libtest.c << 'EOF'
int test_func() { return 42; }
EOF
gcc -shared -fPIC -o libtest.so libtest.c
rm libtest.c
```

### Email Files

```bash
# Simple EML file
cat > test_email.eml << 'EOF'
From: sender@example.com
To: recipient@test.org
Subject: Test Email for Carving
Date: Wed, 01 Jan 2025 12:00:00 +0000
MIME-Version: 1.0
Content-Type: text/plain; charset="UTF-8"

This is a test email message for forensic carving tests.
It contains some sample text content.

Best regards,
Test Sender
EOF

# EML with base64 attachment
cat > test_email_attachment.eml << 'EOF'
From: sender@example.com
To: recipient@test.org
Subject: Test Email with Attachment
Date: Wed, 01 Jan 2025 12:00:00 +0000
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="----=_Part_0"

------=_Part_0
Content-Type: text/plain; charset="UTF-8"

This email has an attachment.

------=_Part_0
Content-Type: application/octet-stream; name="test.txt"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="test.txt"

VGhpcyBpcyBhIHRlc3QgYXR0YWNobWVudC4=

------=_Part_0--
EOF
```

### JSON Files

```bash
# Plain JSON
cat > test_data.json << 'EOF'
{
  "name": "Test User",
  "email": "test@example.com",
  "age": 30,
  "address": {
    "street": "123 Test Street",
    "city": "Test City",
    "country": "Testland"
  },
  "tags": ["forensics", "testing", "carving"]
}
EOF

# JSON log format (NDJSON)
cat > test_logs.jsonl << 'EOF'
{"timestamp": "2025-01-01T12:00:00Z", "level": "INFO", "message": "Application started"}
{"timestamp": "2025-01-01T12:00:01Z", "level": "DEBUG", "message": "Loading configuration"}
{"timestamp": "2025-01-01T12:00:02Z", "level": "WARN", "message": "Config file not found, using defaults"}
EOF
```

### Windows Artefacts (Future - requires Windows or specific tools)

```bash
# Windows shortcut (.lnk) - can be created with pylnk or on Windows
# For now, skip or find open-source samples

# Prefetch files - require Windows system access
# EVTX, MFT, USN - require Windows or forensic image extraction
```

---

## Generation Script

Save this as `generate_test_files.sh` and run to create all test files:

```bash
#!/bin/bash
set -e

OUTPUT_DIR="./generated_samples"
mkdir -p "$OUTPUT_DIR"
cd "$OUTPUT_DIR"

echo "Generating test files..."

# UTF-8 multilingual text
cat > utf8_multilingual.txt << 'HEREDOC'
English: Hello World
German: Größe, Äpfel, Übung  
Russian: Привет мир
Chinese: 你好世界
Japanese: こんにちは
Emoji: 🎉🔥💻
HEREDOC

# UTF-16 LE
echo "UTF-16 test string äöü" | iconv -t UTF-16LE > utf16_le.txt

# Forensic patterns
cat > forensic_strings.txt << 'HEREDOC'
user@example.com
https://www.example.com
192.168.1.1
+1-555-123-4567
4111111111111111
HEREDOC

# SQLite database
sqlite3 test.sqlite << 'SQL'
CREATE TABLE test (id INTEGER PRIMARY KEY, data TEXT);
INSERT INTO test VALUES (1, 'test data');
SQL

# Simple images (requires ImageMagick)
if command -v convert &> /dev/null; then
    convert -size 100x100 xc:red test_red.jpg
    convert -size 100x100 xc:blue test_blue.png
    convert -size 100x100 xc:green test_green.gif
    convert -size 100x100 xc:yellow test_yellow.bmp
fi

# JSON
echo '{"test": "data", "number": 42}' > test.json

echo "Done! Files created in $OUTPUT_DIR"
```

