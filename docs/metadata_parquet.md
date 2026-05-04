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
- `artefacts_phones_summary.parquet`
- `artefacts_bitlocker_recovery_passwords.parquet`

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

Phone rows are validated-only. `phone_raw` preserves the matched source text;
`phone_e164` stores the normalized E.164 value and `country` stores the inferred
region for accepted rows.

Phone summary schema (`artefacts_phones_summary.parquet`):

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `normalized_phone` (string)
- `occurrence_count` (int64)
- `first_global_start` (int64)
- `last_global_start` (int64)
- `country` (string)
- `validation_status` (string)

Summary rows group accepted occurrence rows by normalized phone value. Repeated
values at different evidence offsets remain in `artefacts_phones.parquet`; exact
same-offset processing duplicates are omitted from occurrence output and counted
in run-summary phone metrics.

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

BitLocker recovery password schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `global_start` (int64)
- `global_end` (int64)
- `recovery_password` (string, canonical hyphen-separated form `XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX-XXXXXX`)
- `encoding` (string, e.g. `ascii`, `utf-8`, `utf-16le`, `utf-16be`)
- `source_kind` (string)
- `source_detail` (string)
- `certainty` (float64)

This category covers textual BitLocker recovery passwords found in extracted
string spans only. `.bek` recovery key files and BitLocker key packages are
out of scope for this output.

## BitLocker BEK artefacts

`artefacts_bitlocker_bek.parquet` schema:

- `run_id` (string)
- `tool_version` (string)
- `config_hash` (string)
- `evidence_path` (string)
- `evidence_sha256` (string)
- `global_start` (int64)
- `global_end` (int64)
- `size` (int64)
- `carved_path` (string)
- `key_identifier_guid` (string)
- `description` (string, nullable)
- `key_data_length` (int64)
- `key_encryption_method` (int64)
- `modification_filetime` (uint64)

This category covers binary BitLocker External Key (`.bek`) files only. Textual
48-digit BitLocker recovery passwords remain in
`artefacts_bitlocker_recovery_passwords.parquet`, and BitLocker key packages
(`.KPG`) are out of scope.

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
- `files_capped` (int64)
- `overlap_skipped` (int64)
- `string_spans` (int64)
- `artefacts_extracted` (int64)
- `duplicates_found` (int64)
- `duplicates_skipped` (int64)
- `phone_like_spans_scanned` (int64)
- `phone_regex_candidates` (int64)
- `phone_prefilter_rejections` (int64)
- `phone_rejected_digit_only` (int64)
- `phone_rejected_low_entropy` (int64)
- `phone_rejected_bad_context` (int64)
- `phone_rejected_no_region` (int64)
- `phone_rejected_invalid` (int64)
- `phone_validation_calls` (int64)
- `phone_validated_rows` (int64)
- `phone_exact_duplicates_omitted` (int64)
- `phone_occurrences_capped` (int64)
- `phone_distinct_normalized_values` (int64)
- `phone_repeated_normalized_values` (int64)

`files_prevalidation_rejected` counts hits rejected before file creation by lightweight carver checks. `overlap_skipped` counts fully-carved files discarded by the streaming overlap arbiter because their final byte range `[global_start, global_end]` intersected a range already accepted for the same `file_type`. `files_capped` counts otherwise accepted carves discarded after `max_files` is reached. Phone counters cover validated phone extraction from string spans: scanned phone-like spans, regex candidates, prefilter and validation rejection classes, validation calls, accepted rows, omitted exact duplicates, occurrence rows omitted after the duplicate-tracking memory cap, distinct normalized values, and repeated normalized values across different offsets. Arbitration follows deterministic evidence order by signature-hit offset, then `file_type` and `pattern_id`; overlap checks use the final carved ranges reported by each carver.

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
