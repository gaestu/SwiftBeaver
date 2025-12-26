# Configuration

The default config is `config/default.yml`.

## Top-level fields

- `run_id` (string): optional; if empty, a timestamp-based ID is generated.
- `overlap_bytes` (u64): overlap between chunks.
- `enable_string_scan` (bool): enable printable string scanning.
- `string_min_len` (usize): minimum printable string length.
- `string_max_len` (usize): maximum string length per span.
- `gpu_max_hits_per_chunk` (usize): maximum GPU hits per chunk (overflow truncates).
- `parquet_row_group_size` (usize): max rows per Parquet row group.
- `opencl_platform_index` (usize, optional): select OpenCL platform by index.
- `opencl_device_index` (usize, optional): select OpenCL device by index.
- `file_types` (list): enabled file types and patterns.

Note: ZIP carving will classify docx/xlsx/pptx based on central directory entries when present.

## File type configuration

Each entry in `file_types` contains:

- `id`: identifier (e.g. `jpeg`, `png`, `gif`)
- `extensions`: list of output extensions
- `header_patterns`: signature patterns used by the scanner
- `footer_patterns`: reserved for future use
- `max_size`: maximum carve size in bytes
- `min_size`: minimum carve size in bytes
- `validator`: logical handler name
- `require_eocd`: optional; for ZIP, require an EOCD before carving (prevents large false positives)

## Example

```yaml
run_id: ""
overlap_bytes: 65536
enable_string_scan: false
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
```
