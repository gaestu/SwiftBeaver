use crate::carve::windows::WindowsArtefactRecord;
use crate::carve::{
    CarveError, CarveHandler, DeferredWriter, ExtractionContext, PendingCarve, PostCarveMetadata,
    PreValidation, build_carved_file, create_hashers, finalize_hashers, output_path,
};
use crate::evidence::EvidenceSource;
use crate::parsers::time::windows_filetime_to_datetime;
use crate::scanner::NormalizedHit;

// ------------------------------------------------------------------
// Layout constants (all offsets into the decoded SCCA header bytes)
// ------------------------------------------------------------------

/// SCCA header is always 84 bytes minimum (version, signature, unknown, file_size,
/// exe_name[60], prefetch_hash, unknown)
const SCCA_MIN_SIZE: usize = 84;
const SCCA_VERSION_OFFSET: usize = 0;
const SCCA_SIGNATURE_OFFSET: usize = 4;
const SCCA_FILE_SIZE_OFFSET: usize = 12;
const SCCA_EXE_NAME_OFFSET: usize = 16;
const SCCA_EXE_NAME_LEN: usize = 60; // bytes (30 UTF-16LE code units)
const SCCA_HASH_OFFSET: usize = 76;

const VERSION_XP: u32 = 17;
const VERSION_VISTA_7: u32 = 23;
const VERSION_WIN8: u32 = 26;
const VERSION_WIN10: u32 = 30;
const VERSION_WIN10_1809: u32 = 31;

// Last-run-time offsets (in SCCA header bytes)
const LAST_RUN_TIME_OFFSET_V17: usize = 0x78;
const LAST_RUN_TIME_OFFSET_V23_PLUS: usize = 0x80;
const LAST_RUN_TIME_COUNT_V17: usize = 1;
const LAST_RUN_TIME_COUNT_V23: usize = 1;
const LAST_RUN_TIME_COUNT_V26_PLUS: usize = 8; // 8 × 8 bytes = 64 bytes

// Run-count offsets
const RUN_COUNT_OFFSET_V17: usize = 0x90;
const RUN_COUNT_OFFSET_V23: usize = 0x98;
const RUN_COUNT_OFFSET_V26_PLUS: usize = 0xD0;

// Volume-info offsets (u32 offset at these positions, pointing into the file)
const VOLUME_INFO_OFFSET_FIELD_V17: usize = 0x6C;
const VOLUME_INFO_OFFSET_FIELD_V23: usize = 0x74;
const VOLUME_INFO_OFFSET_FIELD_V26: usize = 0x74;
const VOLUME_INFO_OFFSET_FIELD_V30: usize = 0x78;
const VOLUME_INFO_ENTRY_SIZE_V17: usize = 40;
const VOLUME_INFO_ENTRY_SIZE_V23_V26: usize = 104;
const VOLUME_INFO_ENTRY_SIZE_V30_V31: usize = 96;

// Volume-info entry layout (common across versions). The SCCA spec stores
// volume entries as a fixed-size array; the count and total section size
// come from the SCCA header fields immediately after the volume-info offset.
const VOLUME_PATH_OFFSET_FIELD: usize = 0x00; // path string offset, relative to section start (u32)
const VOLUME_PATH_LEN_FIELD: usize = 0x04; // path length in UTF-16 code units (u32)
// Header offset deltas relative to the volume-info-offset field.
const VOLUME_COUNT_FIELD_DELTA: usize = 4; // entry count
const VOLUME_SIZE_FIELD_DELTA: usize = 8; // section size in bytes

// MAM (compressed Win10+) constants
const MAM_MAGIC: [u8; 4] = [0x4D, 0x41, 0x4D, 0x04];
const SCCA_MAGIC: [u8; 4] = [0x53, 0x43, 0x43, 0x41];
/// Bytes 4-7 of MAM header = uncompressed size (u32 LE)
const MAM_UNCOMPRESSED_SIZE_OFFSET: usize = 4;
/// Compressed data starts at offset 8 in the MAM block
const MAM_COMPRESSED_DATA_OFFSET: usize = 8;

// ------------------------------------------------------------------
// Public data type
// ------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrefetchArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    pub executable_name: String,
    pub prefetch_hash: String,
    pub run_count: u32,
    pub last_run_times: Vec<chrono::NaiveDateTime>,
    pub volume_paths: Vec<String>,
    pub volume_paths_truncated: bool,
    /// File references parsed from the SCCA filename strings section.
    ///
    /// `None` means extraction is not yet implemented for this version /
    /// record (so analysts must not interpret an empty list as "no
    /// references found"). When `Some(_)`, the value reflects the actual
    /// references decoded from the artefact.
    pub referenced_files: Option<Vec<String>>,
    pub version: u8,
}

// ------------------------------------------------------------------
// CarveHandler implementation
// ------------------------------------------------------------------

pub struct PrefetchCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl PrefetchCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for PrefetchCarveHandler {
    fn file_type(&self) -> &str {
        "prefetch"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        // Read 8 bytes to distinguish MAM vs SCCA
        let mut buf = [0u8; 8];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < 8 {
            return Ok(PreValidation::Reject("truncated prefetch header".into()));
        }
        if buf[..4] == MAM_MAGIC {
            return Ok(PreValidation::Proceed);
        }
        // Must be SCCA at bytes 4..8 and a valid version at bytes 0..4
        if buf[4..8] != SCCA_MAGIC {
            return Ok(PreValidation::Reject("not a prefetch signature".into()));
        }
        let version = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if matches!(
            version,
            VERSION_XP | VERSION_VISTA_7 | VERSION_WIN8 | VERSION_WIN10 | VERSION_WIN10_1809
        ) {
            Ok(PreValidation::Proceed)
        } else {
            Ok(PreValidation::Reject(format!(
                "unknown prefetch version {version}"
            )))
        }
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let max_len = usize::try_from(self.max_size)
            .map_err(|_| CarveError::Invalid("prefetch max_size exceeds platform limits".into()))?;

        let (buf, parsed) = match read_and_parse_prefetch(ctx, hit.global_offset, max_len) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        };

        if !passes_min_size(&parsed, self.min_size) {
            return Ok(None);
        }

        let size = usize::try_from(parsed.size)
            .map_err(|_| CarveError::Invalid("prefetch size overflows".into()))?;
        if size > buf.len() {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut writer =
            DeferredWriter::new(full_path, ctx.deferred_buffer_bytes, ctx.metadata_only);
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);
        let payload = &buf[..size];
        writer.write_all(payload)?;
        if let Some(ref mut hasher) = md5 {
            hasher.consume(payload);
        }
        if let Some(ref mut hasher) = sha256 {
            use sha2::Digest;
            hasher.update(payload);
        }
        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);

        let file = build_carved_file(
            ctx.run_id,
            self.file_type(),
            &self.extension,
            rel_path,
            hit.global_offset,
            parsed.size,
            md5_hex,
            sha256_hex,
            true,
            false,
            Vec::new(),
            &hit.pattern_id,
        );

        Ok(Some(PendingCarve::new(file, writer).with_post_metadata(
            vec![PostCarveMetadata::WindowsArtefact(
                WindowsArtefactRecord::Prefetch(parsed.artefact),
            )],
        )))
    }
}

// ------------------------------------------------------------------
// Internal parsing helpers
// ------------------------------------------------------------------

#[derive(Debug)]
struct ParsedPrefetch {
    /// On-disk size of the prefetch record (may differ from decoded size for MAM)
    size: u64,
    /// Size to compare against the configured min_size gate.
    ///
    /// For MAM, this uses the actual decoded byte count so highly compressible
    /// records are not rejected just because their on-disk span is small.
    validation_size: u64,
    artefact: PrefetchArtefact,
}

#[derive(Debug, Clone, Copy)]
enum PfError {
    Invalid,
    Truncated,
}

fn passes_min_size(parsed: &ParsedPrefetch, min_size: u64) -> bool {
    parsed.validation_size >= min_size
}

fn read_and_parse_prefetch(
    ctx: &ExtractionContext,
    offset: u64,
    max_len: usize,
) -> Result<Option<(Vec<u8>, ParsedPrefetch)>, CarveError> {
    // Read enough to determine format (8 bytes)
    let mut header8 = [0u8; 8];
    let n = ctx
        .evidence
        .read_at(offset, &mut header8)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < 8 {
        return Ok(None);
    }

    if header8[..4] == MAM_MAGIC {
        parse_mam_prefetch(ctx, offset, max_len)
    } else {
        parse_scca_prefetch(ctx, offset, max_len)
    }
}

/// Parse a MAM-compressed Win10+ Prefetch.
/// MAM layout: [MAM_MAGIC(4)] [uncompressed_size(4 LE)] [compressed_data...]
fn parse_mam_prefetch(
    ctx: &ExtractionContext,
    offset: u64,
    max_len: usize,
) -> Result<Option<(Vec<u8>, ParsedPrefetch)>, CarveError> {
    // Read the MAM block header (8 bytes)
    let mut header = [0u8; MAM_COMPRESSED_DATA_OFFSET];
    ctx.evidence
        .read_at(offset, &mut header)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;

    let uncompressed_size = u32::from_le_bytes([
        header[MAM_UNCOMPRESSED_SIZE_OFFSET],
        header[MAM_UNCOMPRESSED_SIZE_OFFSET + 1],
        header[MAM_UNCOMPRESSED_SIZE_OFFSET + 2],
        header[MAM_UNCOMPRESSED_SIZE_OFFSET + 3],
    ]) as usize;

    if !(SCCA_MIN_SIZE..=max_len).contains(&uncompressed_size) {
        return Ok(None);
    }

    if max_len <= MAM_COMPRESSED_DATA_OFFSET {
        return Ok(None);
    }

    // Read up to max_len bytes of the full MAM block (including the 8-byte header)
    let mut raw_buf = vec![0u8; max_len];
    let n = ctx
        .evidence
        .read_at(offset, &mut raw_buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    let raw_buf = &raw_buf[..n];
    if raw_buf.len() <= MAM_COMPRESSED_DATA_OFFSET {
        return Ok(None);
    }
    let compressed_data = &raw_buf[MAM_COMPRESSED_DATA_OFFSET..];

    // Decompress LZXPRESS Huffman; decompressor returns consumed compressed bytes
    let (decoded, compressed_consumed) =
        match lzxpress_huffman_decompress(compressed_data, uncompressed_size, max_len) {
            Some(pair) => pair,
            None => return Ok(None),
        };

    if decoded.len() < SCCA_MIN_SIZE {
        return Ok(None);
    }

    // Parse the decoded SCCA block
    let artefact = match parse_scca_bytes(&decoded, ctx.run_id, offset) {
        Ok(a) => a,
        Err(err) => {
            tracing::debug!(
                target: "prefetch",
                offset,
                error = ?err,
                "prefetch candidate passed signature pre-validation but failed MAM->SCCA parsing"
            );
            return Ok(None);
        }
    };

    // On-disk span = MAM header (8 bytes) + compressed bytes consumed by decompressor.
    // This is the exact evidence range belonging to this artefact.
    let on_disk_len = MAM_COMPRESSED_DATA_OFFSET + compressed_consumed;
    let on_disk_len = on_disk_len.min(raw_buf.len()); // never exceed what we read
    let on_disk_size = on_disk_len as u64;

    // Carve only the exact evidence bytes belonging to this MAM record
    let carved_bytes = raw_buf[..on_disk_len].to_vec();

    // artefact.size tracks the on-disk span for provenance; the decoded SCCA
    // file_size field may differ but is not emitted separately.
    Ok(Some((
        carved_bytes,
        ParsedPrefetch {
            size: on_disk_size,
            validation_size: decoded.len() as u64,
            artefact: PrefetchArtefact {
                size: on_disk_size,
                ..artefact
            },
        },
    )))
}

/// Parse an uncompressed SCCA prefetch at the given evidence offset.
fn parse_scca_prefetch(
    ctx: &ExtractionContext,
    offset: u64,
    max_len: usize,
) -> Result<Option<(Vec<u8>, ParsedPrefetch)>, CarveError> {
    // Read up to the configured max_size; the declared file_size is validated below.
    let mut buf = vec![0u8; max_len];
    let n = ctx
        .evidence
        .read_at(offset, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    let buf = &buf[..n];

    if buf.len() < SCCA_MIN_SIZE {
        return Ok(None);
    }

    let artefact = match parse_scca_bytes(buf, ctx.run_id, offset) {
        Ok(a) => a,
        Err(err) => {
            tracing::debug!(
                target: "prefetch",
                offset,
                error = ?err,
                "prefetch candidate passed signature pre-validation but failed SCCA parsing"
            );
            return Ok(None);
        }
    };

    let size = usize::try_from(artefact.size)
        .map_err(|_| CarveError::Invalid("prefetch file_size overflows usize".into()))?;
    // Guard against the declared file_size being larger than what we actually read
    // (e.g. record near EOF where n < size). Without this, buf[..size] would panic.
    if size < SCCA_MIN_SIZE || size > buf.len() {
        return Ok(None);
    }

    let carved = buf[..size].to_vec();

    Ok(Some((
        carved,
        ParsedPrefetch {
            size: artefact.size,
            validation_size: artefact.size,
            artefact,
        },
    )))
}

/// Parse the SCCA artefact from raw (uncompressed) bytes.
fn parse_scca_bytes(bytes: &[u8], run_id: &str, offset: u64) -> Result<PrefetchArtefact, PfError> {
    if bytes.len() < SCCA_MIN_SIZE {
        return Err(PfError::Truncated);
    }

    let version = read_u32(bytes, SCCA_VERSION_OFFSET)?;
    if bytes[SCCA_SIGNATURE_OFFSET..SCCA_SIGNATURE_OFFSET + 4] != SCCA_MAGIC {
        return Err(PfError::Invalid);
    }
    if !matches!(
        version,
        VERSION_XP | VERSION_VISTA_7 | VERSION_WIN8 | VERSION_WIN10 | VERSION_WIN10_1809
    ) {
        return Err(PfError::Invalid);
    }

    let file_size = read_u32(bytes, SCCA_FILE_SIZE_OFFSET)? as u64;
    if file_size < SCCA_MIN_SIZE as u64 {
        return Err(PfError::Invalid);
    }

    // Executable name: 60 bytes of UTF-16LE, null-terminated
    let exe_bytes = &bytes[SCCA_EXE_NAME_OFFSET..SCCA_EXE_NAME_OFFSET + SCCA_EXE_NAME_LEN];
    let executable_name = decode_utf16le_null_term(exe_bytes);

    // Prefetch hash: 4-byte LE hex string
    let hash_raw = read_u32(bytes, SCCA_HASH_OFFSET)?;
    let prefetch_hash = format!("{hash_raw:08X}");

    // Run count and last-run-times (version-specific)
    let (run_count_offset, last_run_offset, last_run_count, volume_info_field_offset) =
        version_offsets(version);

    let run_count = if run_count_offset + 4 <= bytes.len() {
        read_u32(bytes, run_count_offset)?
    } else {
        0
    };

    let last_run_times = parse_last_run_times(bytes, last_run_offset, last_run_count);

    // Volume paths: read offset/count/size from the SCCA header and walk the
    // fixed-size entry array. Bail out cleanly if any of the three header
    // fields can't be read.
    let (volume_paths, volume_paths_truncated) =
        if volume_info_field_offset + VOLUME_SIZE_FIELD_DELTA + 4 <= bytes.len() {
            let vi_offset = read_u32(bytes, volume_info_field_offset)? as usize;
            let entry_count =
                read_u32(bytes, volume_info_field_offset + VOLUME_COUNT_FIELD_DELTA)? as usize;
            let section_size =
                read_u32(bytes, volume_info_field_offset + VOLUME_SIZE_FIELD_DELTA)? as usize;
            parse_volume_paths(bytes, version, vi_offset, entry_count, section_size)
        } else {
            (Vec::new(), false)
        };

    Ok(PrefetchArtefact {
        run_id: run_id.to_string(),
        offset,
        size: file_size,
        executable_name,
        prefetch_hash,
        run_count,
        last_run_times,
        volume_paths,
        volume_paths_truncated,
        referenced_files: None,
        version: version as u8,
    })
}

/// Return (run_count_offset, last_run_offset, last_run_count, volume_info_field_offset)
fn version_offsets(version: u32) -> (usize, usize, usize, usize) {
    match version {
        VERSION_XP => (
            RUN_COUNT_OFFSET_V17,
            LAST_RUN_TIME_OFFSET_V17,
            LAST_RUN_TIME_COUNT_V17,
            VOLUME_INFO_OFFSET_FIELD_V17,
        ),
        VERSION_VISTA_7 => (
            RUN_COUNT_OFFSET_V23,
            LAST_RUN_TIME_OFFSET_V23_PLUS,
            LAST_RUN_TIME_COUNT_V23,
            VOLUME_INFO_OFFSET_FIELD_V23,
        ),
        VERSION_WIN8 => (
            RUN_COUNT_OFFSET_V26_PLUS,
            LAST_RUN_TIME_OFFSET_V23_PLUS,
            LAST_RUN_TIME_COUNT_V26_PLUS,
            VOLUME_INFO_OFFSET_FIELD_V26,
        ),
        VERSION_WIN10 | VERSION_WIN10_1809 => (
            RUN_COUNT_OFFSET_V26_PLUS,
            LAST_RUN_TIME_OFFSET_V23_PLUS,
            LAST_RUN_TIME_COUNT_V26_PLUS,
            VOLUME_INFO_OFFSET_FIELD_V30,
        ),
        _ => (
            RUN_COUNT_OFFSET_V26_PLUS,
            LAST_RUN_TIME_OFFSET_V23_PLUS,
            LAST_RUN_TIME_COUNT_V26_PLUS,
            VOLUME_INFO_OFFSET_FIELD_V30,
        ),
    }
}

fn parse_last_run_times(bytes: &[u8], offset: usize, count: usize) -> Vec<chrono::NaiveDateTime> {
    let mut times = Vec::with_capacity(count);
    for i in 0..count {
        let pos = offset + i * 8;
        if pos + 8 > bytes.len() {
            break;
        }
        #[allow(clippy::collapsible_if)]
        if let Ok(ft) = read_u64(bytes, pos) {
            if let Some(dt) = windows_filetime_to_datetime(ft) {
                times.push(dt);
            }
        }
    }
    times
}

fn parse_volume_paths(
    bytes: &[u8],
    version: u32,
    vi_offset: usize,
    entry_count: usize,
    section_size: usize,
) -> (Vec<String>, bool) {
    /// Cap on volume entries even when the header claims more, to defend
    /// against crafted records. Real Prefetch files have at most a handful.
    const MAX_VOLUME_ENTRIES: usize = 32;

    if entry_count == 0 || section_size == 0 {
        return (Vec::new(), false);
    }
    let entry_size = match volume_info_entry_size(version) {
        Some(size) => size,
        None => return (Vec::new(), false),
    };
    let entry_array_size = match entry_count.checked_mul(entry_size) {
        Some(size) => size,
        None => return (Vec::new(), false),
    };
    if entry_array_size > section_size {
        return (Vec::new(), false);
    }

    let section_end = match vi_offset.checked_add(section_size) {
        Some(end) if end <= bytes.len() => end,
        _ => return (Vec::new(), false),
    };
    if vi_offset
        .checked_add(entry_array_size)
        .is_none_or(|end| end > section_end)
    {
        return (Vec::new(), false);
    }

    let truncated = entry_count > MAX_VOLUME_ENTRIES;
    let entries_to_read = entry_count.min(MAX_VOLUME_ENTRIES);
    let mut paths = Vec::with_capacity(entries_to_read);

    for i in 0..entries_to_read {
        let entry_offset = match i
            .checked_mul(entry_size)
            .and_then(|delta| vi_offset.checked_add(delta))
        {
            Some(v) => v,
            None => break,
        };
        match entry_offset.checked_add(entry_size) {
            Some(end) if end <= section_end => {}
            _ => break,
        }

        let path_offset_rel = match read_u32(bytes, entry_offset + VOLUME_PATH_OFFSET_FIELD) {
            Ok(v) => v as usize,
            Err(_) => continue,
        };
        let path_len = match read_u32(bytes, entry_offset + VOLUME_PATH_LEN_FIELD) {
            Ok(v) => v as usize,
            Err(_) => continue,
        };
        if path_len == 0 {
            continue;
        }

        // Per SCCA spec, the path offset is relative to the start of the
        // volume-information section (not the individual entry).
        let path_byte_start = match vi_offset.checked_add(path_offset_rel) {
            Some(v) => v,
            None => continue,
        };
        let path_byte_end = match path_len
            .checked_mul(2)
            .and_then(|n| path_byte_start.checked_add(n))
        {
            Some(v) => v,
            None => continue,
        };
        if path_byte_end > section_end {
            continue;
        }
        let path = decode_utf16le_null_term(&bytes[path_byte_start..path_byte_end]);
        if !path.is_empty() {
            paths.push(path);
        }
    }

    if truncated {
        tracing::warn!(
            target: "prefetch",
            cap = MAX_VOLUME_ENTRIES,
            claimed = entry_count,
            decoded = paths.len(),
            "parse_volume_paths capped iteration; metadata row marks volume_paths as truncated"
        );
    }
    (paths, truncated)
}

fn volume_info_entry_size(version: u32) -> Option<usize> {
    match version {
        VERSION_XP => Some(VOLUME_INFO_ENTRY_SIZE_V17),
        VERSION_VISTA_7 | VERSION_WIN8 => Some(VOLUME_INFO_ENTRY_SIZE_V23_V26),
        VERSION_WIN10 | VERSION_WIN10_1809 => Some(VOLUME_INFO_ENTRY_SIZE_V30_V31),
        _ => None,
    }
}

// ------------------------------------------------------------------
// Primitive decoders
// ------------------------------------------------------------------

fn decode_utf16le_null_term(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units).to_string()
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PfError> {
    let end = offset.checked_add(4).ok_or(PfError::Truncated)?;
    let slice = bytes.get(offset..end).ok_or(PfError::Truncated)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, PfError> {
    let end = offset.checked_add(8).ok_or(PfError::Truncated)?;
    let slice = bytes.get(offset..end).ok_or(PfError::Truncated)?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

// ------------------------------------------------------------------
// LZXPRESS Huffman decompressor (MS-XCA §2.4)
//
// The Win10+ Prefetch MAM format uses LZXPRESS Huffman compression.
// This is a self-contained implementation of the decompressor per the
// MS-XCA specification. Returns None on malformed input.
// ------------------------------------------------------------------

/// Decompress LZXPRESS Huffman encoded data.
///
/// `compressed` is the raw compressed payload (after the 8-byte MAM header).
/// `uncompressed_size` is the expected output length.
/// `max_uncompressed` caps the allocation to defend against crafted inputs;
/// callers pass the configured prefetch `max_size`.
///
/// Returns `(decompressed_bytes, compressed_bytes_consumed)` on success, or `None`
/// on malformed input. The caller should use `compressed_bytes_consumed` to
/// determine the exact on-disk span of the MAM record.
fn lzxpress_huffman_decompress(
    compressed: &[u8],
    uncompressed_size: usize,
    max_uncompressed: usize,
) -> Option<(Vec<u8>, usize)> {
    if uncompressed_size > max_uncompressed {
        return None;
    }

    // The data is split into 65536-byte (or smaller) uncompressed chunks, each
    // preceded by a 256-byte Huffman table (512 nibbles / 256 bytes) giving
    // 4-bit lengths for 512 symbols.
    const HUFFMAN_TABLE_SIZE: usize = 256; // 512 symbols × 4 bits = 256 bytes
    const MAX_CHUNK_UNCOMPRESSED: usize = 65536;

    let mut out = Vec::with_capacity(uncompressed_size);
    let mut in_pos = 0usize;
    let mut decode_table = vec![DECOMP_TABLE_EMPTY; 65536].into_boxed_slice();

    while out.len() < uncompressed_size {
        // Each chunk starts with a 256-byte Huffman length table on a 16-bit boundary
        if in_pos + HUFFMAN_TABLE_SIZE > compressed.len() {
            return None;
        }
        let chunk_payload_start = in_pos + HUFFMAN_TABLE_SIZE;
        let table_bytes = &compressed[in_pos..chunk_payload_start];
        in_pos = chunk_payload_start;

        build_huffman_table(table_bytes, &mut decode_table)?;

        let chunk_out_limit = (uncompressed_size - out.len()).min(MAX_CHUNK_UNCOMPRESSED);
        let remaining_output = uncompressed_size - out.len();
        let chunk_output = decompress_lzxpress_huffman_chunk(
            compressed,
            &mut in_pos,
            &decode_table,
            &mut out,
            chunk_out_limit,
            remaining_output,
        )?;

        if chunk_output < chunk_out_limit {
            return None;
        }

        // `consumed_len()` rounds only the bit-buffer consumption up to a
        // 16-bit boundary. Raw extension bytes read via `read_byte()` are
        // tracked separately and can leave the overall chunk position odd, so
        // align the total stream position before the next Huffman table.
        if in_pos & 1 != 0 {
            if in_pos >= compressed.len() {
                return None;
            }
            in_pos += 1;
        }
    }

    if out.len() < uncompressed_size {
        return None;
    }
    out.truncate(uncompressed_size);
    // Subtract any bytes that were read into the bit buffer but not logically consumed.
    // This gives the true compressed byte span of the MAM record.
    Some((out, in_pos))
}

const DECOMP_TABLE_EMPTY: u16 = 0xFFFF;

fn decompress_lzxpress_huffman_chunk(
    compressed: &[u8],
    in_pos: &mut usize,
    decode_table: &[u16],
    out: &mut Vec<u8>,
    chunk_size: usize,
    remaining_output: usize,
) -> Option<usize> {
    let block_start = out.len();
    let mut state = LzxpressBitStream::new(compressed, *in_pos);
    state.seed()?;
    let mut decode_index = 0usize;
    let mut pending_match: Option<(usize, usize, usize)> = None;

    while out.len() - block_start < chunk_size {
        if let Some((mut distance_bits_wanted, mut distance, match_len)) = pending_match.take() {
            while distance_bits_wanted > 0 {
                let bit = state.read_bit()?;
                distance_bits_wanted -= 1;
                distance |= (bit as usize) << distance_bits_wanted;
            }

            copy_match(out, distance, match_len)?;
            if out.len() - block_start > remaining_output {
                return None;
            }
            continue;
        }

        let bit = state.read_bit()?;
        decode_index = (decode_index << 1) + bit as usize + 1;
        let symbol = *decode_table.get(decode_index)?;
        if symbol == DECOMP_TABLE_EMPTY {
            continue;
        }

        decode_index = 0;
        if symbol < 256 {
            out.push(symbol as u8);
            continue;
        }

        let distance_bits_wanted = ((symbol >> 4) & 0x0F) as usize;
        let distance = 1usize << distance_bits_wanted;
        let match_len = read_match_length(symbol, &mut state)?;

        if distance_bits_wanted == 0 {
            copy_match(out, distance, match_len)?;
        } else {
            pending_match = Some((distance_bits_wanted, distance, match_len));
        }

        if out.len() - block_start > remaining_output {
            return None;
        }
    }

    if decode_index != 0 || pending_match.is_some() {
        return None;
    }

    *in_pos = state.consumed_len()?;
    Some(out.len() - block_start)
}

fn build_huffman_table(table_bytes: &[u8], decode_table: &mut [u16]) -> Option<()> {
    decode_table.fill(DECOMP_TABLE_EMPTY);

    let mut symbols = Vec::with_capacity(512);
    for (index, &byte) in table_bytes.iter().enumerate() {
        let even = byte & 0x0F;
        let odd = (byte >> 4) & 0x0F;
        if even != 0 {
            symbols.push(((even as u16) << 9) | (index as u16 * 2));
        }
        if odd != 0 {
            symbols.push(((odd as u16) << 9) | (index as u16 * 2 + 1));
        }
    }

    if symbols.is_empty() {
        return None;
    }
    symbols.sort_unstable();

    let mut code: i32 = -1;
    let mut prev_len = 0u16;
    let mut last_len = 0u16;

    for encoded in symbols {
        let len = (encoded >> 9) & 0x0F;
        let symbol = encoded & 0x01FF;
        code += 1;
        while prev_len < len {
            code = code.checked_shl(1)?.checked_add(1)?;
            prev_len += 1;
        }
        if !(0..65535).contains(&code) {
            return None;
        }

        decode_table[code as usize] = symbol;
        let mut prefix = (code - 1) >> 1;
        while prefix > 31 {
            decode_table[prefix as usize] = DECOMP_TABLE_EMPTY;
            prefix = (prefix - 1) >> 1;
        }
        last_len = len;
    }

    let expected_final_code = (1u32 << (last_len as u32 + 1)) - 2;
    if code as u32 != expected_final_code {
        return None;
    }

    Some(())
}

fn read_match_length(symbol: u16, state: &mut LzxpressBitStream<'_>) -> Option<usize> {
    let mut length = (symbol & 0x0F) as usize;
    if length == 15 {
        let ext = state.read_byte()? as usize;
        length += ext;
        if length == 270 {
            length = state.read_u16()? as usize;
            if length == 0 {
                length = usize::try_from(state.read_u32()?).ok()?;
            }
        }
    }
    length.checked_add(3)
}

fn copy_match(out: &mut Vec<u8>, distance: usize, length: usize) -> Option<()> {
    let copy_start = out.len().checked_sub(distance)?;
    // Bounds check: ensure the resulting length will not overflow `usize`.
    out.len().checked_add(length)?;

    for index in 0..length {
        let copy_index = copy_start.checked_add(index)?;
        let byte = *out.get(copy_index)?;
        out.push(byte);
    }

    Some(())
}

struct LzxpressBitStream<'a> {
    bytes: &'a [u8],
    start_pos: usize,
    byte_pos: usize,
    bits: u32,
    remaining_bits: u32,
    bit_bytes_fetched: usize,
    raw_bytes_read: usize,
}

impl<'a> LzxpressBitStream<'a> {
    fn new(bytes: &'a [u8], byte_pos: usize) -> Self {
        Self {
            bytes,
            start_pos: byte_pos,
            byte_pos,
            bits: 0,
            remaining_bits: 0,
            bit_bytes_fetched: 0,
            raw_bytes_read: 0,
        }
    }

    fn seed(&mut self) -> Option<()> {
        let low = self.read_bit_u16()? as u32;
        let high = self.read_bit_u16()? as u32;
        self.bits = (low << 16) | high;
        self.remaining_bits = 32;
        Some(())
    }

    fn refill(&mut self) -> Option<()> {
        if self.byte_pos + 1 < self.bytes.len() {
            let word = self.read_bit_u16()? as u32;
            self.bits = (self.bits << 16) | word;
            self.remaining_bits += 16;
            Some(())
        } else if self.byte_pos < self.bytes.len() {
            let byte = self.read_bit_byte()? as u32;
            self.bits = (self.bits << 8) | byte;
            self.remaining_bits += 8;
            Some(())
        } else {
            None
        }
    }

    fn consumed_len(&self) -> Option<usize> {
        let bits_consumed = self
            .bit_bytes_fetched
            .saturating_mul(8)
            .saturating_sub(self.remaining_bits as usize);
        let bit_units = bits_consumed.checked_add(15)? / 16;
        let bit_bytes_consumed = bit_units.checked_mul(2)?;
        self.start_pos
            .checked_add(self.raw_bytes_read)?
            .checked_add(bit_bytes_consumed)
    }

    fn read_bit_byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.byte_pos)?;
        self.byte_pos += 1;
        self.bit_bytes_fetched += 1;
        Some(byte)
    }

    fn read_bit_u16(&mut self) -> Option<u16> {
        let lo = self.read_bit_byte()?;
        let hi = self.read_bit_byte()?;
        Some(u16::from_le_bytes([lo, hi]))
    }

    fn read_bit(&mut self) -> Option<u8> {
        if self.remaining_bits == 16 {
            self.refill()?;
        }
        if self.remaining_bits == 0 {
            return None;
        }
        self.remaining_bits -= 1;
        Some(((self.bits >> self.remaining_bits) & 1) as u8)
    }

    fn read_byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.byte_pos)?;
        self.byte_pos += 1;
        self.raw_bytes_read += 1;
        Some(byte)
    }

    fn read_u16(&mut self) -> Option<u16> {
        let lo = self.read_byte()?;
        let hi = self.read_byte()?;
        Some(u16::from_le_bytes([lo, hi]))
    }

    fn read_u32(&mut self) -> Option<u32> {
        let b0 = self.read_byte()?;
        let b1 = self.read_byte()?;
        let b2 = self.read_byte()?;
        let b3 = self.read_byte()?;
        Some(u32::from_le_bytes([b0, b1, b2, b3]))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DECOMP_TABLE_EMPTY, ParsedPrefetch, PrefetchArtefact, VERSION_WIN10,
        lzxpress_huffman_decompress, parse_volume_paths, passes_min_size,
    };

    fn set_symbol_len(table: &mut [u8; 256], symbol: usize, len: u8) {
        let byte = &mut table[symbol / 2];
        if symbol & 1 == 0 {
            *byte = (*byte & 0xF0) | (len & 0x0F);
        } else {
            *byte = (*byte & 0x0F) | ((len & 0x0F) << 4);
        }
    }

    fn pack_bits(bits: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut index = 0usize;
        while index < bits.len() {
            let mut word = 0u16;
            for bit_offset in 0..16 {
                let bit = bits.get(index + bit_offset).copied().unwrap_or(0);
                word |= (bit as u16) << (15 - bit_offset);
            }
            out.extend_from_slice(&word.to_le_bytes());
            index += 16;
        }
        out
    }

    #[test]
    fn test_lzxpress_huffman_backref_distance_one() {
        let mut table = [0u8; 256];
        set_symbol_len(&mut table, 65, 1);
        set_symbol_len(&mut table, 257, 1);

        let mut compressed = table.to_vec();
        compressed.extend_from_slice(&pack_bits(&[0, 1]));
        compressed.extend_from_slice(&[0, 0]);

        let (decoded, consumed) =
            lzxpress_huffman_decompress(&compressed, 5, 1024).expect("decode");
        assert_eq!(decoded, b"AAAAA");
        assert_eq!(consumed, 258);
    }

    #[test]
    fn test_lzxpress_huffman_extended_length_uses_byte_stream() {
        let mut table = [0u8; 256];
        set_symbol_len(&mut table, 0, 1);
        set_symbol_len(&mut table, 271, 1);

        let mut compressed = table.to_vec();
        compressed.extend_from_slice(&pack_bits(&[0, 1]));
        compressed.extend_from_slice(&[0, 0]);
        compressed.push(1);

        let (decoded, _consumed) =
            lzxpress_huffman_decompress(&compressed, 20, 1024).expect("decode");
        assert_eq!(decoded, vec![0u8; 20]);
    }

    #[test]
    fn test_prefetch_min_size_uses_decoded_size_for_mam() {
        let parsed = ParsedPrefetch {
            size: 48,
            validation_size: 512,
            artefact: PrefetchArtefact {
                run_id: "run".to_string(),
                offset: 0,
                size: 48,
                executable_name: "CMD.EXE".to_string(),
                prefetch_hash: "00112233".to_string(),
                run_count: 1,
                last_run_times: Vec::new(),
                volume_paths: Vec::new(),
                volume_paths_truncated: false,
                referenced_files: None,
                version: 30,
            },
        };

        assert!(parsed.size < 84);
        assert!(passes_min_size(&parsed, 84));
    }

    #[test]
    fn test_parse_volume_paths_uses_fixed_entry_stride() {
        let vi_offset = 0x20usize;
        let entry_size = 96usize;
        let entry_count = 2usize;
        let first_path = utf16le(r"\Device\HarddiskVolume1");
        let second_path = utf16le(r"\Device\HarddiskVolume2");
        let first_path_offset = entry_size * entry_count;
        let second_path_offset = first_path_offset + first_path.len();
        let section_size = second_path_offset + second_path.len();
        let mut bytes = vec![0u8; vi_offset + section_size];

        bytes[vi_offset..vi_offset + 4].copy_from_slice(&(first_path_offset as u32).to_le_bytes());
        bytes[vi_offset + 4..vi_offset + 8]
            .copy_from_slice(&((first_path.len() / 2) as u32).to_le_bytes());

        let second_entry = vi_offset + entry_size;
        bytes[second_entry..second_entry + 4]
            .copy_from_slice(&(second_path_offset as u32).to_le_bytes());
        bytes[second_entry + 4..second_entry + 8]
            .copy_from_slice(&((second_path.len() / 2) as u32).to_le_bytes());

        let first_path_start = vi_offset + first_path_offset;
        bytes[first_path_start..first_path_start + first_path.len()].copy_from_slice(&first_path);

        let second_path_start = vi_offset + second_path_offset;
        bytes[second_path_start..second_path_start + second_path.len()]
            .copy_from_slice(&second_path);

        let (paths, truncated) =
            parse_volume_paths(&bytes, VERSION_WIN10, vi_offset, entry_count, section_size);

        assert_eq!(
            paths,
            vec![
                r"\Device\HarddiskVolume1".to_string(),
                r"\Device\HarddiskVolume2".to_string()
            ]
        );
        assert!(!truncated);
    }

    #[test]
    fn test_parse_volume_paths_reports_truncation_when_claim_exceeds_cap() {
        let vi_offset = 0x20usize;
        let entry_size = 96usize;
        let entry_count = 33usize;
        let mut encoded_paths = Vec::with_capacity(entry_count);
        let mut path_data = Vec::new();
        let mut next_path_offset = entry_size * entry_count;

        for idx in 0..entry_count {
            let path = utf16le(&format!(r"\Device\HarddiskVolume{idx:02}"));
            encoded_paths.push((next_path_offset, path.len() / 2, path.clone()));
            next_path_offset += path.len();
            path_data.extend_from_slice(&path);
        }

        let section_size = entry_size * entry_count + path_data.len();
        let mut bytes = vec![0u8; vi_offset + section_size];

        for (idx, (path_offset, path_len, _)) in encoded_paths.iter().enumerate() {
            let entry_offset = vi_offset + idx * entry_size;
            bytes[entry_offset..entry_offset + 4]
                .copy_from_slice(&(*path_offset as u32).to_le_bytes());
            bytes[entry_offset + 4..entry_offset + 8]
                .copy_from_slice(&(*path_len as u32).to_le_bytes());
        }

        let path_data_start = vi_offset + entry_size * entry_count;
        bytes[path_data_start..path_data_start + path_data.len()].copy_from_slice(&path_data);

        let (paths, truncated) =
            parse_volume_paths(&bytes, VERSION_WIN10, vi_offset, entry_count, section_size);

        assert_eq!(paths.len(), 32);
        assert!(truncated);
    }

    #[test]
    fn test_decomp_table_empty_sentinel_is_distinct_from_symbol_256() {
        assert_ne!(DECOMP_TABLE_EMPTY, 256);
    }

    fn utf16le(value: &str) -> Vec<u8> {
        value
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect()
    }
}
