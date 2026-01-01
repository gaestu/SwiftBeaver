use std::fs::File;

use crate::carve::{
    output_path, CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

pub struct SqliteCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl SqliteCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for SqliteCarveHandler {
    fn file_type(&self) -> &str {
        "sqlite"
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
        let mut stream = CarveStream::new(ctx.evidence, hit.global_offset, self.max_size, file);

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let result: Result<u64, CarveError> = (|| {
            let header = stream.read_exact(100)?;
            if &header[..SQLITE_HEADER.len()] != SQLITE_HEADER {
                return Err(CarveError::Invalid("sqlite header mismatch".to_string()));
            }

            let page_size_raw = u16::from_be_bytes([header[16], header[17]]);
            let page_size = if page_size_raw == 1 {
                65536
            } else {
                page_size_raw as u32
            };
            if !is_valid_page_size(page_size) {
                return Err(CarveError::Invalid("sqlite page size invalid".to_string()));
            }

            let page_count = u32::from_be_bytes([header[28], header[29], header[30], header[31]]);
            let mut total_size = if page_count == 0 {
                page_size as u64
            } else {
                page_size as u64 * page_count as u64
            };
            if total_size < 100 {
                total_size = 100;
            }

            let max_size = if self.max_size > 0 {
                self.max_size
            } else {
                total_size
            };
            let target_size = total_size.min(max_size);

            let remaining = target_size.saturating_sub(100);
            if remaining > 0 {
                stream.read_exact(remaining as usize)?;
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
                CarveError::Invalid(_msg) => {
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex) = stream.finish()?;
        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

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

fn is_valid_page_size(page_size: u32) -> bool {
    if page_size < 512 || page_size > 65536 {
        return false;
    }
    page_size.is_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::is_valid_page_size;

    #[test]
    fn sqlite_page_sizes() {
        assert!(is_valid_page_size(512));
        assert!(is_valid_page_size(4096));
        assert!(is_valid_page_size(65536));
        assert!(!is_valid_page_size(1000));
        assert!(!is_valid_page_size(128));
    }
}
