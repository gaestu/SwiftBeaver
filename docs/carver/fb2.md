# FB2 Carver

## Overview

The FB2 carver extracts raw FictionBook 2.0 ebook files stored as standalone XML documents. FB2 is an XML-based ebook format used primarily in Russian-language ebook collections and conversion pipelines.

Because the on-disk header is only the generic XML declaration, the carver applies additional FictionBook-specific checks to avoid carving unrelated XML files.

## Signature Detection

**Header Pattern**: `3C 3F 78 6D 6C` (ASCII: `<?xml`)

The scanner triggers on the XML declaration at offset 0. A hit is only accepted if the first 4 KiB also contains a FictionBook-specific marker string:

- `<FictionBook` marker text, matched case-insensitively
- `fictionbook` marker text, matched case-insensitively

The carver then scans forward until it finds the closing `</FictionBook>` tag, also matched case-insensitively.

## Validation

Validation is heuristic and is designed to reject generic XML while keeping valid FB2 files that may be partially damaged.

The handler performs these checks:

1. The file must begin with the literal `<?xml` declaration.
2. The first 4 KiB must contain either a `<fictionbook` marker or a `fictionbook` marker substring.
3. The carve is marked `validated = true` only when a closing `</FictionBook>` tag is found before EOF or `max_size`.

This is not a full XML parser. The carver does not build a DOM, validate against the FB2 schema, or confirm that nested elements are well-formed beyond locating the expected marker strings and end tag.

## Size Constraints

- **Default min_size**: 64 bytes
- **Default max_size**: 100 MiB
- Configurable via `config/default.yml` under the `fb2` file type entry

If EOF is reached before the closing tag, or if `max_size` is reached first, the file is still emitted when it meets `min_size`, but it is marked truncated and an error is recorded.

## Hash Computation

- **MD5**: Computed incrementally during extraction when enabled
- **SHA-256**: Computed incrementally during extraction when enabled

Hashes cover the exact bytes written to the carved output, including truncated files.

## Testing

Current coverage includes:

- Unit tests in `src/carve/fb2.rs` for successful carving, rejection of generic XML, and acceptance of FictionBook namespace usage
- Golden-image coverage through `tests/golden_image_test.rs`, which verifies carved outputs against `tests/golden_image/manifest.json`
- A representative corpus sample at `tests/golden_image/samples/other/sample1.fb2`

These checks keep the handler deterministic while exercising both false-positive rejection and real-world sample carving.

## Edge Cases

### `.fb2.zip` archived form

Many FictionBook distributions are packaged as `.fb2.zip`. Those are ZIP archives, not raw FB2 XML files, and are handled by the ZIP carver rather than the FB2 carver. The FB2 handler only targets standalone XML-based `.fb2` content.

### Embedded base64 binary blocks

FB2 files may contain embedded binary payloads such as cover images encoded in base64 inside `<binary>` elements. The carver treats these as normal XML content and copies them verbatim; it does not decode, validate, or carve the embedded payloads separately.

### Generic XML false positives

The shared `<?xml` signature is common across many XML formats. The FictionBook marker check in the first 4 KiB is the primary false-positive control.

### Truncated ebooks

If the closing tag is missing but the carved content still meets `min_size`, the file is retained as truncated evidence instead of being discarded.

## Performance

- **Read pattern**: Sequential reads in 64 KiB blocks
- **Header check**: One small prefix read of up to 4 KiB before output creation
- **Memory use**: Low; only the current block, a small carry buffer, and optional hash state are retained
- **Complexity**: Linear in carved size

The implementation lowercases the search window while scanning so end-tag detection remains case-insensitive.

## Forensic Considerations

FB2 files often include descriptive metadata in the `<description>` section, including:

- Author names
- Book title
- Language (`<lang>`)
- Publisher and document metadata
- Conversion tool identifiers

These fields can be evidentially useful for attribution, language triage, and timeline/context reconstruction. SwiftBeaver carves the raw XML without modifying metadata and records the normal provenance fields for the carved artifact.

## Structure Examples

Typical FB2 structure:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0"
              xmlns:xlink="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <author>
        <first-name>Eric</first-name>
        <last-name>Weiner</last-name>
      </author>
      <book-title>The Geography of Bliss</book-title>
      <lang>en</lang>
    </title-info>
  </description>
  <body>
    <section>
      <p>Chapter text...</p>
    </section>
  </body>
</FictionBook>
```

At a high level, the carver depends on this pattern:

```text
<?xml ... ?>
<FictionBook ...>
  ... document content ...
</FictionBook>
```

## Known Limitations

- No XML schema validation or full well-formedness parsing
- No support for `.fb2.zip` as an FB2-specific container; archived forms are left to ZIP handling
- No separate extraction of embedded base64 binaries such as cover images
- Validation depends on the presence of a closing `</FictionBook>` tag; malformed files without that tag are only retained as truncated output
- Namespace detection is string-based, not semantic, so unusual XML layouts may evade or confuse the heuristic checks

## Related Carvers

- **MOBI**: Palm Database-based ebook format with binary structural validation
- **LRF**: Sony BBeB ebook format with declared-size header parsing
