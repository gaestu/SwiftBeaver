#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanChunk {
    pub id: u64,
    pub start: u64,
    pub length: u64,
    pub valid_length: u64,
}

#[derive(Debug, Clone)]
pub struct ChunkIter {
    total_len: u64,
    chunk_size: u64,
    overlap: u64,
    next_start: u64,
    next_id: u64,
}

impl ChunkIter {
    pub fn new(total_len: u64, chunk_size: u64, overlap: u64) -> Self {
        Self {
            total_len,
            chunk_size,
            overlap,
            next_start: 0,
            next_id: 0,
        }
    }
}

impl Iterator for ChunkIter {
    type Item = ScanChunk;

    fn next(&mut self) -> Option<Self::Item> {
        if self.chunk_size == 0 || self.next_start >= self.total_len {
            return None;
        }

        let remaining = self.total_len - self.next_start;
        let length = remaining.min(self.chunk_size.saturating_add(self.overlap));
        let valid_length = remaining.min(self.chunk_size);
        let chunk = ScanChunk {
            id: self.next_id,
            start: self.next_start,
            length,
            valid_length,
        };

        self.next_start = self.next_start.saturating_add(self.chunk_size);
        self.next_id = self.next_id.saturating_add(1);
        Some(chunk)
    }
}

pub fn chunk_count(total_len: u64, chunk_size: u64) -> u64 {
    if chunk_size == 0 {
        return 0;
    }
    total_len.saturating_add(chunk_size.saturating_sub(1)) / chunk_size
}

pub fn build_chunks(total_len: u64, chunk_size: u64, overlap: u64) -> Vec<ScanChunk> {
    if chunk_size == 0 {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0u64;
    let mut id = 0u64;

    while start < total_len {
        let remaining = total_len - start;
        let length = remaining.min(chunk_size.saturating_add(overlap));
        let valid_length = remaining.min(chunk_size);

        chunks.push(ScanChunk {
            id,
            start,
            length,
            valid_length,
        });

        start = start.saturating_add(chunk_size);
        id += 1;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_chunks_with_overlap() {
        let chunks = build_chunks(100, 40, 10);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks[0].length, 50);
        assert_eq!(chunks[0].valid_length, 40);
        assert_eq!(chunks[1].start, 40);
        assert_eq!(chunks[1].length, 50);
        assert_eq!(chunks[1].valid_length, 40);
        assert_eq!(chunks[2].start, 80);
        assert_eq!(chunks[2].length, 20);
        assert_eq!(chunks[2].valid_length, 20);
    }

    #[test]
    fn chunk_iter_matches_build_chunks() {
        let cases = [
            (0, 64, 0),
            (1, 64, 0),
            (100, 40, 10),
            (100, 40, 0),
            (257, 64, 16),
        ];

        for (total_len, chunk_size, overlap) in cases {
            let expected = build_chunks(total_len, chunk_size, overlap);
            let got: Vec<ScanChunk> = ChunkIter::new(total_len, chunk_size, overlap).collect();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn chunk_count_matches_build_chunks() {
        let cases = [(0, 64), (1, 64), (128, 64), (129, 64), (257, 64)];
        for (total_len, chunk_size) in cases {
            let expected = build_chunks(total_len, chunk_size, 0).len() as u64;
            assert_eq!(chunk_count(total_len, chunk_size), expected);
        }
    }

    #[test]
    fn chunk_iter_empty_when_chunk_size_zero() {
        let mut iter = ChunkIter::new(100, 0, 0);
        assert!(iter.next().is_none());
        assert_eq!(chunk_count(100, 0), 0);
    }
}
