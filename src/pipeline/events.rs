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
}
