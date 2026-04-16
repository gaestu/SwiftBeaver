//! ELF carving handler.
//!
//! Uses header table offsets to estimate file size.

use std::fs::File;

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const ELF_MAGIC: [u8; 4] = [0x7F, 0x45, 0x4C, 0x46];

pub struct ElfCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl ElfCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for ElfCarveHandler {
    fn file_type(&self) -> &str {
        "elf"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 7];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf[0..4] != ELF_MAGIC {
            return Ok(PreValidation::Reject("elf magic mismatch".to_string()));
        }
        if buf[4] != 1 && buf[4] != 2 {
            return Ok(PreValidation::Reject("elf class invalid".to_string()));
        }
        if buf[5] != 1 && buf[5] != 2 {
            return Ok(PreValidation::Reject("elf endianness invalid".to_string()));
        }
        if buf[6] != 1 {
            return Ok(PreValidation::Reject("elf version invalid".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let header = read_exact_at(ctx, hit.global_offset, 64)
            .ok_or_else(|| CarveError::Invalid("elf header too short".to_string()))?;
        if header[0..4] != ELF_MAGIC {
            return Ok(None);
        }
        let class = header[4];
        let endian = header[5];
        if class != 1 && class != 2 {
            return Ok(None);
        }
        if endian != 1 && endian != 2 {
            return Ok(None);
        }

        let (e_phoff, e_phentsize, e_phnum, e_shoff, e_shentsize, e_shnum) = if class == 1 {
            let phoff = read_u32(&header[28..32], endian) as u64;
            let shoff = read_u32(&header[32..36], endian) as u64;
            let phentsize = read_u16(&header[42..44], endian) as u64;
            let phnum = read_u16(&header[44..46], endian) as u64;
            let shentsize = read_u16(&header[46..48], endian) as u64;
            let shnum = read_u16(&header[48..50], endian) as u64;
            (phoff, phentsize, phnum, shoff, shentsize, shnum)
        } else {
            let phoff = read_u64(&header[32..40], endian);
            let shoff = read_u64(&header[40..48], endian);
            let phentsize = read_u16(&header[54..56], endian) as u64;
            let phnum = read_u16(&header[56..58], endian) as u64;
            let shentsize = read_u16(&header[58..60], endian) as u64;
            let shnum = read_u16(&header[60..62], endian) as u64;
            (phoff, phentsize, phnum, shoff, shentsize, shnum)
        };

        let mut size = 0u64;
        if e_phoff > 0 && e_phentsize > 0 && e_phnum > 0 {
            size = size.max(e_phoff.saturating_add(e_phentsize.saturating_mul(e_phnum)));
        }
        if e_shoff > 0 && e_shentsize > 0 && e_shnum > 0 {
            size = size.max(e_shoff.saturating_add(e_shentsize.saturating_mul(e_shnum)));
        }
        if size == 0 {
            size = header.len() as u64;
        }

        let mut total_end = hit.global_offset.saturating_add(size);
        if self.max_size > 0 {
            let max_end = hit.global_offset.saturating_add(self.max_size);
            if total_end > max_end {
                total_end = max_end;
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

fn read_u16(bytes: &[u8], endian: u8) -> u16 {
    let mut buf = [0u8; 2];
    buf.copy_from_slice(&bytes[0..2]);
    if endian == 1 {
        u16::from_le_bytes(buf)
    } else {
        u16::from_be_bytes(buf)
    }
}

fn read_u32(bytes: &[u8], endian: u8) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&bytes[0..4]);
    if endian == 1 {
        u32::from_le_bytes(buf)
    } else {
        u32::from_be_bytes(buf)
    }
}

fn read_u64(bytes: &[u8], endian: u8) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[0..8]);
    if endian == 1 {
        u64::from_le_bytes(buf)
    } else {
        u64::from_be_bytes(buf)
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
    use super::ElfCarveHandler;
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

    fn minimal_elf64() -> Vec<u8> {
        let mut data = vec![0u8; 0x80];
        data[0..4].copy_from_slice(&[0x7F, 0x45, 0x4C, 0x46]);
        data[4] = 2; // 64-bit
        data[5] = 1; // little endian
        data[6] = 1; // version
        data[0x20..0x28].copy_from_slice(&(0x40u64).to_le_bytes()); // e_phoff
        data[0x28..0x30].copy_from_slice(&(0x40u64).to_le_bytes()); // e_shoff
        data[0x36..0x38].copy_from_slice(&(56u16).to_le_bytes()); // e_phentsize
        data[0x38..0x3A].copy_from_slice(&(1u16).to_le_bytes()); // e_phnum
        data[0x3A..0x3C].copy_from_slice(&(64u16).to_le_bytes()); // e_shentsize
        data[0x3C..0x3E].copy_from_slice(&(1u16).to_le_bytes()); // e_shnum
        data
    }

    #[test]
    fn carves_minimal_elf64() {
        let data = minimal_elf64();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = ElfCarveHandler::new("elf".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "elf".to_string(),
            pattern_id: "elf_magic".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved");
        assert_eq!(carved.size, data.len() as u64);
    }
}
