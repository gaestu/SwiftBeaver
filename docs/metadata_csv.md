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

Known `artefact_kind` values: `url`, `email`, `phone`, `bitlocker_recovery_password`, `string`. For `bitlocker_recovery_password`, `content` is the canonical hyphen-separated form (`XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX`).

## bitlocker_bek.csv

Columns:

- `run_id`
- `global_start`
- `global_end`
- `size`
- `carved_path`
- `key_identifier_guid`
- `description`
- `key_data_length`
- `key_encryption_method`
- `modification_filetime`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

This category covers binary BitLocker External Key (`.bek`) files only. Textual 48-digit BitLocker recovery passwords remain in `string_artefacts.csv`, and BitLocker key packages (`.KPG`) are out of scope.

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
- `volume_paths_truncated`
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
For `volume_paths_truncated`, `true` means the on-disk Prefetch header claimed more volume entries than SwiftBeaver decoded under the defensive cap; `false` means the emitted `volume_paths_json` reflects all decoded entries.
For `referenced_files_json`, `null` means extraction is not implemented for that record yet; `"[]"` means extraction ran and found no references.

## run_summary.csv

Columns:

- `run_id`
- `bytes_scanned`
- `chunks_processed`
- `hits_found`
- `files_carved`
- `files_rejected`
- `files_prevalidation_rejected`
- `files_capped`
- `overlap_skipped`
- `string_spans`
- `artefacts_extracted`
- `duplicates_found`
- `duplicates_skipped`
- `phone_like_spans_scanned`
- `phone_regex_candidates`
- `phone_prefilter_rejections`
- `phone_rejected_digit_only`
- `phone_rejected_low_entropy`
- `phone_rejected_bad_context`
- `phone_rejected_no_region`
- `phone_rejected_invalid`
- `phone_validation_calls`
- `phone_validated_rows`
- `phone_exact_duplicates_omitted`
- `phone_occurrences_capped`
- `phone_distinct_normalized_values`
- `phone_repeated_normalized_values`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

`files_prevalidation_rejected` counts hits rejected before file creation by lightweight carver checks. `overlap_skipped` counts fully-carved files discarded by the streaming overlap arbiter because their final byte range `[global_start, global_end]` intersected a range already accepted for the same `file_type`. `files_capped` counts otherwise accepted carves discarded after `max_files` is reached. Phone counters cover validated phone extraction from string spans, including candidate counts, rejection classes, validation calls, accepted rows, exact same-offset duplicate omissions, occurrence rows omitted after the duplicate-tracking memory cap, distinct normalized values, and repeated normalized values across different offsets. Arbitration follows deterministic evidence order by signature-hit offset, then `file_type` and `pattern_id`; overlap checks use the final carved ranges reported by each carver.

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
