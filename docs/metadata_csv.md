# CSV Metadata Schema (Phase 2)

CSV output is enabled with `--metadata-backend csv`.

## carved_files.csv

Columns:

- `run_id`
- `file_type`
- `path`
- `extension`
- `global_start`
- `global_end`
- `size`
- `md5`
- `sha256`
- `validated`
- `truncated`
- `errors`
- `pattern_id`
- `is_duplicate`
- `duplicate_of_offset`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

## string_artefacts.csv

Columns:

- `run_id`
- `artefact_kind`
- `content`
- `encoding`
- `global_start`
- `global_end`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

## browser_history.csv

Columns:

- `run_id`
- `browser`
- `profile`
- `url`
- `title`
- `visit_time`
- `visit_source`
- `source_file`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

Chromium-based browsers (Chrome/Edge/Brave) share the same schema and may be labeled `chrome`.

Note: `sqlite_page` and `sqlite_wal` are carve-only file outputs and do not emit browser row metadata.

## browser_cookies.csv

Columns:

- `run_id`
- `browser`
- `profile`
- `host`
- `name`
- `value`
- `path`
- `expires_utc`
- `last_access_utc`
- `creation_utc`
- `is_secure`
- `is_http_only`
- `source_file`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

## browser_downloads.csv

Columns:

- `run_id`
- `browser`
- `profile`
- `url`
- `target_path`
- `start_time`
- `end_time`
- `total_bytes`
- `state`
- `source_file`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

Chromium-based browsers (Chrome/Edge/Brave) share the same schema and may be labeled `chrome`.

## windows_artefacts.csv

Columns:

- `run_id`
- `artefact_type`
- `offset`
- `size`
- `target_path`
- `working_dir`
- `creation_time`
- `access_time`
- `write_time`
- `file_size`
- `volume_serial`
- `local_base_path`
- `network_path`
- `executable_name`
- `prefetch_hash`
- `run_count`
- `last_run_times_json`
- `volume_paths_json`
- `referenced_files_json`
- `version`
- `first_chunk`
- `last_chunk`
- `record_count_estimate`
- `log_name`
- `timestamp`
- `hive_name`
- `hive_type`
- `root_key_name`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

Variant-specific fields are nullable. Array-like Prefetch fields are serialized as JSON strings.

## run_summary.csv

Columns:

- `run_id`
- `bytes_scanned`
- `chunks_processed`
- `hits_found`
- `files_carved`
- `files_rejected`
- `files_prevalidation_rejected`
- `overlap_skipped`
- `string_spans`
- `artefacts_extracted`
- `duplicates_found`
- `duplicates_skipped`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

`files_prevalidation_rejected` counts hits rejected before file creation by lightweight carver checks. `overlap_skipped` counts same-type hits skipped because they landed inside a range already carved by that worker.

## entropy_regions.csv

Columns:

- `run_id`
- `global_start`
- `global_end`
- `entropy`
- `window_size`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`
