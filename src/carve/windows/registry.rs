use crate::carve::windows::WindowsArtefactRecord;
use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PostCarveMetadata, PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::parsers::time::windows_filetime_to_datetime;
use crate::scanner::NormalizedHit;

// ------------------------------------------------------------------
// Layout constants
// ------------------------------------------------------------------

/// regf base block is exactly 4096 bytes (one cluster).
const BASE_BLOCK_SIZE: usize = 4096;
/// First hbin starts at this offset (immediately after the base block).
const HIVE_BINS_START_OFFSET: u64 = 4096;

const REGF_MAGIC: &[u8; 4] = b"regf";
const HBIN_MAGIC: &[u8; 4] = b"hbin";
const NK_MAGIC: &[u8; 2] = b"nk";

const PRIMARY_SEQ_OFFSET: usize = 0x04;
const SECONDARY_SEQ_OFFSET: usize = 0x08;
const TIMESTAMP_OFFSET: usize = 0x0C;
const MAJOR_VERSION_OFFSET: usize = 0x14;
const MINOR_VERSION_OFFSET: usize = 0x18;
const FILE_TYPE_OFFSET: usize = 0x1C;
const ROOT_CELL_OFFSET_FIELD: usize = 0x24;
const HIVE_BINS_DATA_SIZE_OFFSET: usize = 0x28;
const FILENAME_OFFSET: usize = 0x30;
const FILENAME_LEN_BYTES: usize = 64;

const NK_FLAG_ASCII_NAME: u16 = 0x0020;
const NK_NAME_LENGTH_OFFSET: usize = 0x48;
const NK_HEADER_SIZE: usize = 0x50;

/// Reasonable upper bound on registry hive bin data size we are willing to parse
/// even before the per-config max_size is applied. 1 GiB is well above the largest
/// hive ever observed in practice.
const MAX_PLAUSIBLE_HIVE_BINS_DATA_SIZE: u64 = 1 << 30;

// ------------------------------------------------------------------
// Public artefact type
// ------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegistryHiveArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    pub timestamp: Option<chrono::NaiveDateTime>,
    pub hive_name: Option<String>,
    pub hive_type: Option<String>,
    pub root_key_name: Option<String>,
}

// ------------------------------------------------------------------
// CarveHandler implementation
// ------------------------------------------------------------------

pub struct RegistryCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl RegistryCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for RegistryCarveHandler {
    fn file_type(&self) -> &str {
        "registry"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 4];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated regf header".to_string()));
        }
        if &buf == REGF_MAGIC {
            Ok(PreValidation::Proceed)
        } else {
            Ok(PreValidation::Reject("regf magic mismatch".to_string()))
        }
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;

        // Stage 1: read the base block and validate structure.
        let base_block = match read_base_block(ctx.evidence, hit.global_offset) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        };

        let header = match parse_base_block(&base_block) {
            Some(h) => h,
            None => return Ok(None),
        };

        // A real hive contains at least one hive bin. Reject obvious garbage
        // that declares less hive-bins data than even the 4-byte hbin magic;
        // this also guarantees the carved file (when not truncated) covers
        // the structural validation point at base + 0x1000.
        if header.hive_bins_data_size < HBIN_MAGIC.len() as u64 {
            return Ok(None);
        }

        // Compute on-disk total size = base block + hive bins data. Bound it
        // against both a defensive plausibility ceiling and the configured max.
        if header.hive_bins_data_size > MAX_PLAUSIBLE_HIVE_BINS_DATA_SIZE {
            tracing::debug!(
                offset = hit.global_offset,
                hive_bins_data_size = header.hive_bins_data_size,
                cap = MAX_PLAUSIBLE_HIVE_BINS_DATA_SIZE,
                "registry hit rejected: hive_bins_data_size exceeds plausibility cap"
            );
            return Ok(None);
        }
        let total_size = HIVE_BINS_START_OFFSET
            .checked_add(header.hive_bins_data_size)
            .ok_or_else(|| CarveError::Invalid("registry size overflows u64".into()))?;

        // Pre-flight structural check: confirm that the hive-bins area starts
        // with a valid `hbin` magic. Doing this before streaming lets us
        // reject false-positive `regf` matches without any disk I/O. A
        // past-boundary read is tolerated — that hit will become a truncated
        // carve below.
        let hbin_status = check_hbin_magic(ctx.evidence, hit.global_offset)?;
        if hbin_status == HbinCheck::Mismatch {
            tracing::debug!(
                offset = hit.global_offset,
                "registry hit rejected: hbin magic missing at base + 0x1000"
            );
            return Ok(None);
        }

        let max_size = if self.max_size > 0 {
            self.max_size
        } else {
            total_size
        };
        if total_size > max_size {
            // Hive declares more data than max_size allows: skip rather than
            // emit a truncated, structurally-incomplete carve.
            tracing::debug!(
                offset = hit.global_offset,
                total_size,
                max_size,
                "registry hit rejected: total_size exceeds configured max_size"
            );
            return Ok(None);
        }
        if total_size < self.min_size {
            tracing::debug!(
                offset = hit.global_offset,
                total_size,
                min_size = self.min_size,
                "registry hit rejected: total_size below configured min_size"
            );
            return Ok(None);
        }

        // Stage 2: stream the full hive (base block + hive bins) to disk while
        // hashing in the same pass. Re-reads the base block bytes so the file
        // contents on disk bit-match the source, including any unparsed trailer.
        let mut stream = CarveStream::new(ctx, hit.global_offset, max_size, full_path.clone());

        let mut truncated = false;
        let mut errors: Vec<String> = Vec::new();

        match stream.consume_remaining(total_size) {
            Ok(()) => {}
            Err(CarveError::Truncated) | Err(CarveError::Eof) => {
                truncated = true;
                errors.push("hive truncated by evidence boundary".to_string());
            }
            Err(other) => {
                stream.discard();
                return Err(other);
            }
        }

        let written = stream.bytes_written();

        // Structural validation beyond the base block: the pre-flight check
        // above already rejected mismatched hbin magic. The remaining case is
        // `PastBoundary`, which can only co-occur with a truncated stream
        // (the hbin offset lies past the evidence end, so `consume_remaining`
        // must have hit Eof first). Bytes are still preserved with
        // `validated=false` so analysts see the partial evidence.
        let hbin_ok = hbin_status == HbinCheck::Match;
        debug_assert!(hbin_ok || truncated, "PastBoundary implies truncation");

        let (size, md5_hex, sha256_hex, mut writer) = stream.finalize()?;

        // This second min-size check fires only in the truncation case, where
        // streaming was cut short and `size < total_size`. Drop sub-min-size
        // partial carves rather than emitting unusable evidence.
        if size < self.min_size {
            writer.discard();
            return Ok(None);
        }

        if !hbin_ok {
            errors.push("hbin magic missing at base + 0x1000 (truncated)".to_string());
        }

        // Stage 4: best-effort root-key name extraction. Failure here is not
        // fatal — the carved bytes are still valid evidence.
        let root_key_name = if hbin_ok {
            extract_root_key_name(
                ctx.evidence,
                hit.global_offset,
                header.root_cell_offset,
                written,
            )
        } else {
            None
        };

        let timestamp = windows_filetime_to_datetime(header.timestamp);
        let hive_name = decode_embedded_filename(&base_block);
        let hive_type = hive_type_from_name(hive_name.as_deref());

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        let validated = !truncated && hbin_ok;

        let file = CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size,
            md5: md5_hex,
            sha256: sha256_hex,
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
            is_duplicate: false,
            duplicate_of_offset: None,
        };

        let artefact = RegistryHiveArtefact {
            run_id: ctx.run_id.to_string(),
            offset: hit.global_offset,
            size,
            timestamp,
            hive_name,
            hive_type,
            root_key_name,
        };

        Ok(Some(PendingCarve::new(file, writer).with_post_metadata(
            vec![PostCarveMetadata::WindowsArtefact(
                WindowsArtefactRecord::RegistryHive(artefact),
            )],
        )))
    }
}

// ------------------------------------------------------------------
// Internal parsing
// ------------------------------------------------------------------

#[derive(Debug)]
struct BaseBlockHeader {
    timestamp: u64,
    root_cell_offset: u32,
    hive_bins_data_size: u64,
}

fn read_base_block(
    evidence: &dyn EvidenceSource,
    offset: u64,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; BASE_BLOCK_SIZE];
    let mut filled = 0usize;
    while filled < BASE_BLOCK_SIZE {
        let n = evidence
            .read_at(offset + filled as u64, &mut buf[filled..])
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        filled += n;
    }
    if &buf[..4] != REGF_MAGIC {
        return Ok(None);
    }
    Ok(Some(buf))
}

fn parse_base_block(buf: &[u8]) -> Option<BaseBlockHeader> {
    if buf.len() < BASE_BLOCK_SIZE {
        return None;
    }
    if &buf[..4] != REGF_MAGIC {
        return None;
    }
    let primary = read_u32(buf, PRIMARY_SEQ_OFFSET)?;
    let secondary = read_u32(buf, SECONDARY_SEQ_OFFSET)?;
    let timestamp = read_u64(buf, TIMESTAMP_OFFSET)?;
    let major = read_u32(buf, MAJOR_VERSION_OFFSET)?;
    let minor = read_u32(buf, MINOR_VERSION_OFFSET)?;
    // Known major versions are 1; minor is 0..=6 in observed hives.
    if major != 1 || minor > 15 {
        return None;
    }
    // file_type 0 = primary hive, 1 = transaction log, 2 = external.
    // We carve primary hives plus log files (which share the regf magic).
    let file_type = read_u32(buf, FILE_TYPE_OFFSET)?;
    if file_type > 2 {
        return None;
    }
    // Freshly initialised transaction-log files (`file_type == 1`) can
    // legitimately have one zero sequence number until the first commit
    // cycle. Primary and external hives keep the stricter invariant that
    // both sequence numbers must be non-zero.
    if primary == 0 && secondary == 0 {
        return None;
    }
    if file_type != 1 && (primary == 0 || secondary == 0) {
        return None;
    }
    let root_cell_offset = read_u32(buf, ROOT_CELL_OFFSET_FIELD)?;
    let hive_bins_data_size = u64::from(read_u32(buf, HIVE_BINS_DATA_SIZE_OFFSET)?);

    Some(BaseBlockHeader {
        timestamp,
        root_cell_offset,
        hive_bins_data_size,
    })
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = buf.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(buf: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let slice = buf.get(offset..end)?;
    Some(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn read_u16(buf: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = buf.get(offset..end)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Decode the UTF-16LE filename embedded in the base block at offset 0x30,
/// stopping at the first NUL code unit. Returns `None` if the field is empty
/// or contains no decodable text.
fn decode_embedded_filename(base_block: &[u8]) -> Option<String> {
    let end = FILENAME_OFFSET + FILENAME_LEN_BYTES;
    if base_block.len() < end {
        return None;
    }
    let raw = &base_block[FILENAME_OFFSET..end];
    let mut units: Vec<u16> = Vec::with_capacity(FILENAME_LEN_BYTES / 2);
    for pair in raw.chunks_exact(2) {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    if units.is_empty() {
        return None;
    }
    let decoded = String::from_utf16_lossy(&units);
    let trimmed = decoded.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Map an embedded hive name (often a path like `\??\C:\Windows\System32\config\SYSTEM`)
/// to a canonical hive-type label by matching its trailing path component against
/// a list of well-known Windows hives.
fn hive_type_from_name(name: Option<&str>) -> Option<String> {
    let raw = name?;
    // Strip path components (both forward and back slashes occur).
    let leaf = raw.rsplit(['\\', '/']).next().unwrap_or(raw).trim();
    if leaf.is_empty() {
        return None;
    }
    let upper = leaf.to_ascii_uppercase();

    // Known canonical hive types (as the trailing filename component).
    const KNOWN: &[&str] = &[
        "SAM",
        "SYSTEM",
        "SOFTWARE",
        "SECURITY",
        "DEFAULT",
        "NTUSER.DAT",
        "USRCLASS.DAT",
        "COMPONENTS",
        "DRIVERS",
        "BCD",
        "BCD-TEMPLATE",
        "ELAM",
        "BBI",
        "USERDIFF",
        "AMCACHE.HVE",
        "SYSCACHE.HVE",
        "SCHEMA.DAT",
        "SETTINGS.DAT",
    ];
    for &candidate in KNOWN {
        if upper == candidate {
            return Some(candidate.to_string());
        }
    }
    // Fall back to the raw leaf so analysts still see something useful for
    // hives not in the canonical set.
    Some(leaf.to_string())
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum HbinCheck {
    /// `hbin` magic is present at base + 0x1000.
    Match,
    /// 4 bytes were readable at base + 0x1000 but did not match `hbin`.
    Mismatch,
    /// Fewer than 4 bytes were readable — the offset is past the evidence
    /// boundary. This indicates a hit at the tail of the image and is
    /// expected to result in a truncated carve.
    PastBoundary,
}

/// Pre-flight structural check: read the 4 bytes at base + 0x1000 once and
/// classify them. Bubbles up evidence-source errors so callers can decide
/// whether to fail or continue.
fn check_hbin_magic(
    evidence: &dyn EvidenceSource,
    base_offset: u64,
) -> Result<HbinCheck, CarveError> {
    let read_at = base_offset
        .checked_add(HIVE_BINS_START_OFFSET)
        .ok_or_else(|| CarveError::Invalid("hbin offset overflows u64".into()))?;
    let mut buf = [0u8; 4];
    let n = evidence
        .read_at(read_at, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < 4 {
        Ok(HbinCheck::PastBoundary)
    } else if &buf == HBIN_MAGIC {
        Ok(HbinCheck::Match)
    } else {
        Ok(HbinCheck::Mismatch)
    }
}

/// Best-effort root key name extraction. Reads the first cell of the first
/// hbin and decodes its name field. Returns `None` on any structural problem.
/// The caller must ensure the hbin magic at `base_offset + 0x1000` is sound;
/// this function does not re-validate it.
fn extract_root_key_name(
    evidence: &dyn EvidenceSource,
    base_offset: u64,
    root_cell_offset: u32,
    carved_size: u64,
) -> Option<String> {
    // Cell offset is relative to the start of the hive bins area.
    let cell_off = HIVE_BINS_START_OFFSET.checked_add(u64::from(root_cell_offset))?;
    // Bound the carved region against the full 32-byte hbin header, since the
    // first cell follows it.
    if carved_size < HIVE_BINS_START_OFFSET + 32 {
        return None;
    }

    // Read the cell header (4-byte signed size field).
    if cell_off.checked_add(4)? > carved_size {
        return None;
    }
    let mut size_buf = [0u8; 4];
    let cell_read_at = base_offset.checked_add(cell_off)?;
    let n = evidence.read_at(cell_read_at, &mut size_buf).ok()?;
    if n < 4 {
        return None;
    }
    let raw_size = i32::from_le_bytes(size_buf);
    // Allocated cells have negative size; free cells have positive size.
    // `unsigned_abs` is overflow-safe for `i32::MIN` (where unary `-` would panic).
    let cell_data_size = if raw_size < 0 {
        raw_size.unsigned_abs()
    } else {
        return None;
    };
    if !(NK_HEADER_SIZE as u32 + 4..=0x10_0000).contains(&cell_data_size) {
        return None;
    }
    // Bound by the carved size.
    if cell_off.checked_add(u64::from(cell_data_size))? > carved_size {
        return None;
    }

    // Read the nk header body (everything after the 4-byte size field).
    // Cap the read at the nk header plus the maximum supported key-name
    // length (Windows: 255 chars). Even if `cell_data_size` claims up to
    // 1 MiB, only the header + name bytes are needed here — avoids per-hit
    // megabyte allocations on false-positive `regf` matches.
    const MAX_NK_NAME_BYTES: usize = 255 * 2;
    let body_cap = NK_HEADER_SIZE + MAX_NK_NAME_BYTES;
    let body_len = (cell_data_size as usize).saturating_sub(4).min(body_cap);
    if body_len < NK_HEADER_SIZE {
        return None;
    }
    let mut body = vec![0u8; body_len];
    let body_read_at = base_offset.checked_add(cell_off)?.checked_add(4)?;
    let n = evidence.read_at(body_read_at, &mut body).ok()?;
    if n < NK_HEADER_SIZE {
        return None;
    }
    body.truncate(n);
    if &body[..2] != NK_MAGIC {
        return None;
    }
    let flags = read_u16(&body, 2)?;
    let name_length = read_u16(&body, NK_NAME_LENGTH_OFFSET)? as usize;
    if name_length == 0 {
        return None;
    }
    let name_start = NK_HEADER_SIZE;
    let name_end = name_start.checked_add(name_length)?;
    if name_end > body.len() {
        return None;
    }
    let raw = &body[name_start..name_end];
    if flags & NK_FLAG_ASCII_NAME != 0 {
        // Windows stores this variant as extended-ASCII (CP-1252-ish), not
        // UTF-8. Map bytes >= 0x80 to '?' so we never silently produce U+FFFD
        // and never misinterpret high bytes as part of a multi-byte sequence.
        let s: String = raw
            .iter()
            .map(|&b| if b < 0x80 { b as char } else { '?' })
            .collect();
        let s = s.trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    } else {
        if !name_length.is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_hive(embedded: &str, root_name: &str) -> Vec<u8> {
        build_hive_with_type_and_seq(embedded, root_name, 0, 1, 1)
    }

    fn build_hive_with_seq(
        embedded: &str,
        root_name: &str,
        primary: u32,
        secondary: u32,
    ) -> Vec<u8> {
        build_hive_with_type_and_seq(embedded, root_name, 0, primary, secondary)
    }

    fn build_log_hive_with_seq(
        embedded: &str,
        root_name: &str,
        primary: u32,
        secondary: u32,
    ) -> Vec<u8> {
        build_hive_with_type_and_seq(embedded, root_name, 1, primary, secondary)
    }

    fn build_hive_with_type_and_seq(
        embedded: &str,
        root_name: &str,
        file_type: u32,
        primary: u32,
        secondary: u32,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; BASE_BLOCK_SIZE + 4096];
        // base block
        buf[0..4].copy_from_slice(REGF_MAGIC);
        buf[PRIMARY_SEQ_OFFSET..PRIMARY_SEQ_OFFSET + 4].copy_from_slice(&primary.to_le_bytes());
        buf[SECONDARY_SEQ_OFFSET..SECONDARY_SEQ_OFFSET + 4]
            .copy_from_slice(&secondary.to_le_bytes());
        // FILETIME: 2024-01-01T00:00:00Z = 133485984000000000
        let filetime: u64 = 133_485_984_000_000_000;
        buf[TIMESTAMP_OFFSET..TIMESTAMP_OFFSET + 8].copy_from_slice(&filetime.to_le_bytes());
        buf[MAJOR_VERSION_OFFSET..MAJOR_VERSION_OFFSET + 4].copy_from_slice(&1u32.to_le_bytes());
        buf[MINOR_VERSION_OFFSET..MINOR_VERSION_OFFSET + 4].copy_from_slice(&5u32.to_le_bytes());
        buf[FILE_TYPE_OFFSET..FILE_TYPE_OFFSET + 4].copy_from_slice(&file_type.to_le_bytes());
        // file_format = 1
        buf[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
        buf[ROOT_CELL_OFFSET_FIELD..ROOT_CELL_OFFSET_FIELD + 4]
            .copy_from_slice(&0x20u32.to_le_bytes());
        buf[HIVE_BINS_DATA_SIZE_OFFSET..HIVE_BINS_DATA_SIZE_OFFSET + 4]
            .copy_from_slice(&0x1000u32.to_le_bytes());
        // clustering
        buf[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
        // embedded filename UTF-16LE
        let mut pos = FILENAME_OFFSET;
        for unit in embedded.encode_utf16().take(FILENAME_LEN_BYTES / 2) {
            buf[pos..pos + 2].copy_from_slice(&unit.to_le_bytes());
            pos += 2;
        }
        // hbin
        let hbin = BASE_BLOCK_SIZE;
        buf[hbin..hbin + 4].copy_from_slice(HBIN_MAGIC);
        buf[hbin + 4..hbin + 8].copy_from_slice(&0u32.to_le_bytes()); // hbin offset
        buf[hbin + 8..hbin + 12].copy_from_slice(&0x1000u32.to_le_bytes()); // hbin size
        buf[hbin + 0x14..hbin + 0x1C].copy_from_slice(&filetime.to_le_bytes());
        // nk root cell at hbin + 0x20
        let cell = hbin + 0x20;
        let name_bytes = root_name.as_bytes();
        let body_len = NK_HEADER_SIZE + name_bytes.len();
        let total = 4 + body_len;
        let pad = (8 - (total % 8)) % 8;
        let cell_size = total + pad;
        let neg = -(cell_size as i32);
        buf[cell..cell + 4].copy_from_slice(&neg.to_le_bytes());
        buf[cell + 4..cell + 6].copy_from_slice(NK_MAGIC);
        // ROOT_KEY (0x000C) | ASCII_NAME (0x0020) = 0x002C
        buf[cell + 6..cell + 8].copy_from_slice(&0x002Cu16.to_le_bytes());
        buf[cell + 8..cell + 16].copy_from_slice(&filetime.to_le_bytes());
        // name_length
        buf[cell + 4 + NK_NAME_LENGTH_OFFSET..cell + 4 + NK_NAME_LENGTH_OFFSET + 2]
            .copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        let name_pos = cell + 4 + NK_HEADER_SIZE;
        buf[name_pos..name_pos + name_bytes.len()].copy_from_slice(name_bytes);
        buf
    }

    #[test]
    fn parses_base_block() {
        let buf = build_minimal_hive("SYSTEM", "ROOT");
        let h = parse_base_block(&buf).expect("parse");
        assert_eq!(h.hive_bins_data_size, 0x1000);
        assert_eq!(h.root_cell_offset, 0x20);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut buf = build_minimal_hive("SYSTEM", "ROOT");
        buf[0] = b'X';
        assert!(parse_base_block(&buf).is_none());
    }

    #[test]
    fn rejects_zero_sequence() {
        let buf = build_hive_with_seq("SYSTEM", "ROOT", 0, 0);
        assert!(parse_base_block(&buf).is_none());
    }

    #[test]
    fn accepts_freshly_initialized_log_seq() {
        // A freshly initialised transaction-log file may have one zero
        // sequence number until its first commit cycle.
        let buf = build_log_hive_with_seq("SYSTEM.LOG1", "ROOT", 1, 0);
        assert!(parse_base_block(&buf).is_some());
        let buf = build_log_hive_with_seq("SYSTEM.LOG2", "ROOT", 0, 1);
        assert!(parse_base_block(&buf).is_some());
    }

    #[test]
    fn rejects_primary_hive_with_zero_secondary_sequence() {
        let buf = build_hive_with_seq("SYSTEM", "ROOT", 1, 0);
        assert!(parse_base_block(&buf).is_none());
    }

    #[test]
    fn rejects_implausible_version() {
        let mut buf = build_minimal_hive("SYSTEM", "ROOT");
        buf[MAJOR_VERSION_OFFSET..MAJOR_VERSION_OFFSET + 4].copy_from_slice(&99u32.to_le_bytes());
        assert!(parse_base_block(&buf).is_none());
    }

    #[test]
    fn embedded_filename_decodes() {
        let buf = build_minimal_hive("NTUSER.DAT", "ROOT");
        assert_eq!(
            decode_embedded_filename(&buf).as_deref(),
            Some("NTUSER.DAT")
        );
    }

    #[test]
    fn embedded_filename_handles_path_with_backslashes() {
        // Field is 64 bytes (32 UTF-16LE units); use a path that fits.
        let buf = build_minimal_hive(r"\config\SYSTEM", "ROOT");
        let name = decode_embedded_filename(&buf).expect("name");
        assert!(name.ends_with("SYSTEM"));
        assert_eq!(hive_type_from_name(Some(&name)), Some("SYSTEM".to_string()));
    }

    #[test]
    fn hive_type_canonical_set() {
        assert_eq!(hive_type_from_name(Some("SAM")), Some("SAM".to_string()));
        assert_eq!(
            hive_type_from_name(Some(r"\Windows\System32\config\SYSTEM")),
            Some("SYSTEM".to_string())
        );
        assert_eq!(
            hive_type_from_name(Some(r"Users\bob\ntuser.dat")),
            Some("NTUSER.DAT".to_string())
        );
        assert_eq!(
            hive_type_from_name(Some("Unknown.Hive")),
            Some("Unknown.Hive".to_string())
        );
        assert_eq!(hive_type_from_name(None), None);
        assert_eq!(hive_type_from_name(Some("")), None);
    }

    #[test]
    fn extracts_ascii_root_key_name() {
        struct Slice(Vec<u8>);
        impl crate::evidence::EvidenceSource for Slice {
            fn len(&self) -> u64 {
                self.0.len() as u64
            }
            fn read_at(
                &self,
                offset: u64,
                buf: &mut [u8],
            ) -> Result<usize, crate::evidence::EvidenceError> {
                let start = offset as usize;
                if start >= self.0.len() {
                    return Ok(0);
                }
                let n = buf.len().min(self.0.len() - start);
                buf[..n].copy_from_slice(&self.0[start..start + n]);
                Ok(n)
            }
        }
        let buf = build_minimal_hive("SYSTEM", "$$$PROTO.HIV");
        let total = buf.len() as u64;
        let s = Slice(buf);
        let got = extract_root_key_name(&s, 0, 0x20, total);
        assert_eq!(got.as_deref(), Some("$$$PROTO.HIV"));
    }
}
