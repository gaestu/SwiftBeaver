# JSONL Metadata Schema (Phase 1)

Each line in `metadata/carved_files.jsonl` is a JSON object with:

- `run_id`
- `file_type`
- `path` (relative to `carved/`)
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

Example:

```json
{
  "run_id": "20250101T120000Z_00000001",
  "file_type": "jpeg",
  "path": "jpeg/jpeg_000000000400.jpg",
  "extension": "jpg",
  "global_start": 1024,
  "global_end": 1055,
  "size": 32,
  "md5": "...",
  "sha256": "...",
  "validated": true,
  "truncated": false,
  "errors": [],
  "pattern_id": "jpeg_soi",
  "tool_version": "0.2.0",
  "config_hash": "...",
  "evidence_path": "/cases/image.dd",
  "evidence_sha256": ""
}
```

## String artefacts (`string_artefacts.jsonl`)

Each line in `metadata/string_artefacts.jsonl` is a JSON object with:

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

## Browser history (`browser_history.jsonl`)

Each line in `metadata/browser_history.jsonl` is a JSON object with:

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

Page-level recovery emits `browser="sqlite_page"` and `visit_source="page_scan"` with best-effort `title` and `visit_time`.
Chromium-based browsers (Chrome/Edge/Brave) share the same schema and may be labeled `chrome`.

## Browser cookies (`browser_cookies.jsonl`)

Each line in `metadata/browser_cookies.jsonl` is a JSON object with:

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

## Browser downloads (`browser_downloads.jsonl`)

Each line in `metadata/browser_downloads.jsonl` is a JSON object with:

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

## Run summary (`run_summary.jsonl`)

Each line in `metadata/run_summary.jsonl` is a JSON object with:

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

## Entropy regions (`entropy_regions.jsonl`)

Each line in `metadata/entropy_regions.jsonl` is a JSON object with:

- `run_id`
- `global_start`
- `global_end`
- `entropy`
- `window_size`
- `tool_version`
- `config_hash`
- `evidence_path`
- `evidence_sha256`
