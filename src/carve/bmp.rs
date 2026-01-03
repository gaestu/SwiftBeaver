use std::fs::File;
use std::io::Write;

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, output_path, write_range,
};
use crate::scanner::NormalizedHit;

/// BMP file header is 14 bytes
const BMP_HEADER_LEN: usize = 14;
/// Minimum header to read: BMP header (14) + DIB header size field (4)
const BMP_MIN_HEADER: usize = 18;
const BMP_MAGIC: [u8; 2] = [0x42, 0x4D];

/// Valid DIB header sizes (most common formats)
/// - BITMAPCOREHEADER: 12
/// - BITMAPINFOHEADER: 40
/// - BITMAPV2INFOHEADER: 52
/// - BITMAPV3INFOHEADER: 56
/// - BITMAPV4HEADER: 108
/// - BITMAPV5HEADER: 124
const VALID_DIB_SIZES: [u32; 6] = [12, 40, 52, 56, 108, 124];

/// Maximum reasonable image dimension (32768 pixels)
const MAX_DIMENSION: u32 = 32768;

pub struct BmpCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl BmpCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for BmpCarveHandler {
    fn file_type(&self) -> &str {
        "bmp"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        // Read BMP header + start of DIB header for validation
        let mut header = [0u8; 58]; // Enough for BMP header + BITMAPINFOHEADER
        let n = ctx
            .evidence
            .read_at(hit.global_offset, &mut header)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < BMP_MIN_HEADER {
            return Ok(None);
        }
        if header[0..2] != BMP_MAGIC {
            return Ok(None);
        }

        let file_size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as u64;
        let pixel_offset =
            u32::from_le_bytes([header[10], header[11], header[12], header[13]]) as u64;

        // Basic BMP header validation
        if file_size < BMP_HEADER_LEN as u64
            || pixel_offset < BMP_HEADER_LEN as u64
            || pixel_offset > file_size
        {
            return Ok(None);
        }

        // Validate DIB header size - this is critical for reducing false positives
        let dib_header_size = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
        if !VALID_DIB_SIZES.contains(&dib_header_size) {
            return Ok(None);
        }

        // pixel_offset must be at least BMP header + DIB header
        if pixel_offset < (BMP_HEADER_LEN as u64 + dib_header_size as u64) {
            return Ok(None);
        }

        // For BITMAPINFOHEADER (40 bytes) and larger, validate dimensions
        if dib_header_size >= 40 && n >= 26 {
            let width = i32::from_le_bytes([header[18], header[19], header[20], header[21]]);
            let height = i32::from_le_bytes([header[22], header[23], header[24], header[25]]);

            // Width must be positive, height can be negative (top-down DIB)
            let abs_width = width.unsigned_abs();
            let abs_height = height.unsigned_abs();

            // Reject unreasonable dimensions
            if width <= 0 || abs_width > MAX_DIMENSION || abs_height > MAX_DIMENSION {
                return Ok(None);
            }

            // If we have enough data, validate bits per pixel
            if n >= 30 {
                let bits_per_pixel = u16::from_le_bytes([header[28], header[29]]);
                // Valid bits per pixel: 1, 4, 8, 16, 24, 32
                if !matches!(bits_per_pixel, 1 | 4 | 8 | 16 | 24 | 32) {
                    return Ok(None);
                }

                // Sanity check: file size should be reasonable for dimensions
                // Row size is padded to 4 bytes
                let row_size = ((abs_width * bits_per_pixel as u32 + 31) / 32) * 4;
                let pixel_data_size = row_size as u64 * abs_height as u64;
                let min_expected_size = pixel_offset + pixel_data_size;

                // Allow some tolerance for palette and other data, but reject wildly wrong sizes
                if file_size < min_expected_size.saturating_sub(1024) {
                    return Ok(None);
                }
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

        let mut total_end = hit.global_offset + file_size;
        let mut truncated = false;
        let mut errors = Vec::new();

        if self.max_size > 0 && file_size > self.max_size {
            total_end = hit.global_offset + self.max_size;
            truncated = true;
            errors.push("max_size reached before BMP end".to_string());
        }

        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &mut file,
            &mut md5,
            &mut sha256,
        )?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before BMP end".to_string());
        }
        file.flush()?;

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
            validated: !truncated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::BmpCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;

    /// Creates a valid minimal BMP file (1x1 pixel, 24-bit)
    fn create_valid_bmp() -> Vec<u8> {
        let mut bmp = Vec::new();
        // BMP header (14 bytes)
        let pixel_offset = 54u32; // BMP header (14) + DIB header (40)
        let file_size = pixel_offset + 4; // 1x1 pixel with row padding to 4 bytes
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&file_size.to_le_bytes()); // File size
        bmp.extend_from_slice(&0u16.to_le_bytes()); // Reserved
        bmp.extend_from_slice(&0u16.to_le_bytes()); // Reserved
        bmp.extend_from_slice(&pixel_offset.to_le_bytes()); // Pixel data offset

        // DIB header (BITMAPINFOHEADER - 40 bytes)
        bmp.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
        bmp.extend_from_slice(&1i32.to_le_bytes()); // Width: 1 pixel
        bmp.extend_from_slice(&1i32.to_le_bytes()); // Height: 1 pixel
        bmp.extend_from_slice(&1u16.to_le_bytes()); // Color planes (must be 1)
        bmp.extend_from_slice(&24u16.to_le_bytes()); // Bits per pixel: 24
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Compression: none
        bmp.extend_from_slice(&4u32.to_le_bytes()); // Image size (can be 0 for uncompressed)
        bmp.extend_from_slice(&2835i32.to_le_bytes()); // Horizontal resolution
        bmp.extend_from_slice(&2835i32.to_le_bytes()); // Vertical resolution
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Colors in palette
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Important colors

        // Pixel data (1 pixel = 3 bytes + 1 byte padding = 4 bytes)
        bmp.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]); // Blue pixel + padding

        bmp
    }

    #[test]
    fn carves_minimal_bmp() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let bmp = create_valid_bmp();
        let file_size = bmp.len() as u64;

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &bmp).expect("write bmp");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = BmpCarveHandler::new("bmp".to_string(), 10, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bmp".to_string(),
            pattern_id: "bmp_header".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, file_size);
    }

    #[test]
    fn rejects_invalid_dib_header_size() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&100u32.to_le_bytes()); // File size
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes()); // Pixel offset
        bmp.extend_from_slice(&99u32.to_le_bytes()); // Invalid DIB header size
        bmp.extend_from_slice(&[0u8; 82]); // Padding

        let input_path = temp_dir.path().join("invalid.bin");
        std::fs::write(&input_path, &bmp).expect("write");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = BmpCarveHandler::new("bmp".to_string(), 10, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bmp".to_string(),
            pattern_id: "bmp_header".to_string(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(result.is_none(), "Should reject invalid DIB header size");
    }

    #[test]
    fn rejects_invalid_bits_per_pixel() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&100u32.to_le_bytes()); // File size
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes()); // Pixel offset
        bmp.extend_from_slice(&40u32.to_le_bytes()); // Valid DIB header size
        bmp.extend_from_slice(&100i32.to_le_bytes()); // Width
        bmp.extend_from_slice(&100i32.to_le_bytes()); // Height
        bmp.extend_from_slice(&1u16.to_le_bytes()); // Planes
        bmp.extend_from_slice(&13u16.to_le_bytes()); // Invalid bits per pixel
        bmp.extend_from_slice(&[0u8; 64]); // Rest of header + padding

        let input_path = temp_dir.path().join("invalid_bpp.bin");
        std::fs::write(&input_path, &bmp).expect("write");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = BmpCarveHandler::new("bmp".to_string(), 10, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bmp".to_string(),
            pattern_id: "bmp_header".to_string(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(result.is_none(), "Should reject invalid bits per pixel");
    }
}
