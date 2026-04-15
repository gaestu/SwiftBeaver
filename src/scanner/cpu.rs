use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use anyhow::{Result, anyhow};

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::scanner::{Hit, SignatureScanner};

#[derive(Debug, Clone)]
struct PatternMeta {
    id: String,
    file_type_id: String,
}

pub struct CpuScanner {
    automaton: AhoCorasick,
    pattern_meta: Vec<PatternMeta>,
}

impl CpuScanner {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut raw_patterns: Vec<Vec<u8>> = Vec::new();
        let mut pattern_meta: Vec<PatternMeta> = Vec::new();
        for file_type in &cfg.file_types {
            for pat in &file_type.header_patterns {
                let bytes = hex::decode(pat.hex.trim())
                    .map_err(|e| anyhow!("invalid hex pattern {}: {e}", pat.id))?;
                if bytes.is_empty() {
                    continue;
                }
                raw_patterns.push(bytes);
                pattern_meta.push(PatternMeta {
                    id: pat.id.clone(),
                    file_type_id: file_type.id.clone(),
                });
            }
        }
        let automaton = AhoCorasickBuilder::new()
            .match_kind(MatchKind::Standard)
            .build(&raw_patterns)
            .map_err(|e| anyhow!("failed to build Aho-Corasick automaton: {e}"))?;
        Ok(Self {
            automaton,
            pattern_meta,
        })
    }
}

impl SignatureScanner for CpuScanner {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<Hit> {
        let mut hits = Vec::new();
        for mat in self.automaton.find_overlapping_iter(data) {
            let meta = &self.pattern_meta[mat.pattern().as_usize()];
            hits.push(Hit {
                chunk_id: chunk.id,
                local_offset: mat.start() as u64,
                pattern_id: meta.id.clone(),
                file_type_id: meta.file_type_id.clone(),
            });
        }
        hits
    }
}
