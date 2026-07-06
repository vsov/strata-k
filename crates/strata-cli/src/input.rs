//! `input pred from "file.tsv"` EDB loading (CLI-5, D10).

use std::path::Path;

use strata_ir::core::{CoreProgram, Semiring};
use strata_ir::dict::SymbolDict;
use strata_ir::high::program::ItemKind;
use strata_ir::high::Program;
use strata_ir::trop::Weight;
use strata_ir::value::{GroundFact, GroundVal};

/// Load every `input pred from "file.tsv"` declaration's EDB (CLI-5, D10).
///
/// TSV convention (Soufflé-compatible): one row per fact, tab-separated columns
/// interned as symbol constants; a `Trop` predicate has one extra trailing
/// integer weight column. Constants are interned into `dict` so they align with
/// the constants the checker already lowered.
pub(crate) fn load_inputs(
    program: &Program,
    core: &CoreProgram,
    dict: &mut SymbolDict,
    base: &Path,
) -> Result<Vec<GroundFact>, String> {
    let mut out = Vec::new();
    for item in &program.items {
        let ItemKind::Input(inp) = &item.node else {
            continue;
        };
        let pred = core
            .predicates
            .iter()
            .find(|p| p.name == inp.pred)
            .ok_or_else(|| format!("input predicate `{}` is not declared/executable", inp.pred))?;
        let is_trop = pred.semiring == Semiring::Trop;
        let ncols = pred.arity as usize + usize::from(is_trop);
        let path = base.join(&inp.path);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() != ncols {
                return Err(format!(
                    "{}:{}: `{}` expects {ncols} column(s), found {}",
                    path.display(),
                    i + 1,
                    inp.pred,
                    cols.len()
                ));
            }
            let args = cols[..pred.arity as usize]
                .iter()
                .map(|c| GroundVal::Sym(dict.intern(c)))
                .collect();
            let weight = if is_trop {
                let raw = cols[pred.arity as usize];
                Some(Weight::Finite(raw.parse::<i64>().map_err(|_| {
                    format!("{}:{}: bad weight {raw:?}", path.display(), i + 1)
                })?))
            } else {
                None
            };
            out.push(GroundFact {
                pred: inp.pred.clone(),
                args,
                weight,
            });
        }
    }
    Ok(out)
}
