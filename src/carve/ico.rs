//! ICO/CUR carving handler.
//!
//! ICO files have a small header with directory entries containing offsets/sizes.
//! Enhanced validation verifies that at least one entry contains valid BMP or PNG data.

use std::fs::File;

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// BMP signature at start of image data within ICO
const BMP_HEADER_MAGIC: [u8; 2] = [0x28, 0x00]; // BITMAPINFOHEADER size (40) in LE
/// PNG signature at start of image data within ICO
const PNG_HEADER_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Maximum reasonable icon entries (Windows typically uses 1-10)
const MAX_ICON_ENTRIES: usize = 64;
/// Maximum reasonable single icon image size (256x256 @ 32bpp + overhead)
const MAX_SINGLE_IMAGE_SIZE: u64 = 512 * 1024; // 512 KB per image
/// Maximum reasonable total ICO size
const MAX_REASONABLE_ICO_SIZE: u64 = 4 * 1024 * 1024; // 4 MB total

pub struct IcoCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl IcoCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }

    /// Validate that data at the given offset looks like valid BMP or PNG image data
    fn validate_image_data(ctx: &ExtractionContext, offset: u64, size: u64) -> bool {
        if size < 8 {
            return false;
        }
        let header = match read_exact_at(ctx, offset, 8) {
            Some(h) => h,
            None => return false,
        };

        // Check for PNG signature (embedded PNG in ICO)
        if header.starts_with(&PNG_HEADER_MAGIC) {
            return true;
        }

        // Check for BMP DIB header (BITMAPINFOHEADER starts with size=40 as u32 LE)
        // ICO embeds BMP without the BM file header, so we look for BITMAPINFOHEADER
        if header[0..2] == BMP_HEADER_MAGIC {
            // Additional validation: check biWidth and biHeight are reasonable
            if header.len() >= 8 {
                let width = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);
                // Width should be positive and <= 256 for ICO
                if width > 0 && width <= 256 {
                    return true;
                }
            }
        }

        false
    }
}

impl CarveHandler for IcoCarveHandler {
    fn file_type(&self) -> &str {
        "ico"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 6];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf[0] != 0x00 || buf[1] != 0x00 {
            return Ok(PreValidation::Reject(
                "ico reserved bytes invalid".to_string(),
            ));
        }
        let icon_type = u16::from_le_bytes([buf[2], buf[3]]);
        if icon_type != 1 && icon_type != 2 {
            return Ok(PreValidation::Reject("ico type invalid".to_string()));
        }
        let count = u16::from_le_bytes([buf[4], buf[5]]);
        if count == 0 || count > 255 {
            return Ok(PreValidation::Reject(
                "ico image count implausible".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let header = read_exact_at(ctx, hit.global_offset, 6)
            .ok_or_else(|| CarveError::Invalid("ico header too short".to_string()))?;
        if header[0] != 0 || header[1] != 0 {
            return Ok(None);
        }
        let icon_type = u16::from_le_bytes([header[2], header[3]]);
        if icon_type != 1 && icon_type != 2 {
            return Ok(None);
        }
        let count = u16::from_le_bytes([header[4], header[5]]) as usize;
        // Stricter limit: most ICO files have 1-10 entries, max 64 for sanity
        if count == 0 || count > MAX_ICON_ENTRIES {
            return Ok(None);
        }

        let dir_len = count * 16;
        let dir = read_exact_at(ctx, hit.global_offset + 6, dir_len)
            .ok_or_else(|| CarveError::Invalid("ico directory truncated".to_string()))?;
        let mut max_end = 0u64;
        let header_size = 6u64 + dir_len as u64;
        let mut valid_image_found = false;

        for i in 0..count {
            let base = i * 16;
            let size =
                u32::from_le_bytes([dir[base + 8], dir[base + 9], dir[base + 10], dir[base + 11]])
                    as u64;
            let offset = u32::from_le_bytes([
                dir[base + 12],
                dir[base + 13],
                dir[base + 14],
                dir[base + 15],
            ]) as u64;

            // Basic sanity checks
            if size == 0 || offset < header_size {
                return Ok(None);
            }

            // Stricter size check per image
            if size > MAX_SINGLE_IMAGE_SIZE {
                return Ok(None);
            }

            // Validate actual image data at declared offset
            let image_global_offset = hit.global_offset.saturating_add(offset);
            if Self::validate_image_data(ctx, image_global_offset, size) {
                valid_image_found = true;
            }

            max_end = max_end.max(offset.saturating_add(size));
        }

        // Reject if no valid image signatures found at any declared offset
        if !valid_image_found {
            return Ok(None);
        }

        // Apply reasonable total size cap
        let reasonable_max = MAX_REASONABLE_ICO_SIZE;
        let mut total_end = hit
            .global_offset
            .saturating_add(max_end.min(reasonable_max));
        if self.max_size > 0 {
            let max_allowed = hit.global_offset.saturating_add(self.max_size);
            if total_end > max_allowed {
                total_end = max_allowed;
            }
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut file = File::create(&full_path)?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &mut file,
            &mut md5,
            &mut sha256,
        )?;

        if written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());
        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size: written,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated: !eof_truncated,
            truncated: eof_truncated,
            errors: Vec::new(),
            pattern_id: Some(hit.pattern_id.clone()),
        }))
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

#[cfg(test)]
mod tests {
    use super::IcoCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::{EvidenceError, EvidenceSource};
    use crate::scanner::NormalizedHit;
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

    #[test]
    fn carves_minimal_ico() {
        let mut data = Vec::new();
        // ICO header: reserved(2), type(2), count(2)
        data.extend_from_slice(&[0x00, 0x00, 0x01, 0x00]); // reserved=0, type=1 (ICO)
        data.extend_from_slice(&[0x01, 0x00]); // count=1
        // ICONDIRENTRY: width(1), height(1), colorCount(1), reserved(1), planes(2), bitCount(2), bytesInRes(4), imageOffset(4)
        data.extend_from_slice(&[16, 16, 0, 0]); // 16x16, 0 colors, reserved
        data.extend_from_slice(&[1, 0]); // planes=1
        data.extend_from_slice(&[32, 0]); // bitCount=32
        let bmp_size: u32 = 40 + 16 * 16 * 4; // DIB header + 16x16 RGBA pixels
        data.extend_from_slice(&bmp_size.to_le_bytes()); // bytesInRes
        data.extend_from_slice(&(22u32).to_le_bytes()); // imageOffset = 6 + 16 = 22

        // Add a valid BMP DIB header (BITMAPINFOHEADER) at offset 22
        // Size (40), Width (16), Height (32 for XOR+AND), Planes (1), BitCount (32), ...
        data.extend_from_slice(&[40, 0, 0, 0]); // biSize = 40
        data.extend_from_slice(&[16, 0, 0, 0]); // biWidth = 16
        data.extend_from_slice(&[32, 0, 0, 0]); // biHeight = 32 (16*2 for XOR+AND)
        data.extend_from_slice(&[1, 0]); // biPlanes = 1
        data.extend_from_slice(&[32, 0]); // biBitCount = 32
        data.extend_from_slice(&[0; 24]); // rest of BITMAPINFOHEADER

        // Add some dummy pixel data
        data.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = IcoCarveHandler::new("ico".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "ico".to_string(),
            pattern_id: "ico_header".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved");
        assert!(carved.size > 0);
    }
}
