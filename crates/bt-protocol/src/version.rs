use crate::{ProtocolError, Result};

pub const PROTOCOL_VERSION: &str = "belltower.v1";
pub const PROTOCOL_HEADER: &str = "x-belltower-protocol-version";

pub fn negotiate_protocol_version(requested: Option<&str>) -> Result<&'static str> {
    match requested {
        None => Ok(PROTOCOL_VERSION),
        Some(version) if version == PROTOCOL_VERSION => Ok(PROTOCOL_VERSION),
        Some(version) => Err(ProtocolError::UnsupportedVersion {
            requested: version.to_owned(),
            supported: PROTOCOL_VERSION,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{PROTOCOL_VERSION, negotiate_protocol_version};

    #[test]
    fn accepts_current_or_missing_version() {
        assert_eq!(
            negotiate_protocol_version(None).expect("missing version"),
            PROTOCOL_VERSION
        );
        assert_eq!(
            negotiate_protocol_version(Some(PROTOCOL_VERSION)).expect("current version"),
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn rejects_unknown_version() {
        assert!(negotiate_protocol_version(Some("belltower.v999")).is_err());
    }
}
