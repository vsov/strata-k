//! IR version + compatibility policy. [IR-1, D2, D11]
//!
//! Every High-IR document carries an `ir_version`. The compiler refuses to load
//! a document whose version it cannot honor. Pre-1.0 (0.x) policy: the MINOR
//! component is the breaking axis, so two 0.x versions are compatible iff they
//! share the same minor. From 1.0 on this switches to standard semver (same
//! major, doc-minor <= tool-minor).

/// The IR version this build emits and understands.
pub const IR_VERSION: Version = Version {
    major: 0,
    minor: 1,
    patch: 0,
};

/// String form of [`IR_VERSION`]. Kept in sync by a test below.
pub const IR_VERSION_STR: &str = "0.1.0";

/// A parsed semantic version. Kept tiny and dependency-free at Slice 0; IR-1 may
/// swap in the `semver` crate behind this same API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

/// Can this build load a document authored at `doc`?
///
/// - 0.x: breaking axis is MINOR — compatible iff same major(0) and same minor.
/// - >=1.0: compatible iff same major and `doc.minor <= IR_VERSION.minor`.
pub fn is_compatible(doc: Version) -> bool {
    let tool = IR_VERSION;
    if tool.major != doc.major {
        return false;
    }
    if tool.major == 0 {
        tool.minor == doc.minor
    } else {
        doc.minor <= tool.minor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_version_is_compatible() {
        assert!(is_compatible(IR_VERSION));
    }

    #[test]
    fn zero_x_minor_is_breaking() {
        // 0.1.x vs 0.2.x: incompatible (minor is the breaking axis pre-1.0).
        assert!(!is_compatible(Version::new(0, 2, 0)));
        // patch differences within the same minor are fine.
        assert!(is_compatible(Version::new(0, 1, 7)));
    }

    #[test]
    fn major_mismatch_is_incompatible() {
        assert!(!is_compatible(Version::new(1, 0, 0)));
    }

    #[test]
    fn version_str_matches_struct() {
        let expected = format!(
            "{}.{}.{}",
            IR_VERSION.major, IR_VERSION.minor, IR_VERSION.patch
        );
        assert_eq!(IR_VERSION_STR, expected);
    }
}
