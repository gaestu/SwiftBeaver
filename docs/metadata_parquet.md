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
- `is_duplicate` (bool)
- `duplicate_of_offset` (int64, nullable)

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

Chromium-based browsers (Chrome/Edge/Brave) share the same schema and may be labeled `chrome`.

Note: `sqlite_page` and `sqlite_wal` are carve-only file outputs and do not emit browser row metadata.

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

## Windows artefacts

`windows_artefacts.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `artefact_type` (string)
- `offset` (int64)
- `size` (int64)
- `target_path` (string, nullable)
- `working_dir` (string, nullable)
- `creation_time_utc` (timestamp micros, nullable)
- `access_time_utc` (timestamp micros, nullable)
- `write_time_utc` (timestamp micros, nullable)
- `file_size` (int64, nullable)
- `volume_serial` (string, nullable)
- `local_base_path` (string, nullable)
- `network_path` (string, nullable)
- `executable_name` (string, nullable)
- `prefetch_hash` (string, nullable)
- `run_count` (int64, nullable)
- `last_run_times_json` (string, nullable)
- `volume_paths_json` (string, nullable)
- `volume_paths_truncated` (boolean, nullable)
- `referenced_files_json` (string, nullable)
- `version` (int32, nullable)
- `first_chunk` (int64, nullable)
- `last_chunk` (int64, nullable)
- `record_count_estimate` (int64, nullable) — for EVTX rows, also `null` when chunk headers declare a record-number range that cannot be represented as `int64` (e.g. `last_record_number == u64::MAX`); `null` for non-EVTX artefact types
- `log_name` (string, nullable)
- `timestamp_utc` (timestamp micros, nullable)
- `hive_name` (string, nullable)
- `hive_type` (string, nullable)
- `root_key_name` (string, nullable)

Variant-specific fields are nullable. Array-like Prefetch fields are stored as JSON strings.
For `volume_paths_truncated`, `true` means the on-disk Prefetch header claimed more volume entries than SwiftBeaver decoded under the defensive cap; `false` means the emitted `volume_paths_json` reflects all decoded entries.
For `referenced_files_json`, `null` means extraction is not implemented for that record yet; `"[]"` means extraction ran and found no references.

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
- `files_rejected` (int64)
- `files_prevalidation_rejected` (int64)
- `overlap_skipped` (int64)
- `string_spans` (int64)
- `artefacts_extracted` (int64)
- `duplicates_found` (int64)
- `duplicates_skipped` (int64)

`files_prevalidation_rejected` counts hits rejected before file creation by lightweight carver checks. `overlap_skipped` counts same-type hits skipped because they landed inside a range already carved by that worker.

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
