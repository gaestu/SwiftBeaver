//! # Pipeline Events
//!
//! Events that flow through the pipeline for metadata recording.

use crate::carve::CarvedFile;
use crate::metadata::{EntropyRegion, RunSummary};
use crate::parsers::browser::{BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord};
use crate::strings::artifacts::StringArtefact;

/// Events sent to the metadata recording thread
#[derive(Debug)]
pub enum MetadataEvent {
    /// A carved file was successfully extracted
    File(CarvedFile),
    /// A string artefact (URL, email, phone) was found
    String(StringArtefact),
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
    Flush,
}

/// Events for the entropy metadata shard
#[derive(Debug)]
pub enum EntropyShardEvent {
    Entropy(EntropyRegion),
    Flush,
}
