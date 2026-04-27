# EML Carver

## Overview

The EML carver extracts RFC 822 / RFC 5322 email messages from raw forensic
evidence. EML files are plain-text by nature and have **no formal end-of-file
marker**, so the carver combines header validation with three layered
end-of-message detection strategies (MIME boundary, mbox separator, binary
content transition) to produce tightly bounded output instead of greedily
consuming all trailing data.

The implementation lives in [src/carve/eml.rs](../../src/carve/eml.rs).

## Signature Detection

EML carving is triggered by either of two header byte patterns. Both signatures
are matched at any byte offset by the scanner.

| Pattern ID     | Hex Signature        | ASCII        |
|----------------|----------------------|--------------|
| `eml_from`     | `46 72 6F 6D 3A 20`  | `From: `     |
| `eml_received` | `52 65 63 65 69 76 65 64 3A` | `Received:` |

Two patterns are used because:

- `From:` is the canonical RFC 822 originator header but is also extremely
  common in compiled binaries and other text, so additional validation is
  required.
- `Received:` headers are added by every MTA that handles a message and provide
  a more discriminating entry point on disks that contain server-side mail.

## Carving Algorithm

EML is a **marker-based** carver with strict pre-validation:

1. **Pre-validate header window** — read 2 KiB at the candidate offset and run
   `validate_email_prefix` (header count, template rejection, `@` indicator,
   line-ending check). Reject the candidate if validation fails.
2. **Extract MIME boundary** — scan the header window for a
   `Content-Type: multipart/...; boundary=...` parameter and build the final
   boundary marker `--<boundary>--`.
3. **Stream forward** in 64 KiB chunks, applying three end-of-message
   strategies in priority order. A small carry buffer (sized to the longer of
   the mbox and MIME boundary lengths minus 1 byte) is preserved between
   chunks so a marker that straddles a chunk boundary is still detected.
4. **Post-carve validation** — if no structural boundary was matched, verify
   the overall binary-indicator ratio of the carved bytes is below the
   threshold; otherwise discard the candidate.
5. **Persist** the byte range to the output directory, computing MD5 / SHA-256
   incrementally as configured.

### State Machine

```
  candidate hit (From:/Received:)
          │
          ▼
  read 2 KiB header window
          │
          ▼
  validate_email_prefix ── fail ──► reject
          │ ok
          ▼
  extract MIME boundary (optional)
          │
          ▼
  stream 64 KiB chunks ◄────────────┐
          │                         │
          ├─ MIME final boundary ──►│ end = boundary + len  (validated)
          │                         │
          ├─ mbox "\nFrom " ───────►│ end = boundary offset (validated)
          │                         │
          ├─ binary transition ────►│ end = transition pos  (validated)
          │   (after first 512 B)   │
          │                         │
          ├─ max_size reached ─────►│ end = max_end         (truncated)
          │                         │
          └─ EOF reached ──────────►│ end = EOF             (truncated)
                                    │
                                    ▼
                       post-carve binary-ratio check
                       (only when no boundary found)
                                    │
                                    ▼
                              write & hash
```

## Validation

### Pre-Carve Header Validation

`validate_email_prefix` (see [src/carve/eml.rs](../../src/carve/eml.rs)) requires
**all** of the following on the first 2 KiB:

- At least **3 distinct** RFC 822 headers from this set:

  | Header         | Bytes                                  |
  |----------------|----------------------------------------|
  | `From:`        | `46 72 6F 6D 3A`                       |
  | `To:`          | `54 6F 3A`                             |
  | `Subject:`     | `53 75 62 6A 65 63 74 3A`              |
  | `Date:`        | `44 61 74 65 3A`                       |
  | `Message-ID:`  | `4D 65 73 73 61 67 65 2D 49 44 3A`     |
  | `MIME-Version:`| `4D 49 4D 45 2D 56 65 72 73 69 6F 6E 3A` |
  | `Received:`    | `52 65 63 65 69 76 65 64 3A`           |

- **Template rejection** — the header area (everything before the first blank
  line) must not contain `%s`, `%d`, `{}`, `<%s>`, or `${`. These tokens are
  hallmarks of compiled-in format strings and template literals harvested by
  the scanner from binaries.
- **Email pattern** — at least one `@` byte must appear in the header window.
- **Line endings** — the header window must contain `\r\n` or `\n`.

### End-of-Message Detection

Strategies are applied in priority order on each streamed chunk. The first to
match wins.

#### 1. MIME Final Boundary (highest confidence)

For multipart messages, the `boundary=` parameter is parsed from the
`Content-Type` header, supporting both quoted (`boundary="abc"`) and
unquoted (`boundary=abc`) forms. The boundary value is bounded to ≤ 200 bytes
to reject obviously corrupt headers. The carver searches for the closing
sentinel `--<boundary>--` and ends the carve **immediately after** it, so the
written file includes the terminating marker.

#### 2. Mbox Boundary

A `\nFrom ` (line-feed + `From ` + space) sequence terminates the current
message. This handles mbox-style mailbox files where multiple emails are
concatenated without MIME structure. The carve ends **at** the boundary
offset, so the next message's `From ` line is not included in the previous
file.

#### 3. Binary Content Transition

A 512-byte sliding window (50% step) inspects each streamed chunk. If more
than 30 % of the bytes in any window match `is_binary_indicator`
(bytes `0x00`–`0x08`, `0x0E`–`0x1F`, `0x7F`), the carve ends at that window
position. This prevents the carver from running into binary filesystem data
that follows the email on disk.

Binary-transition detection only activates after the first 512 bytes of the
carve have been scanned, so it cannot fire inside the header area.

### Post-Carve Validation

If none of the structural strategies matched (the carve ran to `max_size` or
EOF without finding a boundary), the cumulative binary-indicator ratio over
the entire carved range is checked. If it exceeds 30 %, the candidate is
discarded entirely (no file is written, no metadata row is emitted).

The `validated` flag in metadata is set to `true` whenever the file is not
EOF-truncated by `write_range`, regardless of which detection strategy
terminated it.

## Size Constraints

| Parameter  | Default  | Notes                                                |
|------------|----------|------------------------------------------------------|
| `min_size` | 32 bytes | Smaller carves are discarded after writing           |
| `max_size` | 10 MiB   | Streaming stops at this offset; result is truncated  |

Defaults come from [config/default.yml](../../config/default.yml). Both can be
overridden per run via configuration.

## Hash Computation

- **MD5** and **SHA-256** are computed incrementally during `write_range` over
  the bytes that are actually written.
- Hashes cover only the carved range (start of headers through whichever
  end-of-message strategy fired, or `max_size` / EOF for truncated outputs).
- Hash computation honours the global `hash_config` (either or both can be
  disabled). See [src/hash.rs](../../src/hash.rs).

## Testing

EML coverage is provided by the **golden image framework**:

- Golden image: `tests/golden_image/golden.bin`
- Manifest entries (see `tests/golden_image/manifest.json`):
  - `email/test_simple.eml` — minimal headers + body, 468 bytes
  - `email/test_with_attachment.eml` — multipart MIME with attachment, 810 bytes

Both files are validated end-to-end through
[tests/golden_image_test.rs](../../tests/golden_image_test.rs), which asserts
exact size and SHA-256 match against the manifest.

In addition, [src/carve/eml.rs](../../src/carve/eml.rs) contains in-module
unit tests covering header validation, MIME boundary extraction, mbox
termination, template rejection, and binary-transition detection using a
synthetic `SliceEvidence` source.

## Edge Cases

| Case                                       | Behaviour                                                                 |
|--------------------------------------------|---------------------------------------------------------------------------|
| Mbox `From ` line as separator             | Detected via `\nFrom ` and used to terminate the prior message            |
| Multipart MIME with quoted boundary        | `boundary="..."` parsed, final `--boundary--` used to end carve           |
| Multipart MIME with unquoted boundary      | Parsed up to whitespace, `;`, or end of header                            |
| Nested MIME parts                          | Only the outermost boundary is honoured (see Limitations)                 |
| Compiled-binary `From:` strings            | Rejected by template heuristic and the 3-header minimum                   |
| Email immediately followed by binary data  | Binary-transition heuristic terminates the carve before binary section    |
| Header window without `@`                  | Rejected by `contains_email_pattern`                                      |
| Malformed boundary (>200 bytes / empty)    | Boundary is ignored; mbox + binary-transition still apply                 |
| Marker straddling 64 KiB chunk boundary    | Caught via per-chunk carry buffer sized to the longer marker − 1 byte     |
| File reaches `max_size` mid-message        | Output is kept and marked `truncated = true`, `validated = false`         |
| End-of-message detection (historical)      | Tracked in [#12](https://github.com/HagenTroidl/SwiftBeaver/issues/12); resolved by the three-strategy pipeline above |

## Performance

- **Memory**: constant — a 64 KiB read buffer plus a small carry buffer
  (≤ MIME boundary length, capped at ~200 bytes by the parser).
- **I/O pattern**: sequential `read_at` from the evidence source.
- **Scanning cost**: dominated by per-chunk substring searches for the MIME
  and mbox markers and the 512-byte binary-indicator window. No regex or
  full-text parser is invoked.
- **Hashing**: single-pass, computed during `write_range`; cost is linear in
  carved bytes.

## Forensic Considerations

- **Evidence integrity**: the source is opened read-only via
  `EvidenceSource::read_at`; no write-back path exists.
- **Reproducibility**: pre-validation, boundary extraction, and termination
  thresholds are deterministic and configuration-driven, so identical input +
  configuration produce identical output.
- **Provenance**: every emitted row includes `run_id`, `tool_version`,
  `config_hash`, `evidence_path`, plus `pattern_id` (`eml_from` or
  `eml_received`), `global_start`, `global_end`, `validated`, and `truncated`.
- **No path traversal**: output paths are constructed via `output_path` from
  the configured output root and the byte offset, never from data inside the
  email (Subject, filename headers, etc. are ignored when naming files).
- **Truncation transparency**: when `max_size` or EOF terminates a carve, the
  metadata row is marked `truncated = true` so analysts can see that the
  file is incomplete rather than silently trusting the bytes.

## Structure Examples

### Simple RFC 822 message (terminated by EOF or binary transition)

```
From: alice@example.com
To: bob@example.org
Subject: Hello
Date: Mon, 01 Jan 2024 12:00:00 +0000
Message-ID: <abc@example.com>
                                  ◄── blank line: end of headers
Hello Bob,
This is the body.
```

### Multipart MIME (terminated by `--boundary--`)

```
From: alice@example.com
To: bob@example.org
Subject: With attachment
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="BOUNDARY"

--BOUNDARY
Content-Type: text/plain

Body text.
--BOUNDARY
Content-Type: application/octet-stream
Content-Transfer-Encoding: base64

QmFzZTY0IHBheWxvYWQ=
--BOUNDARY--           ◄── end of carve (inclusive)
```

### Mbox concatenation (terminated by `\nFrom `)

```
From alice@example.com Mon Jan  1 12:00:00 2024
From: alice@example.com
Subject: First
Date: ...

First body.
                                 ◄── carve ends here
From bob@example.org Mon Jan  1 12:05:00 2024   ◄── start of next mbox entry
From: bob@example.org
...
```

## Known Limitations

- **No recursive MIME parsing**: only the outermost `boundary=` parameter is
  honoured. Nested multipart parts are treated as opaque body bytes.
- **Encrypted bodies (S/MIME, PGP)**: ciphertext that is not base64-armoured
  may trigger the binary-transition heuristic and prematurely terminate the
  carve. ASCII-armoured PGP and base64-encoded S/MIME bodies are unaffected.
- **Address syntax not validated**: the carver only checks for the presence of
  an `@` byte; full RFC 5322 address parsing is out of scope.
- **`From ` quoting in mbox bodies**: a literal `\nFrom ` inside a body that
  has not been mbox-escaped (`>From `) will end the carve early. This matches
  the standard mbox interchange convention.
- **Single-strategy precedence**: when both a MIME boundary and an mbox
  separator appear in the same chunk, MIME wins regardless of which occurs
  first in the stream.
- **`Content-Type` boundary > 200 bytes**: deliberately rejected to bound
  worst-case marker search cost; affected messages fall back to mbox /
  binary-transition detection.

## Related Carvers

- **PDF** ([pdf.md](pdf.md)) — another marker-based text carver (`%PDF` →
  `%%EOF`) with comparable end-of-stream challenges.
- **RTF** — text-based document format with marker-based termination.
- **OLE** — used for legacy `.msg` Outlook messages, which are *not* handled
  by the EML carver.
- **MOBI / FB2 / LRF** — text-derived formats sharing the
  validate-then-stream pattern.
