# Parquet metadata

Parquet output is enabled via `--metadata-backend parquet`. Files are written under
`<run_dir>/parquet/` with one file per category.

## Files

Per-type files (examples):

- `files_jpeg.parquet`
- `files_png.parquet`
- `files_gif.parquet`
- `files_sqlite.parquet`
- `files_pdf.parquet`
- `files_zip.parquet`
- `files_webp.parquet`
- `files_other.parquet` (fallback for unknown types)

Schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `handler_id` (string)
- `file_type` (string)
- `carved_path` (string)
- `global_start` (int64)
- `global_end` (int64)
- `size` (int64)
- `md5` (string, nullable)
- `sha256` (string, nullable)
- `pattern_id` (string, nullable)
- `magic_bytes` (binary, nullable)
- `validated` (bool)
- `truncated` (bool)
- `error` (string, nullable)

## String artefacts

- `artefacts_urls.parquet`
- `artefacts_emails.parquet`
- `artefacts_phones.parquet`

URL schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `global_start` (int64)
- `global_end` (int64)
- `url` (string)
- `scheme` (string)
- `host` (string)
- `port` (int32, nullable)
- `path` (string, nullable)
- `query` (string, nullable)
- `fragment` (string, nullable)
- `source_kind` (string)
- `source_detail` (string)
- `certainty` (float64)

Email schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `global_start` (int64)
- `global_end` (int64)
- `email` (string)
- `local_part` (string)
- `domain` (string)
- `source_kind` (string)
- `source_detail` (string)
- `certainty` (float64)

Phone schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `global_start` (int64)
- `global_end` (int64)
- `phone_raw` (string)
- `phone_e164` (string, nullable)
- `country` (string, nullable)
- `source_kind` (string)
- `source_detail` (string)
- `certainty` (float64)

## Browser history

`browser_history.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `source_file` (string)
- `browser` (string)
- `profile` (string)
- `url` (string)
- `title` (string, nullable)
- `visit_time_utc` (timestamp micros, nullable)
- `visit_source` (string, nullable)
- `row_id` (int64, nullable)
- `table_name` (string, nullable)

Page-level recovery emits `browser="sqlite_page"` and `visit_source="page_scan"` with best-effort `title` and `visit_time_utc`.
Chromium-based browsers (Chrome/Edge/Brave) share the same schema and may be labeled `chrome`.

## Browser cookies

`browser_cookies.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `source_file` (string)
- `browser` (string)
- `profile` (string)
- `host` (string)
- `name` (string)
- `value` (string, nullable)
- `path` (string, nullable)
- `expires_utc` (timestamp micros, nullable)
- `last_access_utc` (timestamp micros, nullable)
- `creation_utc` (timestamp micros, nullable)
- `is_secure` (bool, nullable)
- `is_http_only` (bool, nullable)

## Browser downloads

`browser_downloads.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `source_file` (string)
- `browser` (string)
- `profile` (string)
- `url` (string, nullable)
- `target_path` (string, nullable)
- `start_time_utc` (timestamp micros, nullable)
- `end_time_utc` (timestamp micros, nullable)
- `total_bytes` (int64, nullable)
- `state` (string, nullable)

Chromium-based browsers (Chrome/Edge/Brave) share the same schema and may be labeled `chrome`.

## Run summary

`run_summary.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `bytes_scanned` (int64)
- `chunks_processed` (int64)
- `hits_found` (int64)
- `files_carved` (int64)
- `string_spans` (int64)
- `artefacts_extracted` (int64)

## Entropy regions

`entropy_regions.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `global_start` (int64)
- `global_end` (int64)
- `entropy` (float64)
- `window_size` (int64)
