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
- `deferred_buffer_kb` (usize): kilobytes to buffer in memory before creating an output file on disk. Deferred creation avoids the create-write-delete I/O cycle for candidates that fail structural validation during carving. Set to `0` to disable deferral (eager file creation, matching pre-deferred behavior). Default: `64`.
- `ewf_cache_segments` (usize): number of 64 KiB segments to cache in the EWF read-through LRU cache. Each segment consumes 64 KiB of memory. Default: `4096` (256 MiB total cache). Set to `0` to disable caching.
- `ewf_reader_handles` (usize): number of independent libewf handles to open for parallel EWF decompression. Each handle decompresses independently, enabling concurrent reads from different pipeline threads. Default: `0` (auto-detect: `min(4, max(2, num_cpus/4))`). Set to `1` to disable pooling and use a single serialized handle.
- `hash_algorithms` (list of strings): hash algorithms to compute for each carved file. Supported: `md5`, `sha256`. Default: `["md5", "sha256"]`. Unknown names produce a warning and are ignored.
- `enable_deduplication` (bool): enable deduplication tracking based on SHA256 hashes. When enabled, each carved file's metadata includes `is_duplicate` and `duplicate_of_offset` fields. Requires `sha256` in `hash_algorithms` (added automatically if missing). Default: `false`. Note: the total number of duplicates is deterministic, but which specific file is marked as the "original" versus "duplicate" may vary between runs due to parallel processing order.
- `skip_duplicate_files` (bool): skip writing duplicate files to disk when deduplication is enabled. Metadata is still recorded for all files. Requires `enable_deduplication`. Default: `false`. Note: when enabled, duplicate detection uses zero-write dedup — SHA-256 hashes are computed during the carve (validation) phase before any disk I/O, so duplicate files are discarded without ever being written to disk.
- `fast_carve_worker_ratio` (f64): fraction of carve workers assigned to the fast queue (0.0–1.0). Fast carvers (BMP, ICO, ELF, TIFF, WebP, HEIC, MOV, LRF, MOBI, WMV, Bzip2, Gzip) are routed to a separate worker pool so they are not blocked behind slow, I/O-heavy carvers (SQLite, MP3, PDF, etc.). When `workers < 2`, all hits go through a single pool. Default: `0.25`.
- `write_workers` (usize): number of dedicated I/O writer threads for flushing carved files to disk. Carve workers perform CPU-bound validation, parsing and hashing, then hand off the validated result to writer threads for disk I/O. Fewer writer threads are typically needed compared to carve workers, since SSDs can saturate with limited parallelism. Default: `4`.
- `carver_limits` (map): per-carver concurrency limits. Each key is a carver type ID (e.g. `sqlite`, `mp3`), value is an object with `max_concurrent` (optional usize) specifying the maximum number of slow carve workers that may process this type simultaneously. Default: `{}` (unlimited for all types).
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
