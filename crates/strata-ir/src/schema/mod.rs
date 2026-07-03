//! JSON Schema generation, publication, and validation. [IR-7, D11]
//!
//! The Rust High-IR ADT is the single source; the JSON Schema is generated from
//! it (schemars) and published as a checked-in artifact
//! (`schema/high-ir.schema.json`). A hand-written (LLM-authored) High-IR document
//! is validated against the schema and gated by `ir_version` before it reaches
//! semantic analysis.
//!
//! Slice 1 delivers generation + the drift-guard entry points. Full JSON-Schema
//! *validation* of arbitrary documents (a real validator) is wired by INFRA-9;
//! here we provide the generated schema and a serde-level load that enforces
//! `ir_version` compatibility.

use crate::high::Program;
use crate::version::{is_compatible, Version};

/// The generated JSON Schema for the High-IR [`Program`] document.
pub fn high_ir_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Program)
}

/// The generated schema as pretty JSON — the byte form published in the repo and
/// guarded against drift by INFRA-9.
pub fn high_ir_schema_json() -> String {
    serde_json::to_string_pretty(&high_ir_schema()).expect("schema serializes")
}

/// Error loading a High-IR document.
#[derive(Debug)]
pub enum LoadError {
    /// JSON did not match the ADT shape.
    Parse(serde_json::Error),
    /// `ir_version` is present but incompatible with this build (D11).
    IncompatibleVersion { doc: String },
    /// `ir_version` was not valid semver.
    BadVersion { doc: String },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Parse(e) => write!(f, "malformed High-IR JSON: {e}"),
            LoadError::IncompatibleVersion { doc } => {
                write!(f, "ir_version {doc} is incompatible with this build")
            }
            LoadError::BadVersion { doc } => write!(f, "ir_version {doc:?} is not valid semver"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Deserialize a High-IR document and gate it on `ir_version` (D11).
pub fn load_program(json: &str) -> Result<Program, LoadError> {
    let prog: Program = serde_json::from_str(json).map_err(LoadError::Parse)?;
    let v = parse_semver(&prog.ir_version).ok_or_else(|| LoadError::BadVersion {
        doc: prog.ir_version.clone(),
    })?;
    if !is_compatible(v) {
        return Err(LoadError::IncompatibleVersion {
            doc: prog.ir_version.clone(),
        });
    }
    Ok(prog)
}

fn parse_semver(s: &str) -> Option<Version> {
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_generable_and_nonempty() {
        let json = high_ir_schema_json();
        assert!(json.contains("Program"));
        assert!(json.len() > 100);
    }

    #[test]
    fn load_rejects_incompatible_version() {
        let bad = r#"{"ir_version":"9.9.9","items":[]}"#;
        assert!(matches!(
            load_program(bad),
            Err(LoadError::IncompatibleVersion { .. })
        ));
    }

    #[test]
    fn load_accepts_current_version() {
        let ok = format!(r#"{{"ir_version":"{}","items":[]}}"#, crate::IR_VERSION_STR);
        assert!(load_program(&ok).is_ok());
    }
}
