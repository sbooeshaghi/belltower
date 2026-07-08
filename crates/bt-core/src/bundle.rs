use crate::{BranchId, EventId, SessionId, SessionRecord};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

const SHA256_PREFIX: &str = "sha256:";
const SHA256_HEX_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PortableHash(String);

impl PortableHash {
    pub fn new(value: impl Into<String>) -> Result<Self, PortableHashParseError> {
        let value = value.into();
        validate_portable_hash(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn from_sha256_hex(hex: impl Into<String>) -> Self {
        let hex = hex.into();
        debug_assert_eq!(hex.len(), SHA256_HEX_LEN);
        debug_assert!(hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
        Self(format!("{SHA256_PREFIX}{}", hex.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn sha256_hex(&self) -> &str {
        self.0.strip_prefix(SHA256_PREFIX).unwrap_or(&self.0)
    }
}

impl Display for PortableHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for PortableHash {
    type Err = PortableHashParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for PortableHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("portable hash must be sha256:<64 lowercase hex chars>")]
pub struct PortableHashParseError;

fn validate_portable_hash(value: &str) -> Result<(), PortableHashParseError> {
    let Some(hex) = value.strip_prefix(SHA256_PREFIX) else {
        return Err(PortableHashParseError);
    };
    if hex.len() != SHA256_HEX_LEN {
        return Err(PortableHashParseError);
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PortableHashParseError);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionNodeRef {
    pub bundle_hash: Option<PortableHash>,
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub bundle_event_ordinal: u64,
    pub store_seq_id: Option<i64>,
    pub event_id: EventId,
    pub event_hash: PortableHash,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentRef {
    pub hash: PortableHash,
    pub media_type: Option<String>,
    pub size_bytes: u64,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRange {
    pub branch_id: BranchId,
    pub ordinal_start: u64,
    pub ordinal_end: u64,
    pub first_event_hash: PortableHash,
    pub last_event_hash: PortableHash,
    pub exported_store_seq_start: Option<i64>,
    pub exported_store_seq_end: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleArtifactRef {
    pub artifact_id: String,
    pub kind: String,
    pub source_node: SessionNodeRef,
    pub content: ContentRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchManifest {
    pub branch_id: BranchId,
    pub parent_branch_id: Option<BranchId>,
    pub parent_event_id: Option<EventId>,
    pub head: Option<SessionNodeRef>,
    pub is_default: bool,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionPolicy {
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionBundleArtifactMode {
    TraceOnly,
    TracePlusPatches,
    TracePlusArtifacts,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleManifest {
    pub schema_version: u32,
    pub bundle_hash: PortableHash,
    pub session: SessionRecord,
    pub branches: Vec<BranchManifest>,
    pub event_ranges: Vec<EventRange>,
    pub producer: ProducerInfo,
    pub redaction: RedactionPolicy,
    pub artifact_mode: SessionBundleArtifactMode,
    pub contents: Vec<ContentRef>,
    pub artifacts: Vec<BundleArtifactRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_hash_requires_canonical_sha256_form() {
        let valid = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(PortableHash::new(valid).unwrap().as_str(), valid);

        assert!(PortableHash::new("0123456789abcdef").is_err());
        assert!(
            PortableHash::new(
                "sha256:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef"
            )
            .is_err()
        );
        assert!(PortableHash::new("sha256:abc").is_err());
    }
}
