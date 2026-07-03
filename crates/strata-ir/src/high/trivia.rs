//! Trivia: comments carried on the nearest node for fmt roundtrip. [IR-5, D12]
//!
//! **Contract:** trivia is a High-IR-only concern. It is excluded from Core-IR
//! and MUST NOT participate in semantic equality — two programs differing only in
//! comments are semantically equal. Carriers implement equality that skips
//! trivia; see [`crate::high::Item`]. Attachment (which comment binds to which
//! node) is the parser/printer's job (FRONT-10); this module only defines the
//! representation and the contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Comments and blank-line hints attached to a node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Trivia {
    /// Comment lines appearing immediately before the node (in source order).
    pub leading: Vec<String>,
    /// A trailing same-line comment, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing: Option<String>,
    /// Whether a blank line preceded the node (canonical printer may honor it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub blank_before: bool,
}

impl Trivia {
    pub fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_none() && !self.blank_before
    }
}
