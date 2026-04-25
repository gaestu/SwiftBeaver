# RTF Carver

## Overview

The RTF carver extracts Rich Text Format documents from raw forensic evidence.
RTF is a brace-delimited, ASCII-based document format with **no formal
end-of-file marker**: the document ends when the outermost `{` group is
closed. The carver therefore tracks brace depth byte-by-byte, honouring RTF
escape rules and the binary `\binN` opcode so embedded `}` bytes inside binary
payloads cannot prematurely terminate the carve.

The implementation lives in [src/carve/rtf.rs](../../src/carve/rtf.rs).

## Signature Detection

| Pattern ID   | Hex Signature       | ASCII     |
|--------------|---------------------|-----------|
| `rtf_header` | `7B 5C 72 74 66`    | `{\rtf`   |

The five-byte sequence `{\rtf` is the canonical RTF prefix as defined by the
RTF specification. Every conformant RTF document begins with it (typically
followed by `1` for `{\rtf1`). A single header pattern is sufficient because
the prefix is sufficiently distinctive that false positives are rare.

## Carving Algorithm

RTF is a **structure-driven** carver:

1. **Pre-validate** — read 5 bytes at the candidate offset and require an
   exact `{\rtf` match. Anything else is rejected before any output is
   created.
2. **Open stream** — start a `CarveStream` at the hit offset, bounded by
   `max_size`.
3. **Re-read header** — consume the leading `{\rtf` through the stream and
   initialise brace depth to `1`.
4. **Walk the body byte-by-byte**, applying RTF parsing rules (see below)
   until depth reaches `0` (validated end of document) or the stream hits
   `max_size` / EOF (truncated).
5. **Finalise** — compute MD5 / SHA-256, enforce `min_size`, and emit the
   metadata row.

### State Machine

```
  candidate hit ({\rtf)
          │
          ▼
  pre-validate 5-byte header ── fail ──► reject
          │ ok
          ▼
  consume "{\rtf", depth = 1
          │
          ▼
  read next byte ◄────────────────────────────┐
          │                                    │
          │                                    │
  ┌───────┴───────┐                            │
  │ bin_len > 0?  │ yes ──► skip 1 raw byte ──┘
  └───────┬───────┘
          │ no
          ▼
  ┌───────────────┐
  │ in control    │ yes ──► parse control word / digits ──┐
  │ word?         │                                       │
  └───────┬───────┘                                       │
          │ no                                            │
          ▼                                               │
  byte == '\\' ──► enter control mode ──────────────────┐ │
  byte == '{'  ──► depth += 1                           │ │
  byte == '}'  ──► depth -= 1; if depth <= 0: VALIDATED │ │
  other        ──► literal text                         │ │
          │                                             │ │
          └─────────────────────────────────────────────┴─┘
                              │
                              ▼
                   max_size / EOF reached ──► TRUNCATED
                              │
                              ▼
                        write & hash
```

## Validation

### Pre-Carve Validation

The first 5 bytes at the candidate offset must equal `{\rtf` exactly. The
check is performed in `pre_validate` before any stream is opened, so rejected
candidates create no temporary files.

### Brace-Balanced Termination

RTF documents are wrapped in a single outer `{ ... }` group. The carver
maintains a signed `depth` counter:

- `{` increments `depth`.
- `}` decrements `depth`. When `depth <= 0`, the closing brace is consumed,
  the carve ends, and `validated` is set to `true`.

### Escape & Control-Word Handling

Unescaped scanning of `{` and `}` would be unsafe, because RTF allows literal
braces inside the document. The parser implements three RTF-specific rules:

1. **Backslash escapes** — when `\` is followed by `{`, `}`, or `\`, the next
   byte is a literal character and is **not** treated as a brace or as the
   start of a control word.
2. **Control words** — `\` followed by an ASCII letter starts a control word
   (e.g. `\par`, `\b`, `\bin`). Letters are accumulated into `control_buf`
   until the first non-letter byte, which terminates the control word and is
   re-fed into the main loop via the `pending` slot so it is parsed
   normally (it may itself be `{`, `}`, or another `\`).
3. **`\binN` binary payload** — when the control word is exactly `bin`, any
   subsequent ASCII digits are accumulated into `bin_len`. While `bin_len > 0`,
   each following byte is treated as opaque binary and consumed without any
   structural interpretation. This prevents `}` bytes inside an embedded
   binary blob (images, OLE objects, etc.) from collapsing the brace counter.

`bin_len` accumulation uses `saturating_mul` / `saturating_add`, so a
deliberately oversized `\bin` length cannot wrap or panic; it will simply
cause the carve to stream binary bytes until `max_size` or EOF is reached
and the result will be marked `truncated`.

### Post-Carve Validation

- `validated = true` only when the outer brace closed before `max_size` /
  EOF.
- If `max_size` is hit, `truncated = true` and the error list contains
  `"max_size reached"`.
- If EOF is hit, `truncated = true` and the underlying I/O error string is
  recorded in the error list.
- If `pre_validate` somehow accepted but the stream's first 5 bytes do not
  re-match `{\rtf`, the candidate is silently discarded (no file, no
  metadata row).
- Files smaller than `min_size` after finalisation are discarded.

## Size Constraints

| Parameter  | Default   | Notes                                                |
|------------|-----------|------------------------------------------------------|
| `min_size` | 7 bytes   | Smaller carves are discarded after writing           |
| `max_size` | 100 MiB   | Streaming stops at this offset; result is truncated  |

Defaults come from [config/default.yml](../../config/default.yml). Both can be
overridden per run via configuration.

## Hash Computation

- **MD5** and **SHA-256** are computed incrementally during streaming over
  the bytes that are actually written.
- Hashes cover only the carved range (start of `{\rtf` through the matching
  closing brace, or `max_size` / EOF for truncated outputs).
- Hash computation honours the global `hash_config` (either or both can be
  disabled). See [src/hash.rs](../../src/hash.rs).

## Testing

RTF coverage is provided by the **golden image framework**:

- Golden image: `tests/golden_image/golden.bin`
- Manifest entry (see `tests/golden_image/manifest.json`):
  - `documents/rtf/file-sample_100kB.rtf` — full RTF document with font
    table, colour table, stylesheet, and body content; 100 605 bytes.

The file is validated end-to-end through
[tests/golden_image_test.rs](../../tests/golden_image_test.rs), which asserts
exact size and SHA-256 match against the manifest.

In addition, [src/carve/rtf.rs](../../src/carve/rtf.rs) contains an in-module
unit test `carves_minimal_rtf` that exercises a synthetic `{\rtf1 Hello}`
document via a `SliceEvidence` source and asserts the carve is `validated`
and exactly the input length.

## Edge Cases

| Case                                         | Behaviour                                                                 |
|----------------------------------------------|---------------------------------------------------------------------------|
| Escaped brace `\{` or `\}` in body           | Skipped via backslash-escape rule; does not affect depth                  |
| Escaped backslash `\\`                       | Consumed as literal; next byte parsed normally                            |
| Control word followed by `{` / `}` / `\`     | Control word terminates; following structural byte is processed via `pending` |
| Embedded binary via `\binN ...`              | `N` raw bytes consumed verbatim; no brace tracking inside the payload     |
| Oversized `\bin` length                      | `saturating_mul` / `saturating_add` prevent overflow; carve runs to `max_size` and is marked `truncated` |
| Nested groups (font tables, stylesheets…)    | Tracked correctly via depth counter; carve ends only when outer `}` closes |
| Header mismatch at hit offset                | Rejected in `pre_validate`; no output created                             |
| Truncated header (< 5 bytes available)       | Rejected in `pre_validate`                                                |
| Document reaches `max_size` mid-group        | Output kept and marked `truncated = true`, `validated = false`            |
| EOF mid-group                                | Output kept and marked `truncated = true`, `validated = false`            |
| Resulting carve smaller than `min_size`      | Discarded after finalisation                                              |

## Performance

- **Memory**: constant — a small `control_buf` for the active control word
  plus the shared carve stream buffer. No look-ahead beyond a single
  `pending` byte.
- **I/O pattern**: sequential `read_at` from the evidence source via
  `CarveStream`.
- **Scanning cost**: one-byte-at-a-time state machine. No regex, no
  allocation per byte (`control_buf` is reused).
- **Hashing**: single-pass, computed during streaming; cost is linear in
  carved bytes.

## Forensic Considerations

- **Evidence integrity**: the source is opened read-only via
  `EvidenceSource::read_at`; no write-back path exists.
- **Reproducibility**: the brace-tracking state machine is fully
  deterministic and configuration-driven, so identical input + configuration
  produce identical output.
- **Provenance**: every emitted row includes `run_id`, `tool_version`,
  `config_hash`, `evidence_path`, plus `pattern_id` (`rtf_header`),
  `global_start`, `global_end`, `validated`, and `truncated`.
- **No path traversal**: output paths are constructed via `output_path` from
  the configured output root and the byte offset, never from data inside the
  RTF document (file table entries, embedded object names, etc. are ignored
  when naming files).
- **Truncation transparency**: when `max_size` or EOF terminates a carve, the
  metadata row is marked `truncated = true` so analysts can see that the
  file is incomplete rather than silently trusting the bytes.
- **`\bin` payload safety**: oversized `\bin` lengths cannot cause integer
  overflow or panic; the worst case is a truncated carve.

## Structure Example

A minimal RTF document the carver fully validates:

```
{\rtf1 Hello}
│ │   │     │
│ │   │     └── outer '}' → depth 0 → VALIDATED
│ │   └──────── body text
│ └──────────── control word "rtf" + version digit
└────────────── opening '{' → depth 1
```

A more representative document with nested groups (font and colour tables,
stylesheet, body) is exercised by the golden image
`documents/rtf/file-sample_100kB.rtf`.

## Known Limitations

- **No semantic parsing**: the carver does not interpret control words,
  Unicode escapes (`\u`), hex byte escapes (`\'hh`), or document properties.
  It guarantees byte-accurate extraction of the document, not its meaning.
- **No nested RTF detection**: an RTF document embedded inside another (e.g.
  via `\object`) is not separately emitted; it is part of the outer carve.
- **Trailing whitespace / newlines outside the outer brace** are not
  included in the carve; the file ends precisely at the closing `}`.
- **Malformed RTF with unbalanced braces** will run to `max_size` or EOF and
  be marked `truncated`, since there is no shorter structural terminator to
  fall back on.

## Related Carvers

- **PDF**: another document container with a structural end marker
  (`%%EOF`); see [pdf.md](pdf.md).
- **OLE/CFB**: binary Office documents (DOC, XLS, PPT) — different
  structure, FAT-based.
- **EML**: text-based, marker-driven document carver; see [eml.md](eml.md).
