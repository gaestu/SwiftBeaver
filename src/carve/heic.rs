//! HEIC/HEIF image carving handler.
//!
//! HEIC (High Efficiency Image Container) and HEIF (High Efficiency Image Format) files
//! use the ISO Base Media File Format (ISOBMFF), the same box-based structure as MP4/MOV.
//! They are the default photo formats on modern iOS devices (iPhone 7+, iOS 11+).
//!
//! Key brands for detection:
//! - `heic` / `heix` - HEIC image / extended
//! - `heim` / `heis` - HEIC image sequence
//! - `mif1` - HEIF image
//! - `msf1` - HEIF image sequence
//! - `hevc` / `hevx` - HEVC video (can contain images)

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const BOX_HEADER_LEN: usize = 8;
const EXTENDED_HEADER_LEN: usize = 16;

/// Valid HEIC/HEIF major brands
const HEIC_BRANDS: &[&[u8; 4]] = &[
    b"heic", // HEIC image
    b"heix", // HEIC image extended
    b"heim", // HEIC image sequence
    b"heis", // HEIC image sequence
    b"mif1", // HEIF image (MIAF)
    b"msf1", // HEIF image sequence
    b"hevc", // HEVC video (can contain images)
    b"hevx", // HEVC extended
];

pub struct HeicCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl HeicCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for HeicCarveHandler {
    fn file_type(&self) -> &str {
        "heic"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 12];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if &buf[4..8] != b"ftyp" {
            return Ok(PreValidation::Reject(
                "heic ftyp marker mismatch".to_string(),
            ));
        }
        let brand = &buf[8..12];
        let valid = HEIC_BRANDS.iter().any(|b| brand == &b[..]);
        if !valid {
            return Ok(PreValidation::Reject("heic brand mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let mut errors = Vec::new();
        let mut truncated = false;
        let mut seen_ftyp = false;
        let mut seen_meta = false;

        let mut offset = hit.global_offset;
        let mut last_good = hit.global_offset;

        loop {
            if self.max_size > 0 && offset - hit.global_offset >= self.max_size {
                truncated = true;
                errors.push("max_size reached before HEIC end".to_string());
                break;
            }

            let header = match read_exact_at(ctx, offset, BOX_HEADER_LEN) {
                Some(buf) => buf,
                None => {
                    let evidence_len = ctx.evidence.len();
                    // If we've seen required boxes and hit EOF naturally, that's okay
                    if seen_ftyp && offset.saturating_add(BOX_HEADER_LEN as u64) > evidence_len {
                        break;
                    }
                    truncated = true;
                    errors.push("eof before HEIC end".to_string());
                    break;
                }
            };

            let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
            let box_type = &header[4..8];

            let (box_size, header_len) = if size32 == 1 {
                // Extended size in next 8 bytes
                let ext = match read_exact_at(ctx, offset, EXTENDED_HEADER_LEN) {
                    Some(buf) => buf,
                    None => {
                        if seen_ftyp {
                            break;
                        }
                        truncated = true;
                        errors.push("eof before HEIC extended size".to_string());
                        break;
                    }
                };
                let size64 = u64::from_be_bytes([
                    ext[8], ext[9], ext[10], ext[11], ext[12], ext[13], ext[14], ext[15],
                ]);
                (size64, EXTENDED_HEADER_LEN as u64)
            } else if size32 == 0 {
                // Box extends to EOF - we need to stop here
                if seen_ftyp {
                    break;
                }
                truncated = true;
                errors.push("heic box size 0 encountered".to_string());
                break;
            } else {
                (size32, BOX_HEADER_LEN as u64)
            };

            if box_size < header_len || box_size == 0 {
                if seen_ftyp {
                    break;
                }
                return Ok(None);
            }

            // Validate first box is ftyp with a HEIC/HEIF brand
            if offset == hit.global_offset {
                if box_type != b"ftyp" {
                    return Ok(None);
                }
                let brand = match read_exact_at(ctx, offset.saturating_add(header_len), 4) {
                    Some(bytes) => bytes,
                    None => return Ok(None),
                };
                if !is_heic_brand(&brand) {
                    return Ok(None);
                }
                seen_ftyp = true;
            }

            // Track meta box presence (contains item info for images)
            if box_type == b"meta" {
                seen_meta = true;
            }

            if self.max_size > 0
                && (offset - hit.global_offset).saturating_add(box_size) > self.max_size
            {
                truncated = true;
                errors.push("max_size reached before HEIC end".to_string());
                break;
            }

            offset = offset.saturating_add(box_size);
            last_good = offset;
        }

        // HEIC files must have ftyp box (meta is optional for some variants)
        if !seen_ftyp {
            return Ok(None);
        }

        // Bonus validation: most HEIC files should have meta box
        // but we don't reject if missing since some minimal files may not have it
        if !seen_meta {
            errors.push("meta box not found (image data may be incomplete)".to_string());
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let mut total_end = last_good;
        if self.max_size > 0 && total_end - hit.global_offset > self.max_size {
            total_end = hit.global_offset + self.max_size;
        }

        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before HEIC end".to_string());
        }

        if written < self.min_size {
            writer.discard();
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: hit.global_offset,
                global_end,
                size: written,
                md5: md5_hex,
                sha256: sha256_hex,
                validated: !truncated,
                truncated,
                errors,
                pattern_id: Some(hit.pattern_id.clone()),
                is_duplicate: false,
                duplicate_of_offset: None,
            },
            writer,
        )))
    }
}

fn read_exact_at(ctx: &ExtractionContext, offset: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).ok()?;
    if n < len {
        return None;
    }
    Some(buf)
}

fn is_heic_brand(brand: &[u8]) -> bool {
    if brand.len() < 4 {
        return false;
    }
    let brand_arr: &[u8; 4] = brand[..4].try_into().unwrap_or(&[0; 4]);
    HEIC_BRANDS.contains(&brand_arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::RawFileSource;

    /// Build a minimal valid HEIC file with ftyp and meta boxes
    fn build_minimal_heic(brand: &[u8; 4]) -> Vec<u8> {
        let mut data = Vec::new();

        // ftyp box (20 bytes): size(4) + type(4) + brand(4) + minor_version(4) + compatible_brand(4)
        data.extend_from_slice(&20u32.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(brand);
        data.extend_from_slice(&0u32.to_be_bytes()); // minor version
        data.extend_from_slice(brand); // compatible brand

        // meta box (12 bytes minimal): size(4) + type(4) + version+flags(4)
        data.extend_from_slice(&12u32.to_be_bytes());
        data.extend_from_slice(b"meta");
        data.extend_from_slice(&0u32.to_be_bytes()); // version + flags (full box)

        // mdat box (8 bytes empty): size(4) + type(4)
        data.extend_from_slice(&8u32.to_be_bytes());
        data.extend_from_slice(b"mdat");

        data
    }

    #[test]
    fn carves_minimal_heic() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let heic = build_minimal_heic(b"heic");
        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &heic).expect("write heic");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = HeicCarveHandler::new("heic".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "heic".to_string(),
            pattern_id: "heic_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, heic.len() as u64);
    }

    #[test]
    fn carves_mif1_brand() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let heif = build_minimal_heic(b"mif1");
        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &heif).expect("write heif");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = HeicCarveHandler::new("heic".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "heic".to_string(),
            pattern_id: "mif1_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, heif.len() as u64);
    }

    #[test]
    fn rejects_non_heic_isobmff() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        // Build an MP4-like file with 'isom' brand (not HEIC)
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&24u32.to_be_bytes());
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(&8u32.to_be_bytes());
        mp4.extend_from_slice(b"moov");

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &mp4).expect("write mp4");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = HeicCarveHandler::new("heic".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "heic".to_string(),
            pattern_id: "heic_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(carved.is_none(), "Should reject non-HEIC ISOBMFF files");
    }

    #[test]
    fn enforces_max_size() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let heic = build_minimal_heic(b"heic");
        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &heic).expect("write heic");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        // Set max_size smaller than the file
        let handler = HeicCarveHandler::new("heic".to_string(), 8, 30);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "heic".to_string(),
            pattern_id: "heic_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.truncated);
        assert!(carved.size <= 30);
    }

    #[test]
    fn handles_truncated_file() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        // Build incomplete HEIC (only ftyp box, no meta or mdat)
        let mut data = Vec::new();
        data.extend_from_slice(&20u32.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"heic");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"heic");
        // File ends here - ftyp complete but no more boxes

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &data).expect("write truncated heic");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = HeicCarveHandler::new("heic".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "heic".to_string(),
            pattern_id: "heic_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        // Should still carve what we have (ftyp is valid)
        let carved = carved.expect("carved").flush().expect("flush");
        // File should have warning about missing meta box
        assert!(carved.errors.iter().any(|e| e.contains("meta")));
    }
}
