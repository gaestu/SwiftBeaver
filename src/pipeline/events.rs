//! # Pipeline Events
//!
//! Events that flow through the pipeline for metadata recording.

use crate::carve::CarvedFile;
use crate::carve::PostCarveMetadata;
use crate::metadata::{EntropyRegion, RunSummary};
use crate::parsers::browser::{BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord};
use crate::strings::artifacts::StringArtefact;
use crate::strings::phones::PhoneSummaryRow;

/// Events sent to the metadata recording thread
#[derive(Debug)]
pub enum MetadataEvent {
    /// A carved file was successfully extracted
    File(CarvedFile),
    /// A parsed Windows artefact record was found
    PostCarveMetadata(PostCarveMetadata),
    /// A string artefact (URL, email, phone) was found
    String(StringArtefact),
    /// Aggregated phone summary row
    PhoneSummary(PhoneSummaryRow),
    /// A browser history record was parsed
    History(BrowserHistoryRecord),
    /// A browser cookie record was parsed
    Cookie(BrowserCookieRecord),
    /// A browser download record was parsed
    Download(BrowserDownloadRecord),
    /// Run summary statistics
    RunSummary(RunSummary),
    /// High entropy region detected
    Entropy(EntropyRegion),
    /// Flush buffered data to disk
    Flush,
}

/// Events for the file metadata shard (carved files, browser data, run summary)
#[derive(Debug)]
pub enum FileShardEvent {
    File(CarvedFile),
    PostCarveMetadata(PostCarveMetadata),
    History(BrowserHistoryRecord),
    Cookie(BrowserCookieRecord),
    Download(BrowserDownloadRecord),
    RunSummary(RunSummary),
    Flush,
}

/// Events for the string artefact metadata shard
#[derive(Debug)]
pub enum StringShardEvent {
    String(StringArtefact),
    PhoneSummary(PhoneSummaryRow),
    Flush,
}

/// Events for the entropy metadata shard
#[derive(Debug)]
pub enum EntropyShardEvent {
    Entropy(EntropyRegion),
    Flush,
}
