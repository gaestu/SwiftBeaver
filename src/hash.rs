/// Hash algorithm selection for carved file integrity verification.
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgorithm {
    Md5,
    Sha256,
}

#[derive(Debug, Clone)]
pub struct HashConfig {
    pub algorithms: Vec<HashAlgorithm>,
}

impl HashConfig {
    pub fn has_md5(&self) -> bool {
        self.algorithms.contains(&HashAlgorithm::Md5)
    }

    pub fn has_sha256(&self) -> bool {
        self.algorithms.contains(&HashAlgorithm::Sha256)
    }

    /// Parse a list of algorithm name strings into a HashConfig.
    /// Unrecognised names produce a warning and are ignored.
    pub fn from_names(names: &[String]) -> Self {
        let algorithms = names
            .iter()
            .filter_map(|n| match n.to_ascii_lowercase().as_str() {
                "md5" => Some(HashAlgorithm::Md5),
                "sha256" => Some(HashAlgorithm::Sha256),
                _ => {
                    warn!("unknown hash algorithm '{}', ignoring", n);
                    None
                }
            })
            .collect();
        Self { algorithms }
    }
}

impl Default for HashConfig {
    fn default() -> Self {
        Self {
            algorithms: vec![HashAlgorithm::Md5, HashAlgorithm::Sha256],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_both() {
        let cfg = HashConfig::default();
        assert!(cfg.has_md5());
        assert!(cfg.has_sha256());
    }

    #[test]
    fn from_names_md5_only() {
        let cfg = HashConfig::from_names(&["md5".to_string()]);
        assert!(cfg.has_md5());
        assert!(!cfg.has_sha256());
    }

    #[test]
    fn from_names_sha256_only() {
        let cfg = HashConfig::from_names(&["sha256".to_string()]);
        assert!(!cfg.has_md5());
        assert!(cfg.has_sha256());
    }

    #[test]
    fn from_names_ignores_unknown() {
        let cfg = HashConfig::from_names(&["md5".to_string(), "sha1".to_string()]);
        assert!(cfg.has_md5());
        assert!(!cfg.has_sha256());
    }
}
