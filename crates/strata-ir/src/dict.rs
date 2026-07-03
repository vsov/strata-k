//! Symbol dictionary: constant ↔ scalar id, one per database. [IR-8, spec 3.1]
//!
//! Core-IR references constants by [`SymbolId`], never by string. The dictionary
//! is the single owner of that mapping; eval, the EDB/TSV loader, and the
//! Soufflé translator all decode through it. Interning is deterministic within a
//! run: ids are assigned in first-seen order, so a rebuilt dictionary over the
//! same input yields the same ids.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A dictionary-interned constant. Opaque scalar; ordering is by id.
/// Canonical *output* ordering (by resolved string) is IR-10's concern.
/// Serializes as a bare integer.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct SymbolId(pub u32);

/// Bidirectional string interner for constants.
#[derive(Debug, Default, Clone)]
pub struct SymbolDict {
    by_str: HashMap<String, SymbolId>,
    by_id: Vec<String>,
}

impl SymbolDict {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `s`, returning its id (assigning a fresh one on first sight).
    pub fn intern(&mut self, s: &str) -> SymbolId {
        if let Some(&id) = self.by_str.get(s) {
            return id;
        }
        let id = SymbolId(self.by_id.len() as u32);
        self.by_id.push(s.to_owned());
        self.by_str.insert(s.to_owned(), id);
        id
    }

    /// Look up an already-interned constant without inserting.
    pub fn get(&self, s: &str) -> Option<SymbolId> {
        self.by_str.get(s).copied()
    }

    /// Resolve an id back to its string. `None` if the id is foreign to this dict.
    pub fn resolve(&self, id: SymbolId) -> Option<&str> {
        self.by_id.get(id.0 as usize).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_stable_and_deduplicating() {
        let mut d = SymbolDict::new();
        let a1 = d.intern("a");
        let b = d.intern("b");
        let a2 = d.intern("a");
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn resolve_roundtrips() {
        let mut d = SymbolDict::new();
        let id = d.intern("node42");
        assert_eq!(d.resolve(id), Some("node42"));
        assert_eq!(d.get("node42"), Some(id));
        assert_eq!(d.resolve(SymbolId(999)), None);
    }

    #[test]
    fn ids_are_first_seen_order() {
        let mut d = SymbolDict::new();
        assert_eq!(d.intern("x"), SymbolId(0));
        assert_eq!(d.intern("y"), SymbolId(1));
    }
}
