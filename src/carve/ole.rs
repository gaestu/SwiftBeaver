//! OLE Compound File Binary (CFB) carving handler.
//!
//! OLE/CFB is used by Microsoft Office 97-2003 formats (DOC, XLS, PPT, MSG).
//! The file structure uses a FAT-based sector allocation scheme.
//!
//! Signature: D0 CF 11 E0 A1 B1 1A E1

use std::fs::File;

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// OLE/CFB magic signature
const OLE_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// CFB version 3 (512-byte sectors)
const VERSION_3: u16 = 3;
/// CFB version 4 (4096-byte sectors)  
const VERSION_4: u16 = 4;

/// Sector size for version 3
const SECTOR_SIZE_V3: u64 = 512;
/// Sector size for version 4
const SECTOR_SIZE_V4: u64 = 4096;
/// Maximum FAT sectors allowed (supports files up to ~128MB)
const MAX_FAT_SECTORS: u32 = 1000;

pub struct OleCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
    allowed_kinds: Option<Vec<String>>,
}

impl OleCarveHandler {
    pub fn new(
        extension: String,
        min_size: u64,
        max_size: u64,
        allowed_kinds: Option<Vec<String>>,
    ) -> Self {
        let allowed_kinds =
            allowed_kinds.map(|kinds| kinds.into_iter().map(|v| v.to_ascii_lowercase()).collect());
        Self {
            extension,
            min_size,
            max_size,
            allowed_kinds,
        }
    }
}

/// Parse OLE/CFB header and calculate file size.
///
/// Header structure (512 bytes for v3, 4096 for v4):
/// - Bytes 0-7: Signature
/// - Bytes 8-23: CLSID (usually zeros)
/// - Bytes 24-25: Minor version
/// - Bytes 26-27: Major version (3 or 4)
/// - Bytes 28-29: Byte order (0xFFFE = little-endian)
/// - Bytes 30-31: Sector size power (9 for 512, 12 for 4096)
/// - Bytes 32-33: Mini sector size power
/// - Bytes 34-39: Reserved
/// - Bytes 40-43: Total sectors in FAT (v4 only reliable)
/// - Bytes 44-47: First directory sector SECID
/// - Bytes 48-51: Transaction signature
/// - Bytes 52-55: Mini stream cutoff size
/// - Bytes 56-59: First mini FAT sector
/// - Bytes 60-63: Number of mini FAT sectors
/// - Bytes 64-67: First DIFAT sector
/// - Bytes 68-71: Number of DIFAT sectors
/// - Bytes 72-511: DIFAT array (109 entries, each 4 bytes)
fn parse_ole_header(header: &[u8]) -> Result<(u64, u64), CarveError> {
    if header.len() < 512 {
        return Err(CarveError::Invalid("ole header too short".to_string()));
    }

    // Check signature
    if header[0..8] != OLE_SIGNATURE {
        return Err(CarveError::Invalid("ole signature mismatch".to_string()));
    }

    // Check byte order
    let byte_order = u16::from_le_bytes([header[28], header[29]]);
    if byte_order != 0xFFFE {
        return Err(CarveError::Invalid("ole byte order invalid".to_string()));
    }

    // Get version and sector size
    let major_version = u16::from_le_bytes([header[26], header[27]]);
    let sector_power = u16::from_le_bytes([header[30], header[31]]);

    let (sector_size, header_size) = match major_version {
        VERSION_3 => {
            if sector_power != 9 {
                return Err(CarveError::Invalid(
                    "ole v3 sector power invalid".to_string(),
                ));
            }
            (SECTOR_SIZE_V3, SECTOR_SIZE_V3)
        }
        VERSION_4 => {
            if sector_power != 12 {
                return Err(CarveError::Invalid(
                    "ole v4 sector power invalid".to_string(),
                ));
            }
            (SECTOR_SIZE_V4, SECTOR_SIZE_V4)
        }
        _ => {
            return Err(CarveError::Invalid(format!(
                "ole version {} not supported",
                major_version
            )));
        }
    };

    // For v3, we need to calculate total sectors from FAT
    // The key insight: the file size = header + (highest_sector_id + 1) * sector_size

    // Read number of FAT sectors (this tells us how many sectors contain FAT entries)
    let num_fat_sectors = u32::from_le_bytes([header[44], header[45], header[46], header[47]]);

    // Cap FAT sectors to reject implausibly large files
    if num_fat_sectors > MAX_FAT_SECTORS {
        return Err(CarveError::Invalid(format!(
            "ole num_fat_sectors {} exceeds limit {}",
            num_fat_sectors, MAX_FAT_SECTORS
        )));
    }

    // Read first directory sector
    let first_dir_sector = u32::from_le_bytes([header[48], header[49], header[50], header[51]]);

    // Read number of DIFAT sectors (extended FAT)
    let num_difat_sectors = u32::from_le_bytes([header[68], header[69], header[70], header[71]]);

    // Find the highest FAT sector location from the DIFAT array in header
    let mut max_fat_sector: u32 = 0;
    for i in 0..109 {
        let offset = 76 + i * 4;
        if offset + 4 > header.len() {
            break;
        }
        let sector_id = u32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ]);
        // Valid sector IDs are < 0xFFFFFFFA (special values like ENDOFCHAIN are >= that)
        if sector_id < 0xFFFFFFFA && sector_id > max_fat_sector {
            max_fat_sector = sector_id;
        }
    }

    // The file must contain at least:
    // - Header (1 sector)
    // - FAT sectors (num_fat_sectors)
    // - DIFAT sectors (num_difat_sectors)
    // - Directory (at least 1 sector, starting at first_dir_sector)
    // - Data sectors
    //
    // A simple and often accurate approach: the highest sector used is typically
    // around the directory sector location + data. The number of FAT sectors
    // tells us how many sectors the file can index.
    //
    // Each FAT sector can index (sector_size / 4) sectors.
    // So max possible sectors = num_fat_sectors * (sector_size / 4)

    let entries_per_fat_sector = sector_size / 4;
    let max_indexed_sectors = num_fat_sectors as u64 * entries_per_fat_sector;

    // The actual file size is header + all used sectors
    // We use the FAT capacity as upper bound, but also check first_dir_sector
    // as a more realistic estimate
    let mut estimated_sectors = if first_dir_sector < 0xFFFFFFFA {
        // Use directory sector as base and add buffer for data
        // Most small files have data after the directory
        (first_dir_sector as u64 + 1).max(max_indexed_sectors.min(500))
    } else {
        // No directory found, use FAT capacity (capped)
        max_indexed_sectors.min(1000)
    };

    // Ensure we include all FAT sectors
    if max_fat_sector > 0 {
        estimated_sectors = estimated_sectors.max(max_fat_sector as u64 + 1);
    }

    // Add DIFAT sectors
    estimated_sectors += num_difat_sectors as u64;

    let estimated_size = header_size + (estimated_sectors * sector_size);

    Ok((estimated_size, sector_size))
}

/// Try to get a more accurate size by reading FAT entries and finding the highest used sector
fn refine_ole_size(
    evidence: &dyn EvidenceSource,
    base_offset: u64,
    header: &[u8],
    sector_size: u64,
    max_size: u64,
) -> Result<u64, CarveError> {
    let header_size = 512u64; // Always 512 for header

    // Read DIFAT entries from header to find FAT sector locations
    let mut fat_sectors = Vec::new();

    for i in 0..109 {
        let offset = 76 + i * 4;
        if offset + 4 > header.len() {
            break;
        }
        let sector_id = u32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ]);
        if sector_id < 0xFFFFFFFA {
            fat_sectors.push(sector_id);
        } else {
            break;
        }
    }

    if fat_sectors.is_empty() {
        // No FAT sectors found, return minimal size
        return Ok(header_size + sector_size);
    }

    // Read all FAT sectors to find the highest sector that's in use
    let mut highest_used_sector: u32 = 0;

    // Track FAT sectors themselves as used
    for &fat_sec in &fat_sectors {
        if fat_sec > highest_used_sector {
            highest_used_sector = fat_sec;
        }
    }

    // Read first directory sector from header
    let first_dir_sector = u32::from_le_bytes([header[48], header[49], header[50], header[51]]);
    if first_dir_sector < 0xFFFFFFFA && first_dir_sector > highest_used_sector {
        highest_used_sector = first_dir_sector;
    }

    // Read each FAT sector and scan for the highest sector ID that is allocated
    // A sector is "used" if its FAT entry is not FREESECT (0xFFFFFFFF)
    for (fat_index, &fat_sector_id) in fat_sectors.iter().enumerate() {
        let fat_file_offset = header_size + (fat_sector_id as u64 * sector_size);

        if fat_file_offset + sector_size > max_size {
            break;
        }

        // Read this FAT sector directly from evidence
        let mut fat_data = vec![0u8; sector_size as usize];
        let read_offset = base_offset + fat_file_offset;

        match evidence.read_at(read_offset, &mut fat_data) {
            Ok(n) if n == sector_size as usize => {}
            _ => break, // Couldn't read full sector, stop
        }

        // Now parse the FAT entries from this sector
        let entries_per_sector = (sector_size / 4) as usize;
        let base_sector_id = fat_index * entries_per_sector;

        for entry_idx in 0..entries_per_sector {
            let byte_offset = entry_idx * 4;
            if byte_offset + 4 > fat_data.len() {
                break;
            }

            let fat_entry = u32::from_le_bytes([
                fat_data[byte_offset],
                fat_data[byte_offset + 1],
                fat_data[byte_offset + 2],
                fat_data[byte_offset + 3],
            ]);

            // If this entry is not FREESECT (0xFFFFFFFF), this sector index is used
            // FREESECT = 0xFFFFFFFF, ENDOFCHAIN = 0xFFFFFFFE, FATSECT = 0xFFFFFFFD, etc.
            if fat_entry != 0xFFFFFFFF {
                let sector_index = (base_sector_id + entry_idx) as u32;
                if sector_index > highest_used_sector && sector_index < 0xFFFFFFFA {
                    highest_used_sector = sector_index;
                }
                // Also check where this entry points to (the chain)
                if fat_entry < 0xFFFFFFFA && fat_entry > highest_used_sector {
                    highest_used_sector = fat_entry;
                }
            }
        }
    }

    // File size = header + (highest_sector + 1) * sector_size
    let total_size = header_size + ((highest_used_sector as u64 + 1) * sector_size);

    Ok(total_size.min(max_size))
}

fn classify_ole_kind(
    evidence: &dyn EvidenceSource,
    base_offset: u64,
    header: &[u8],
    sector_size: u64,
    max_size: u64,
) -> Option<&'static str> {
    if header.len() < 512 {
        return None;
    }
    let first_dir_sector = u32::from_le_bytes([header[48], header[49], header[50], header[51]]);
    if first_dir_sector >= 0xFFFFFFFA {
        return None;
    }
    let fat = read_fat(evidence, base_offset, header, sector_size, max_size).ok()?;
    let mut current = first_dir_sector;
    let mut visited = 0u32;
    let mut found_doc = false;
    let mut found_xls = false;
    let mut found_ppt = false;

    while current < 0xFFFFFFFA && visited < 1024 {
        let offset = base_offset + 512u64 + (current as u64 * sector_size);
        if offset + sector_size > base_offset.saturating_add(max_size) {
            break;
        }
        let mut buf = vec![0u8; sector_size as usize];
        let n = evidence.read_at(offset, &mut buf).ok()?;
        if n < sector_size as usize {
            break;
        }
        for entry in buf.chunks(128) {
            if entry.len() < 128 {
                continue;
            }
            let name_len = u16::from_le_bytes([entry[64], entry[65]]) as usize;
            if !(2..=64).contains(&name_len) {
                continue;
            }
            let entry_type = entry[66];
            if entry_type != 2 {
                continue;
            }
            let name = decode_utf16le(&entry[..name_len.saturating_sub(2)]);
            if name == "WordDocument" {
                found_doc = true;
            } else if name == "Workbook" || name == "Book" {
                found_xls = true;
            } else if name == "PowerPoint Document" {
                found_ppt = true;
            }
        }

        let next = fat.get(current as usize).copied().unwrap_or(0xFFFFFFFE);
        if next >= 0xFFFFFFFA {
            break;
        }
        current = next;
        visited += 1;
    }

    if found_doc {
        Some("doc")
    } else if found_xls {
        Some("xls")
    } else if found_ppt {
        Some("ppt")
    } else {
        None
    }
}

fn read_fat(
    evidence: &dyn EvidenceSource,
    base_offset: u64,
    header: &[u8],
    sector_size: u64,
    max_size: u64,
) -> Result<Vec<u32>, CarveError> {
    let mut fat_sectors = Vec::new();

    for i in 0..109 {
        let offset = 76 + i * 4;
        if offset + 4 > header.len() {
            break;
        }
        let sector_id = u32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ]);
        if sector_id < 0xFFFFFFFA {
            fat_sectors.push(sector_id);
        } else {
            break;
        }
    }

    if fat_sectors.is_empty() {
        return Err(CarveError::Invalid("ole fat sectors missing".to_string()));
    }

    let mut fat_entries = Vec::new();
    for sector_id in fat_sectors {
        let file_offset = 512u64 + (sector_id as u64 * sector_size);
        if file_offset + sector_size > max_size {
            break;
        }
        let read_offset = base_offset + file_offset;
        let mut buf = vec![0u8; sector_size as usize];
        let n = evidence
            .read_at(read_offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < sector_size as usize {
            break;
        }
        for chunk in buf.chunks(4) {
            if chunk.len() < 4 {
                break;
            }
            fat_entries.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
    }
    Ok(fat_entries)
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks(2) {
        if chunk.len() < 2 {
            break;
        }
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        if value == 0 {
            break;
        }
        out.push(value);
    }
    String::from_utf16_lossy(&out)
}

impl CarveHandler for OleCarveHandler {
    fn file_type(&self) -> &str {
        "ole"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let (mut full_path, mut rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let file = File::create(&full_path)?;
        let effective_max = if self.max_size > 0 {
            self.max_size
        } else {
            512 * 1024 * 1024 // 512 MiB default limit
        };
        let mut stream = CarveStream::new(ctx.evidence, hit.global_offset, effective_max, file);

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let mut classified_kind: Option<&'static str> = None;

        let result: Result<u64, CarveError> = (|| {
            // Read OLE header (512 bytes minimum)
            let header = stream.read_exact(512)?;

            // Parse and validate header
            let (_estimated_size, sector_size) = parse_ole_header(&header)?;
            classified_kind = classify_ole_kind(
                ctx.evidence,
                hit.global_offset,
                &header,
                sector_size,
                effective_max,
            );

            // Try to refine size estimate by reading FAT from evidence directly
            let target_size = refine_ole_size(
                ctx.evidence,
                hit.global_offset,
                &header,
                sector_size,
                effective_max,
            )?;

            // Apply max_size limit
            let target_size = target_size.min(effective_max);

            // Read remaining data
            let already_read = stream.bytes_written();
            let remaining = target_size.saturating_sub(already_read);

            if remaining > 0 {
                match stream.read_exact(remaining as usize) {
                    Ok(_) => {}
                    Err(CarveError::Eof) | Err(CarveError::Truncated) => {
                        // Partial read is OK
                    }
                    Err(e) => return Err(e),
                }
            }

            validated = true;
            Ok(target_size)
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_) => {
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex) = stream.finish()?;

        // Check minimum size
        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let mut file_type = self.file_type().to_string();
        let mut extension = self.extension.clone();

        if let Some(kind) = classified_kind {
            file_type = kind.to_string();
            extension = kind.to_string();
            if file_type != self.file_type()
                && let Ok((new_path, new_rel)) =
                    output_path(ctx.output_root, &file_type, &extension, hit.global_offset)
                && std::fs::rename(&full_path, &new_path).is_ok()
            {
                full_path = new_path;
                rel_path = new_rel;
            }
        }

        if let Some(allowed) = &self.allowed_kinds
            && !allowed.contains(&file_type)
        {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        // Check if we hit max_size
        if self.max_size > 0 && size >= self.max_size {
            truncated = true;
            if !errors.iter().any(|e| e.contains("max_size")) {
                errors.push("max_size reached".to_string());
            }
        }

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type,
            path: rel_path,
            extension,
            global_start: hit.global_offset,
            global_end,
            size,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{EvidenceError, EvidenceSource};
    use tempfile::tempdir;

    struct SliceEvidence {
        data: Vec<u8>,
    }

    impl EvidenceSource for SliceEvidence {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
            if offset as usize >= self.data.len() {
                return Ok(0);
            }
            let max = self.data.len() - offset as usize;
            let to_copy = buf.len().min(max);
            buf[..to_copy].copy_from_slice(&self.data[offset as usize..offset as usize + to_copy]);
            Ok(to_copy)
        }
    }

    fn create_minimal_ole() -> Vec<u8> {
        let mut ole = vec![0u8; 512];

        // Signature
        ole[0..8].copy_from_slice(&OLE_SIGNATURE);

        // Minor version
        ole[24..26].copy_from_slice(&0x003Eu16.to_le_bytes());

        // Major version (3)
        ole[26..28].copy_from_slice(&VERSION_3.to_le_bytes());

        // Byte order (little-endian)
        ole[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());

        // Sector size power (9 = 512 bytes)
        ole[30..32].copy_from_slice(&9u16.to_le_bytes());

        // Mini sector size power (6 = 64 bytes)
        ole[32..34].copy_from_slice(&6u16.to_le_bytes());

        // Number of FAT sectors
        ole[44..48].copy_from_slice(&1u32.to_le_bytes());

        // First directory sector
        ole[48..52].copy_from_slice(&0u32.to_le_bytes());

        // Mini stream cutoff (4096)
        ole[52..56].copy_from_slice(&4096u32.to_le_bytes());

        // First mini FAT sector (end of chain)
        ole[56..60].copy_from_slice(&0xFFFFFFFEu32.to_le_bytes());

        // First DIFAT sector (end of chain)
        ole[64..68].copy_from_slice(&0xFFFFFFFEu32.to_le_bytes());

        // DIFAT[0] = sector 1 contains FAT
        ole[76..80].copy_from_slice(&1u32.to_le_bytes());

        // Rest of DIFAT = free
        for i in 1..109 {
            let offset = 76 + i * 4;
            ole[offset..offset + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        }

        // Add a FAT sector (sector 1)
        let mut fat_sector = vec![0u8; 512];
        // Fill FAT entries with ENDOFCHAIN marker (0xFFFFFFFE as u32)
        for i in 0..(512 / 4) {
            let offset = i * 4;
            fat_sector[offset..offset + 4].copy_from_slice(&0xFFFFFFFEu32.to_le_bytes());
        }
        ole.extend_from_slice(&fat_sector);

        // Add directory sector (sector 0, but comes after FAT in this layout)
        let mut dir_sector = vec![0u8; 512];
        // Root entry name: "Root Entry" (UTF-16LE)
        let name = "Root Entry";
        for (i, ch) in name.bytes().enumerate() {
            dir_sector[i * 2] = ch;
        }
        // Entry name size (bytes, including null terminator)
        dir_sector[64..66].copy_from_slice(&22u16.to_le_bytes());
        // Entry type (5 = root)
        dir_sector[66] = 5;
        ole.extend_from_slice(&dir_sector);

        ole
    }

    #[test]
    fn parses_ole_header() {
        let ole = create_minimal_ole();
        let (size, sector_size) = parse_ole_header(&ole).unwrap();
        assert_eq!(sector_size, 512);
        assert!(size >= 512);
    }

    #[test]
    fn rejects_invalid_signature() {
        let mut ole = create_minimal_ole();
        ole[0] = 0x00; // Corrupt signature
        assert!(parse_ole_header(&ole).is_err());
    }

    #[test]
    fn carves_minimal_ole() {
        let ole_data = create_minimal_ole();
        let evidence = SliceEvidence {
            data: ole_data.clone(),
        };
        let handler = OleCarveHandler::new("doc".to_string(), 0, 0, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "ole".to_string(),
            pattern_id: "ole_cfb".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file");

        assert_eq!(carved.file_type, "ole");
        assert!(carved.validated);
        assert!(carved.size >= 512);
    }

    #[test]
    fn rejects_non_ole_data() {
        let data = vec![0x00; 1024];
        let evidence = SliceEvidence { data };
        let handler = OleCarveHandler::new("doc".to_string(), 0, 0, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "ole".to_string(),
            pattern_id: "ole_cfb".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none());
    }

    #[test]
    fn rejects_excessive_fat_sectors() {
        let mut ole = create_minimal_ole();

        // Set num_fat_sectors to exceed MAX_FAT_SECTORS (1000)
        ole[44..48].copy_from_slice(&2000u32.to_le_bytes());

        let evidence = SliceEvidence { data: ole };
        let handler = OleCarveHandler::new("doc".to_string(), 0, 0, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "ole".to_string(),
            pattern_id: "ole_cfb".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        // Should reject due to excessive FAT sectors
        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none(), "should reject excessive FAT sectors");
    }

    #[test]
    fn accepts_fat_sectors_at_limit() {
        let mut ole = create_minimal_ole();

        // Set num_fat_sectors to exactly MAX_FAT_SECTORS (1000)
        ole[44..48].copy_from_slice(&1000u32.to_le_bytes());

        // This will still parse, though size estimation will be large
        let result = parse_ole_header(&ole);
        assert!(result.is_ok(), "should accept FAT sectors at limit");
    }
}
