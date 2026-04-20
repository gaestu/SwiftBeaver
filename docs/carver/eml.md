# EML Carver

Carves RFC 822 email messages (`.eml` files) from forensic disk images.

## Detection

The EML carver triggers on two header patterns:

| Pattern ID     | Hex Signature        | ASCII     |
|----------------|----------------------|-----------|
| `eml_from`     | `46726F6D3A20`       | `From: `  |
| `eml_received` | `52656365697665643A` | `Received:` |

## Header Validation

A candidate must contain at least **3 distinct** RFC 822 headers from:

- `From:`
- `To:`
- `Subject:`
- `Date:`
- `Message-ID:`
- `MIME-Version:`
- `Received:`

Additional checks reject false positives:

- **Template rejection**: Strings containing `%s`, `%d`, `{}`, `<%s>`, or `${` are discarded (common in compiled binaries).
- **Email pattern**: At least one `@` character must appear in the header area.
- **Line endings**: Proper `\r\n` or `\n` line endings must be present.

## End-of-Message Detection

Three strategies are applied in priority order during the scan loop:

### 1. MIME Final Boundary (highest confidence)

For multipart emails, the `boundary=` parameter is extracted from the `Content-Type` header. The carve ends immediately after the final boundary marker (`--<boundary>--`).

### 2. Mbox Boundary

A `\nFrom ` sequence (mbox separator) terminates the current message. This handles mbox-style mailbox files where multiple emails are concatenated.

### 3. Binary Content Transition

A sliding 512-byte window scans the data stream. If more than 30% of bytes in any window are binary indicators (bytes `0x00`–`0x08`, `0x0E`–`0x1F`, `0x7F`), the carve terminates at that point. This prevents the carver from consuming binary filesystem data that follows an email on disk.

Binary transition detection is only active after the first 512 bytes to avoid false triggers in the header area.

## Post-Carve Validation

If no structural boundary (MIME or mbox) was found, the carver checks the overall binary ratio of the scanned content. Files where more than 30% of bytes are binary indicators are rejected entirely.

## Configuration

Default settings in `config/default.yml`:

| Parameter  | Default    | Description                    |
|------------|------------|--------------------------------|
| `max_size` | 10 MiB     | Maximum carved file size       |
| `min_size` | 32 bytes   | Minimum carved file size       |

## Limitations

- Nested MIME messages are not parsed recursively; only the outermost boundary is used.
- Encrypted (S/MIME, PGP) email bodies may trigger binary transition detection if the ciphertext is not base64-encoded.
- The carver does not validate email address syntax beyond checking for `@`.
