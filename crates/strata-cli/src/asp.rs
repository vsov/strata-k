//! `@asp` — stable-model execution via the reference solver (Phase 5).

use strata_ir::high::program::{ItemKind, Pragma, Term};
use strata_ir::high::Program;

// --- ASP (Phase 5) -----------------------------------------------------------

pub(crate) fn is_asp(prog: &Program) -> bool {
    prog.items
        .iter()
        .any(|i| matches!(&i.node, ItemKind::Pragma(Pragma::Asp)))
}

/// Enumerate the stable models of an `@asp` program via the reference solver.
pub(crate) fn run_asp(prog: &Program) -> Result<String, String> {
    use strata_asp::Val;

    let mut rules = Vec::new();
    let mut facts: Vec<(String, Vec<Val>)> = Vec::new();
    for item in &prog.items {
        match &item.node {
            ItemKind::Rule(r) => rules.push(r.clone()),
            ItemKind::Fact(f) => {
                let mut args = Vec::with_capacity(f.atom.args.len());
                let mut ground = true;
                for t in &f.atom.args {
                    match t {
                        Term::Const { name } => args.push(Val::Sym(name.clone())),
                        Term::Int { value } => args.push(Val::Int(*value)),
                        _ => ground = false,
                    }
                }
                if ground {
                    facts.push((f.atom.pred.clone(), args));
                }
            }
            _ => {}
        }
    }

    let models = strata_asp::solve(&rules, &facts, &[]).map_err(|e| e.to_string())?;
    if models.is_empty() {
        return Ok("UNSATISFIABLE\n".to_string());
    }
    let mut out = String::new();
    for (i, m) in models.iter().enumerate() {
        let atoms: Vec<String> = m.iter().map(render_asp_atom).collect();
        out.push_str(&format!("Answer {}: {{{}}}\n", i + 1, atoms.join(", ")));
    }
    Ok(out)
}

fn render_asp_atom((pred, args): &(String, Vec<strata_asp::Val>)) -> String {
    if args.is_empty() {
        pred.clone()
    } else {
        let rendered: Vec<String> = args.iter().map(render_asp_val).collect();
        format!("{pred}({})", rendered.join(", "))
    }
}

fn render_asp_val(v: &strata_asp::Val) -> String {
    match v {
        strata_asp::Val::Sym(s) => s.clone(),
        strata_asp::Val::Int(n) => n.to_string(),
    }
}
