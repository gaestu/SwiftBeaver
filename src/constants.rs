//! # Constants Module
//!
//! Centralized constants used throughout the SwiftBeaver crate.
//! This avoids magic numbers scattered across the codebase.

/// Default I/O buffer size for reading/writing file data (64 KiB)
pub const DEFAULT_IO_BUFFER_SIZE: usize = 64 * 1024;

/// One Mebibyte in bytes
pub const MIB: u64 = 1024 * 1024;

/// One Kibibyte in bytes
pub const KIB: u64 = 1024;

/// Default channel capacity multiplier for workers
pub const CHANNEL_CAPACITY_MULTIPLIER: usize = 4;

/// Minimum channel capacity
pub const MIN_CHANNEL_CAPACITY: usize = 1;

/// Default chunk size in MiB for scanning
pub const DEFAULT_CHUNK_SIZE_MIB: u64 = 64;

/// Default overlap in KiB for chunk scanning
pub const DEFAULT_OVERLAP_KIB: u64 = 64;

/// Minimum interval in seconds between metadata flush events sent from the
/// scan loop.  This prevents flush storms when progress reporting fires
/// frequently (e.g. every chunk with `--progress-interval-secs 0`).
pub const METADATA_FLUSH_INTERVAL_SECS: u64 = 10;

/// Default fraction of carve workers assigned to the fast queue (0.0–1.0).
pub const DEFAULT_FAST_WORKER_RATIO: f64 = 0.25;

/// Hit channel capacity multiplier for the **fast** carve queue.
/// Large because fast carves drain quickly and we want high throughput.
/// Note: each pending NormalizedHit holds an Arc to a chunk (~64 MiB default).
/// In practice, hits from the same chunk share the Arc, so real memory impact
/// is much lower than `slots × chunk_size`.
pub const FAST_HIT_CHANNEL_MULTIPLIER: usize = 16;

/// Hit channel capacity multiplier for the **slow** carve queue.
/// Smaller to bound memory from large pending carves.
pub const SLOW_HIT_CHANNEL_MULTIPLIER: usize = 4;
