use std::fs::File;
use std::io::{BufWriter, Write};

use sha2::{Digest, Sha256};

use crate::carve::{output_path, CarveError, CarveHandler, CarvedFile, ExtractionContext};
use crate::scanner::NormalizedHit;

pub struct JpegCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl JpegCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for JpegCarveHandler {
    fn file_type(&self) -> &str {
        "jpeg"
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
        let mut writer = BufWriter::new(file);
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut offset = hit.global_offset;
        let mut bytes_written = 0u64;
        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let mut prev: Option<u8> = None;
        let buf_size = 64 * 1024;

        loop {
            if self.max_size > 0 && bytes_written >= self.max_size {
                truncated = true;
                errors.push("max_size reached before EOI".to_string());
                break;
            }

            let remaining = if self.max_size > 0 {
                (self.max_size - bytes_written).min(buf_size as u64)
            } else {
                buf_size as u64
            };

            let mut buf = vec![0u8; remaining as usize];
            let n = ctx
                .evidence
                .read_at(offset, &mut buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if n == 0 {
                truncated = true;
                errors.push("eof before EOI".to_string());
                break;
            }
            buf.truncate(n);

            let mut write_len = n;
            for (i, b) in buf.iter().enumerate() {
                if prev == Some(0xFF) && *b == 0xD9 {
                    write_len = i + 1;
                    validated = true;
                    break;
                }
                prev = Some(*b);
            }

            let slice = &buf[..write_len];
            writer.write_all(slice)?;
            md5.consume(slice);
            sha256.update(slice);

            bytes_written = bytes_written.saturating_add(write_len as u64);
            offset = offset.saturating_add(write_len as u64);

            if validated {
                break;
            }

            if write_len < n {
                break;
            }
        }

        writer.flush()?;

        if bytes_written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());
        let global_end = if bytes_written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + bytes_written - 1
        };

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size: bytes_written,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}
