use std::fs::File;
use std::io::{BufWriter, Write};

use sha2::{Digest, Sha256};

use crate::carve::{output_path, CarveError, CarveHandler, CarvedFile, ExtractionContext};
use crate::scanner::NormalizedHit;

pub struct FooterCarveHandler {
    file_type: String,
    extension: String,
    min_size: u64,
    max_size: u64,
    header_patterns: Vec<Vec<u8>>,
    footer_patterns: Vec<Vec<u8>>,
    max_footer_len: usize,
}

impl FooterCarveHandler {
    pub fn new(
        file_type: String,
        extension: String,
        min_size: u64,
        max_size: u64,
        header_patterns: Vec<Vec<u8>>,
        footer_patterns: Vec<Vec<u8>>,
    ) -> Self {
        let max_footer_len = footer_patterns.iter().map(|p| p.len()).max().unwrap_or(0);
        Self {
            file_type,
            extension,
            min_size,
            max_size,
            header_patterns,
            footer_patterns,
            max_footer_len,
        }
    }

    fn header_matches(&self, buf: &[u8]) -> bool {
        if self.header_patterns.is_empty() {
            return true;
        }
        self.header_patterns
            .iter()
            .any(|pat| !pat.is_empty() && buf.len() >= pat.len() && &buf[..pat.len()] == pat)
    }
}

impl CarveHandler for FooterCarveHandler {
    fn file_type(&self) -> &str {
        &self.file_type
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

        let mut carry: Vec<u8> = Vec::new();
        let buf_size = 64 * 1024;

        loop {
            if self.max_size > 0 && bytes_written >= self.max_size {
                truncated = true;
                errors.push("max_size reached before footer".to_string());
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
                errors.push("eof before footer".to_string());
                break;
            }
            buf.truncate(n);

            if bytes_written == 0 && !self.header_matches(&buf) {
                let _ = std::fs::remove_file(&full_path);
                return Ok(None);
            }

            let mut search_buf = Vec::with_capacity(carry.len() + buf.len());
            search_buf.extend_from_slice(&carry);
            search_buf.extend_from_slice(&buf);

            if let Some((pos, pat_len)) = find_first_pattern(&search_buf, &self.footer_patterns) {
                let write_len = if pos < carry.len() {
                    pos + pat_len - carry.len()
                } else {
                    pos - carry.len() + pat_len
                };
                let write_len = write_len.min(buf.len());
                if write_len > 0 {
                    let slice = &buf[..write_len];
                    writer.write_all(slice)?;
                    md5.consume(slice);
                    sha256.update(slice);
                    bytes_written = bytes_written.saturating_add(slice.len() as u64);
                }
                validated = true;
                break;
            }

            writer.write_all(&buf)?;
            md5.consume(&buf);
            sha256.update(&buf);
            bytes_written = bytes_written.saturating_add(buf.len() as u64);
            offset = offset.saturating_add(buf.len() as u64);

            if self.max_footer_len > 1 {
                let keep = self.max_footer_len - 1;
                if buf.len() >= keep {
                    carry = buf[buf.len() - keep..].to_vec();
                } else {
                    carry = buf.clone();
                }
            } else {
                carry.clear();
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

fn find_first_pattern(haystack: &[u8], patterns: &[Vec<u8>]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for pat in patterns {
        if pat.is_empty() || haystack.len() < pat.len() {
            continue;
        }
        if let Some(pos) = find_pattern(haystack, pat) {
            match best {
                None => best = Some((pos, pat.len())),
                Some((best_pos, _)) if pos < best_pos => best = Some((pos, pat.len())),
                _ => {}
            }
        }
    }
    best
}

fn find_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::FooterCarveHandler;
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
    fn carves_until_footer() {
        let header = b"HEAD";
        let footer = b"FOOT";
        let mut data = Vec::new();
        data.extend_from_slice(header);
        data.extend_from_slice(b"payload");
        data.extend_from_slice(footer);

        let evidence = SliceEvidence { data };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "run1",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let handler = FooterCarveHandler::new(
            "custom".to_string(),
            "bin".to_string(),
            1,
            0,
            vec![header.to_vec()],
            vec![footer.to_vec()],
        );

        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "custom".to_string(),
            pattern_id: "header".to_string(),
        };

        let carved = handler
            .process_hit(&hit, &ctx)
            .expect("process")
            .expect("carved");

        assert!(carved.validated);
        assert_eq!(
            carved.size,
            (header.len() + "payload".len() + footer.len()) as u64
        );
    }
}
