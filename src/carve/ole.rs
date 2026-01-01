//! OLE Compound File Binary (CFB) carving handler.
//!
//! OLE/CFB is used by Microsoft Office 97-2003 formats (DOC, XLS, PPT, MSG).
//! The file structure uses a FAT-based sector allocation scheme.
//!
//! Signature: D0 CF 11 E0 A1 B1 1A E1

use std::fs::File;

use crate::carve::{
    output_path, CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext,
};
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

pub struct OleCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl OleCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
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
    // For v4, bytes 40-43 contain the number of FAT sectors
    // But a simpler approach: use the DIFAT to find the highest used sector

    // Read number of FAT sectors
    let num_fat_sectors = u32::from_le_bytes([header[44], header[45], header[46], header[47]]);

    // Read first directory sector
    let first_dir_sector = u32::from_le_bytes([header[48], header[49], header[50], header[51]]);

    // Read number of DIFAT sectors (extended FAT)
    let num_difat_sectors = u32::from_le_bytes([header[68], header[69], header[70], header[71]]);

    // Estimate size: We'll use a conservative approach.
    // The file contains at least:
    // - 1 header sector
    // - FAT sectors
    // - DIFAT sectors
    // - Directory sectors
    // - Data sectors
    //
    // For a basic estimate, we use: (first_dir_sector + some_buffer) * sector_size
    // Better: scan DIFAT entries to find highest sector reference

    // Count DIFAT entries in header (109 entries at offset 76)
    let mut max_sector: u32 = 0;

    // Check FAT sector locations from DIFAT
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
        // Valid sector IDs are < 0xFFFFFFFA (special values are >= that)
        if sector_id < 0xFFFFFFFA && sector_id > max_sector {
            max_sector = sector_id;
        }
    }

    // Also check first directory sector
    if first_dir_sector < 0xFFFFFFFA && first_dir_sector > max_sector {
        max_sector = first_dir_sector;
    }

    // If we found FAT sectors, the file extends beyond them
    // A rough estimate: max_sector * 2 to account for data after FAT
    // Minimum reasonable size
    let estimated_sectors = if max_sector > 0 {
        (max_sector as u64 + 1) * 2
    } else {
        // Fallback: use fat_sectors count
        (num_fat_sectors as u64 + num_difat_sectors as u64 + 10).max(10)
    };

    let estimated_size = header_size + (estimated_sectors * sector_size);

    Ok((estimated_size, sector_size))
}

/// Try to get a more accurate size by reading the FAT
fn refine_ole_size(
    stream: &mut CarveStream,
    header: &[u8],
    sector_size: u64,
    max_size: u64,
) -> Result<u64, CarveError> {
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
        // No FAT sectors found, use header-only estimate
        return Ok(sector_size * 2);
    }

    // Read the first FAT sector to find the highest allocated sector
    let first_fat_sector = fat_sectors[0];
    let fat_offset = sector_size + (first_fat_sector as u64 * sector_size);

    if fat_offset + sector_size > max_size {
        return Ok(fat_offset);
    }

    // We've already written the header (512 bytes), need to read up to FAT
    let already_written = 512u64;
    let to_read = (fat_offset + sector_size).saturating_sub(already_written);

    if to_read > 0 && to_read < 10 * 1024 * 1024 {
        // Read more data to include FAT sector
        let _ = stream.read_exact(to_read as usize);
    }

    // Find highest sector in FAT
    let mut max_used_sector = first_fat_sector;

    // Simple heuristic: highest sector in first FAT
    // (Full implementation would need to follow FAT chains)
    for fat_sector in &fat_sectors {
        if *fat_sector > max_used_sector {
            max_used_sector = *fat_sector;
        }
    }

    // Estimate: sectors up to max_used + buffer for data
    let total_sectors = (max_used_sector as u64 + 1) * 3; // Conservative multiplier
    let estimated = sector_size + (total_sectors * sector_size);

    Ok(estimated.min(max_size))
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
        let (full_path, rel_path) = output_path(
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

        let result: Result<u64, CarveError> = (|| {
            // Read OLE header (512 bytes minimum)
            let header = stream.read_exact(512)?;

            // Parse and validate header
            let (_estimated_size, sector_size) = parse_ole_header(&header)?;

            // Try to refine size estimate by reading FAT
            let target_size = refine_ole_size(&mut stream, &header, sector_size, effective_max)?;

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
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
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
        let handler = OleCarveHandler::new("doc".to_string(), 0, 0);
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
        let handler = OleCarveHandler::new("doc".to_string(), 0, 0);
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
}
