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

## run_summary.csv

Columns:

- `run_id`
- `bytes_scanned`
- `chunks_processed`
- `hits_found`
- `files_carved`
- `string_spans`
- `artefacts_extracted`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`

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
