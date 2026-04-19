//! MOBI/AZW (PDB) carving handler.
//!
//! Uses PDB record offsets to estimate file size; best-effort heuristic.

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, create_hashers,
    finalize_hashers, output_path, write_range,
};
use crate::scanner::NormalizedHit;

const PDB_HEADER_LEN: usize = 78;
const MOBI_OFFSET: u64 = 60;
const MOBI_MAGIC: &[u8; 8] = b"BOOKMOBI";

pub struct MobiCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl MobiCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for MobiCarveHandler {
    fn file_type(&self) -> &str {
        "mobi"
    }

    fn is_fast(&self) -> bool {
        true
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let start_offset = if hit.pattern_id == "mobi_pdb" {
            if hit.global_offset < MOBI_OFFSET {
                return Ok(None);
            }
            hit.global_offset - MOBI_OFFSET
        } else {
            hit.global_offset
        };

        let header = read_exact_at(ctx, start_offset, PDB_HEADER_LEN)
            .ok_or_else(|| CarveError::Invalid("pdb header too short".to_string()))?;
        if &header[60..68] != MOBI_MAGIC {
            return Ok(None);
        }

        let record_count = u16::from_be_bytes([header[76], header[77]]) as usize;
        if record_count == 0 || record_count > 4096 {
            return Ok(None);
        }

        let record_list_len = record_count * 8;
        let record_list = read_exact_at(ctx, start_offset + PDB_HEADER_LEN as u64, record_list_len)
            .ok_or_else(|| CarveError::Invalid("pdb record list truncated".to_string()))?;

        let mut offsets = Vec::with_capacity(record_count);
        for i in 0..record_count {
            let base = i * 8;
            let offset = u32::from_be_bytes([
                record_list[base],
                record_list[base + 1],
                record_list[base + 2],
                record_list[base + 3],
            ]) as u64;
            offsets.push(offset);
        }
        offsets.sort();
        if offsets[0] < PDB_HEADER_LEN as u64 + record_list_len as u64 {
            return Ok(None);
        }

        let last_offset = *offsets.last().unwrap_or(&0);
        let est_last_size = if offsets.len() >= 2 {
            let prev = offsets[offsets.len() - 2];
            last_offset.saturating_sub(prev).max(1)
        } else {
            4096u64
        };
        let mut total_size = last_offset.saturating_add(est_last_size);
        if self.max_size > 0 {
            total_size = total_size.min(self.max_size);
        }

        let mut total_end = start_offset.saturating_add(total_size);

        if self.max_size > 0 {
            let max_end = start_offset.saturating_add(self.max_size);
            if total_end > max_end {
                total_end = max_end;
            }
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            start_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            start_offset,
            total_end,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;

        if written < self.min_size {
            writer.discard();
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
        let global_end = if written == 0 {
            start_offset
        } else {
            start_offset + written - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: start_offset,
                global_end,
                size: written,
                md5: md5_hex,
                sha256: sha256_hex,
                validated: !eof_truncated,
                truncated: eof_truncated,
                errors: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::MobiCarveHandler;
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

    fn minimal_mobi() -> Vec<u8> {
        let mut data = vec![0u8; 140];
        data[60..64].copy_from_slice(b"BOOK");
        data[64..68].copy_from_slice(b"MOBI");
        data[76..78].copy_from_slice(&(2u16).to_be_bytes());
        data[78..82].copy_from_slice(&(100u32).to_be_bytes());
        data[86..90].copy_from_slice(&(120u32).to_be_bytes());
        data
    }

    #[test]
    fn carves_minimal_mobi() {
        let data = minimal_mobi();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = MobiCarveHandler::new("mobi".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 60,
            file_type_id: "mobi".to_string(),
            pattern_id: "mobi_pdb".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved").flush().expect("flush");
        assert_eq!(carved.size, data.len() as u64);
    }
}
