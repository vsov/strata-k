//! Reference relation storage. [EVAL-2]
//!
//! Sorted, deduplicated tuples keyed by argument vector, each carrying a
//! semiring annotation. Deliberately simple and correct (a `BTreeMap`), NOT the
//! GPU columnar layout — this is the oracle.

use std::collections::BTreeMap;

use strata_ir::core::{CoreProgram, Semiring};

use crate::value::{Ann, GroundVal};

pub type Tuple = Vec<GroundVal>;

/// One relation: its arity, its semiring, and its tuples→annotation map.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    pub arity: u32,
    pub semiring: Semiring,
    pub rows: BTreeMap<Tuple, Ann>,
}

impl Relation {
    pub fn new(arity: u32, semiring: Semiring) -> Self {
        Self {
            arity,
            semiring,
            rows: BTreeMap::new(),
        }
    }

    /// `⊕`-combine a derived tuple into the relation. Returns whether the
    /// relation changed (new tuple, or a strictly better Trop weight).
    pub fn combine(&mut self, tuple: Tuple, ann: Ann) -> bool {
        match self.rows.get(&tuple) {
            None => {
                self.rows.insert(tuple, ann);
                true
            }
            Some(&existing) => {
                let (merged, changed) = existing.oplus(ann);
                if changed {
                    self.rows.insert(tuple, merged);
                }
                changed
            }
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// The whole database: one relation per predicate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Db {
    rels: BTreeMap<String, Relation>,
}

impl Db {
    /// Allocate an empty relation for every predicate declared in `prog`.
    pub fn from_program(prog: &CoreProgram) -> Self {
        let mut rels = BTreeMap::new();
        for p in &prog.predicates {
            rels.insert(p.name.clone(), Relation::new(p.arity, p.semiring));
        }
        Db { rels }
    }

    pub fn relation(&self, pred: &str) -> Option<&Relation> {
        self.rels.get(pred)
    }

    pub fn relation_mut(&mut self, pred: &str) -> Option<&mut Relation> {
        self.rels.get_mut(pred)
    }

    /// Seed an EDB tuple (or any fact) into `pred`'s relation.
    pub fn insert(&mut self, pred: &str, tuple: Tuple, ann: Ann) -> bool {
        self.rels
            .get_mut(pred)
            .unwrap_or_else(|| panic!("insert into unknown relation {pred:?}"))
            .combine(tuple, ann)
    }

    pub fn predicates(&self) -> impl Iterator<Item = &String> {
        self.rels.keys()
    }
}
