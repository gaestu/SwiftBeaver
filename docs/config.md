# Configuration

The default config is `config/default.yml`.

## Top-level fields

- `run_id` (string): optional; if empty, a timestamp-based ID is generated.
- `overlap_bytes` (u64): overlap between chunks.
- `max_files` (u64, optional): strict cap on carved files; the pipeline stops once the limit is reached.
- `max_memory_mib` (u64, optional): limit address space in MiB (Unix only).
- `max_open_files` (u64, optional): limit max open file descriptors (Unix only).
- `enable_string_scan` (bool): enable ASCII/UTF-8 printable string scanning.
- `enable_url_scan` (bool): enable URL extraction from string spans.
- `enable_email_scan` (bool): enable email extraction from string spans.
- `enable_phone_scan` (bool): enable phone extraction from string spans.
- `string_scan_utf16` (bool): enable UTF-16LE/BE printable string scanning.
- `string_min_len` (usize): minimum printable string length.
- `string_max_len` (usize): maximum string length per span.
- `gpu_max_hits_per_chunk` (usize): maximum GPU hits per chunk (overflow truncates).
- `gpu_max_string_spans_per_chunk` (usize): maximum GPU ASCII string spans per chunk (overflow truncates).
- `parquet_row_group_size` (usize): max rows per Parquet row group.
- `enable_entropy_detection` (bool): enable entropy region detection.
- `entropy_window_size` (usize): window size (bytes) used for entropy calculation.
- `entropy_threshold` (float): entropy threshold for marking high-entropy regions.
- `sqlite_page_max_hits_per_chunk` (usize): cap for `sqlite_page` scanner hits per chunk to limit single-byte marker overload.
- `sqlite_wal_max_consecutive_checksum_failures` (u32): maximum consecutive WAL frames allowed to fail full rolling checksum validation before carving stops. This controls stop behavior, not frame filtering; mismatching frames observed before the stop threshold may still be included in carved bytes. Set to `0` to stop at the first checksum mismatch.
- `opencl_platform_index` (usize, optional): select OpenCL platform by index.
- `opencl_device_index` (usize, optional): select OpenCL device by index.
- `zip_allowed_kinds` (list, optional): restrict ZIP outputs to `zip`, `docx`, `xlsx`, `pptx`, `odt`, `ods`, `odp`, `epub` when set.
- `ole_allowed_kinds` (list, optional): restrict OLE outputs to `doc`, `xls`, `ppt` when set.
- `quicktime_mode` (string): handling for QuickTime; `mov` (default) keeps MOV separate, `mp4` treats QuickTime as MP4.
- `file_types` (list): enabled file types and patterns.

Note: ZIP carving will classify docx/xlsx/pptx/odt/ods/odp/epub based on central directory entries when present.
Note: `sqlite_page` and `sqlite_wal` are carve-only outputs; enable/disable them via `file_types` and CLI type filters (`--types` / `--enable-types`).

## File type configuration

Each entry in `file_types` contains:

- `id`: identifier (e.g. `jpeg`, `png`, `gif`)
- `extensions`: list of output extensions
- `header_patterns`: signature patterns used by the scanner
- `footer_patterns`: footer signatures used by the `footer` validator
- `max_size`: maximum carve size in bytes
- `min_size`: minimum carve size in bytes
- `validator`: handler name (`jpeg`, `png`, `gif`, `sqlite`, `sqlite_wal`, `sqlite_page`, `pdf`, `zip`, `webp`, `bmp`, `tiff`, `mp4`, `mov`, `rar`, `sevenz`, `wav`, `avi`, `mp3`, `ole`, `tar`, `gzip`, `bzip2`, `xz`, `ogg`, `webm`, `wmv`, `rtf`, `ico`, `elf`, `eml`, `mobi`, `fb2`, `lrf`, `footer`)
- `require_eocd`: optional; for ZIP, require an EOCD before carving (prevents large false positives)

The `footer` validator performs a simple header-to-footer carve for formats without a dedicated handler.

## Example

```yaml
run_id: ""
overlap_bytes: 65536
enable_string_scan: false
string_scan_utf16: false
file_types:
  - id: "jpeg"
    extensions: ["jpg", "jpeg"]
    header_patterns:
      - id: "jpeg_soi"
        hex: "FFD8FF"
    footer_patterns: []
    max_size: 104857600
    min_size: 16
    validator: "jpeg"
  - id: "sqlite"
    extensions: ["sqlite"]
    header_patterns:
      - id: "sqlite_header"
        hex: "53514C69746520666F726D6174203300"
    footer_patterns: []
    max_size: 536870912
    min_size: 100
    validator: "sqlite"
  - id: "pdf"
    extensions: ["pdf"]
    header_patterns:
      - id: "pdf_header"
        hex: "255044462D"
    footer_patterns: []
    max_size: 104857600
    min_size: 64
    validator: "pdf"
  - id: "zip"
    extensions: ["zip"]
    header_patterns:
      - id: "zip_header"
        hex: "504B0304"
    footer_patterns: []
    max_size: 104857600
    min_size: 32
    validator: "zip"
  - id: "webp"
    extensions: ["webp"]
    header_patterns:
      - id: "webp_header"
        hex: "52494646"
    footer_patterns: []
    max_size: 104857600
    min_size: 20
    validator: "webp"
  - id: "bmp"
    extensions: ["bmp"]
    header_patterns:
      - id: "bmp_header"
        hex: "424D"
    footer_patterns: []
    max_size: 104857600
    min_size: 54
    validator: "bmp"
  - id: "tiff"
    extensions: ["tiff", "tif"]
    header_patterns:
      - id: "tiff_le_header"
        hex: "49492A00"
      - id: "tiff_be_header"
        hex: "4D4D002A"
    footer_patterns: []
    max_size: 104857600
    min_size: 8
    validator: "tiff"
  - id: "mp4"
    extensions: ["mp4"]
    header_patterns:
      - id: "mp4_ftyp_18"
        hex: "0000001866747970"
      - id: "mp4_ftyp_1c"
        hex: "0000001C66747970"
      - id: "mp4_ftyp_20"
        hex: "0000002066747970"
    footer_patterns: []
    max_size: 1073741824
    min_size: 16
    validator: "mp4"
  - id: "rar"
    extensions: ["rar"]
    header_patterns:
      - id: "rar4_header"
        hex: "526172211A0700"
      - id: "rar5_header"
        hex: "526172211A070100"
    footer_patterns: []
    max_size: 1073741824
    min_size: 32
    validator: "rar"
  - id: "7z"
    extensions: ["7z"]
    header_patterns:
      - id: "7z_header"
        hex: "377ABCAF271C"
    footer_patterns: []
    max_size: 1073741824
    min_size: 32
    validator: "sevenz"
```
