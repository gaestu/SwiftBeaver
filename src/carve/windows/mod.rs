pub mod evtx;
pub mod lnk;
pub mod prefetch;
pub mod registry;

pub use evtx::EvtxArtefact;
pub use lnk::LnkArtefact;
pub use prefetch::PrefetchArtefact;
pub use registry::RegistryHiveArtefact;

#[derive(Debug, Clone, serde::Serialize)]
pub enum WindowsArtefactRecord {
    Lnk(LnkArtefact),
    Prefetch(PrefetchArtefact),
    Evtx(EvtxArtefact),
    RegistryHive(RegistryHiveArtefact),
}

impl WindowsArtefactRecord {
    pub fn artefact_type(&self) -> &'static str {
        match self {
            Self::Lnk(_) => "lnk",
            Self::Prefetch(_) => "prefetch",
            Self::Evtx(_) => "evtx",
            Self::RegistryHive(_) => "registry",
        }
    }

    pub fn run_id(&self) -> &str {
        match self {
            Self::Lnk(record) => &record.run_id,
            Self::Prefetch(record) => &record.run_id,
            Self::Evtx(record) => &record.run_id,
            Self::RegistryHive(record) => &record.run_id,
        }
    }

    pub fn offset(&self) -> u64 {
        match self {
            Self::Lnk(record) => record.offset,
            Self::Prefetch(record) => record.offset,
            Self::Evtx(record) => record.offset,
            Self::RegistryHive(record) => record.offset,
        }
    }

    pub fn size(&self) -> u64 {
        match self {
            Self::Lnk(record) => record.size,
            Self::Prefetch(record) => record.size,
            Self::Evtx(record) => record.size,
            Self::RegistryHive(record) => record.size,
        }
    }
}
