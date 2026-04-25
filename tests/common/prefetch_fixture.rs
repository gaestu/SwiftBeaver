/// Build a minimal SCCA Prefetch byte blob for testing.
///
/// The blob is 0x200 bytes (512 bytes), which matches the `file_size` field
/// written into the header, so the carver will accept it.
///
/// # Arguments
///
/// * `version`         – 17=XP, 23=Vista/7, 26=Win8.x, 30=Win10/11
/// * `exe_name`        – executable name (max 29 chars; stored as UTF-16LE)
/// * `hash`            – prefetch hash stored in the file header
/// * `run_count`       – run count value
/// * `last_run_filetime` – Windows FILETIME (100 ns intervals since 1601-01-01)
pub fn build_scca_prefetch(
    version: u32,
    exe_name: &str,
    hash: u32,
    run_count: u32,
    last_run_filetime: u64,
) -> Vec<u8> {
    const SCCA_EXE_NAME_OFFSET: usize = 16;
    const SCCA_HASH_OFFSET: usize = 76;

    let mut buf = vec![0u8; 0x200];

    // version (u32 LE)
    buf[0..4].copy_from_slice(&version.to_le_bytes());
    // "SCCA" signature
    buf[4..8].copy_from_slice(b"SCCA");
    // file_size (u32 LE) — must match the actual buf length so the carver keeps it
    let file_size: u32 = buf.len() as u32;
    buf[12..16].copy_from_slice(&file_size.to_le_bytes());

    // executable name: up to 29 UTF-16LE code units + null terminator in 60 bytes
    let mut name_pos = SCCA_EXE_NAME_OFFSET;
    for unit in exe_name.encode_utf16().take(29) {
        buf[name_pos] = unit as u8;
        buf[name_pos + 1] = (unit >> 8) as u8;
        name_pos += 2;
    }

    // prefetch hash
    buf[SCCA_HASH_OFFSET..SCCA_HASH_OFFSET + 4].copy_from_slice(&hash.to_le_bytes());

    // Version-specific offsets (mirrors version_offsets() in the carver)
    let (run_count_offset, last_run_offset) = match version {
        17 => (0x90usize, 0x78usize),
        23 => (0x98usize, 0x80usize),
        26 => (0xD0usize, 0x80usize),
        _ => (0xD0usize, 0x80usize),
    };

    if run_count_offset + 4 <= buf.len() {
        buf[run_count_offset..run_count_offset + 4].copy_from_slice(&run_count.to_le_bytes());
    }
    if last_run_offset + 8 <= buf.len() {
        buf[last_run_offset..last_run_offset + 8].copy_from_slice(&last_run_filetime.to_le_bytes());
    }

    buf
}

/// Wrap a SCCA blob in a MAM (Win10+ LZXPRESS Huffman) header.
///
/// The compressed payload uses a literal-only LZXPRESS Huffman encoding:
/// all 256 literal symbols get length 8, back-reference symbols are unused.
/// This produces a valid stream that the decompressor accepts while keeping
/// the fixture code self-contained and dependency-free.
///
/// The returned bytes are: [MAM_MAGIC(4)][uncompressed_size u32 LE][compressed_payload]
///
/// Appending arbitrary sentinel bytes after the returned slice allows callers
/// to verify the carver stops at the exact MAM record boundary.
pub fn build_mam_prefetch(scca_blob: &[u8]) -> Vec<u8> {
    let uncompressed_size = scca_blob.len();

    // -------------------------------------------------------------------
    // Literal-only LZXPRESS Huffman encoding
    // -------------------------------------------------------------------
    // Symbol table: 512 symbols, 4-bit lengths packed into 256 bytes.
    //   Symbols 0..255 (literals)  → length 8
    //   Symbols 256..511 (back-refs) → length 0 (unused)
    // Packed: each byte holds two 4-bit lengths (lo nibble = even symbol, hi = odd).
    // Bytes 0..127: both nibbles = 8 → 0x88
    // Bytes 128..255: both nibbles = 0 → 0x00
    let mut table = [0u8; 256];
    for b in table[..128].iter_mut() {
        *b = 0x88;
    }

    // Canonical Huffman codes for 256 equal-length-8 symbols:
    //   code[i] = i
    // The decoder reads bits MSB-first from 16-bit little-endian words, so we
    // pack each symbol's canonical 8-bit code in that order.
    let mut bit_buf: u16 = 0;
    let mut bits_in_buf: usize = 0;
    let mut payload: Vec<u8> = Vec::new();

    for &byte in scca_blob {
        bit_buf |= (byte as u16) << (8 - bits_in_buf);
        bits_in_buf += 8;
        if bits_in_buf == 16 {
            payload.extend_from_slice(&bit_buf.to_le_bytes());
            bit_buf = 0;
            bits_in_buf = 0;
        }
    }

    // Flush remaining bits (pad with zeros)
    if bits_in_buf > 0 {
        payload.extend_from_slice(&bit_buf.to_le_bytes());
    }

    // -------------------------------------------------------------------
    // Assemble MAM blob: magic + uncompressed_size + table + payload
    // -------------------------------------------------------------------
    let mut mam = Vec::new();
    mam.extend_from_slice(&[0x4D, 0x41, 0x4D, 0x04]); // MAM magic
    mam.extend_from_slice(&(uncompressed_size as u32).to_le_bytes());
    mam.extend_from_slice(&table);
    mam.extend_from_slice(&payload);
    mam
}
