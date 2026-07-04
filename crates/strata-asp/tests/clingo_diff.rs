//! Phase-5 exit criterion (spec §9): "agreement of the model sets with clingo on
//! ASP-competition tasks". Each task is built twice — once as clingo source, once
//! as our normalized normal program (choice/cardinality expanded by
//! `strata_asp::normalize`) — then the answer sets are compared. Skips cleanly if
//! clingo is not installed (mirrors the Soufflé differential harness).

use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};

use strata_asp::clasp::solve_with;
use strata_asp::normalize::{normalize, ChoiceRule, FreshGen};
use strata_asp::simplify::{reduction, simplify};
use strata_asp::{ground, GroundAtom, Val};
use strata_ir::high::program::{atom, Atom, Literal, Rule, Term};

// --- clingo runner -----------------------------------------------------------

/// Run clingo on `lp`, enumerating all answer sets, projected to the shown
/// predicate. Returns `(count, model-sets-as-strings)` or `None` if clingo is
/// absent. clingo's default output is `Answer: N` followed by the model line.
fn clingo_models(lp: &str) -> Option<BTreeSet<BTreeSet<String>>> {
    let mut child = Command::new("clingo")
        .args(["-n", "0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?; // clingo not installed → skip
    let mut stdin = child.stdin.take()?;
    let lp = lp.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(lp.as_bytes());
    });
    let out = child.wait_with_output().ok()?;
    let _ = writer.join();
    let text = String::from_utf8_lossy(&out.stdout);

    let lines: Vec<&str> = text.lines().collect();
    let mut models: BTreeSet<BTreeSet<String>> = BTreeSet::new();
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with("Answer:") {
            let m: BTreeSet<String> = lines
                .get(i + 1)
                .copied()
                .unwrap_or("")
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            models.insert(m);
        }
    }
    Some(models)
}

// --- rendering our models to clingo's textual atoms --------------------------

fn render_val(v: &Val) -> String {
    match v {
        Val::Sym(s) => s.clone(),
        Val::Int(n) => n.to_string(),
    }
}
fn render_atom((pred, args): &GroundAtom) -> String {
    if args.is_empty() {
        pred.clone()
    } else {
        let a: Vec<String> = args.iter().map(render_val).collect();
        format!("{pred}({})", a.join(","))
    }
}

/// Our production pipeline: normalize (done by caller) → ground → aspif → clasp.
/// Returns the answer sets projected to `show_pred` as string sets, or `None` if
/// clasp is not installed. This is the Phase-5 path (the exponential reference
/// solver in `solve` does not scale to competition sizes).
fn our_models(
    rules: &[Rule],
    facts: &[GroundAtom],
    cons: &[Vec<Literal>],
    show_pred: &str,
) -> Option<BTreeSet<BTreeSet<String>>> {
    let g = ground(rules, facts, cons).unwrap();
    let models = solve_with("clasp", &g)?;
    Some(
        models
            .into_iter()
            .map(|m| {
                m.iter()
                    .filter(|(p, _)| p == show_pred)
                    .map(render_atom)
                    .collect()
            })
            .collect(),
    )
}

// --- helpers to build both forms ---------------------------------------------

fn ic(n: i64) -> Term {
    Term::Int { value: n }
}
fn iv(pred: &str, n: i64) -> Atom {
    atom(pred, vec![ic(n)])
}
fn iv2(pred: &str, a: i64, b: i64) -> Atom {
    atom(pred, vec![ic(a), ic(b)])
}
fn fact1(pred: &str, n: i64) -> GroundAtom {
    (pred.to_string(), vec![Val::Int(n)])
}
fn fact2(pred: &str, a: i64, b: i64) -> GroundAtom {
    (pred.to_string(), vec![Val::Int(a), Val::Int(b)])
}

/// Assert our reference answer sets equal clingo's, projected to `show_pred`.
/// Skips if clingo is unavailable.
fn assert_agrees(
    name: &str,
    lp: &str,
    rules: &[Rule],
    facts: &[GroundAtom],
    cons: &[Vec<Literal>],
    show_pred: &str,
) {
    let Some(c_models) = clingo_models(lp) else {
        eprintln!("skipping {name}: clingo not installed");
        return;
    };
    let Some(ours) = our_models(rules, facts, cons, show_pred) else {
        eprintln!("skipping {name}: clasp not installed");
        return;
    };
    assert_eq!(
        ours.len(),
        c_models.len(),
        "{name}: model COUNT differs (ours {} vs clingo {})",
        ours.len(),
        c_models.len()
    );
    assert_eq!(ours, c_models, "{name}: model SETS differ from clingo");
}

// --- Task 1: graph 3-coloring ------------------------------------------------

#[test]
fn coloring_matches_clingo() {
    // Graph: 4-cycle 1-2-3-4-1 plus chord 1-3.
    let nodes = [1, 2, 3, 4];
    let edges = [(1, 2), (2, 3), (3, 4), (4, 1), (1, 3)];
    let colors = [1, 2, 3];

    let mut lp = String::new();
    for n in nodes {
        lp += &format!("node({n}).\n");
    }
    for (x, y) in edges {
        lp += &format!("edge({x},{y}).\n");
    }
    lp += "1 { color(X,C) : color_id(C) } 1 :- node(X).\n";
    for c in colors {
        lp += &format!("color_id({c}).\n");
    }
    lp += ":- edge(X,Y), color(X,C), color(Y,C).\n#show color/2.\n";

    // Our form: a ground exactly-1 choice per node; a first-order clash constraint.
    let mut rules = Vec::new();
    let mut cons = Vec::new();
    let mut facts: Vec<GroundAtom> = Vec::new();
    let mut fresh = FreshGen::new();
    for n in nodes {
        facts.push(fact1("node", n));
    }
    for (x, y) in edges {
        facts.push(fact2("edge", x, y));
    }
    for n in nodes {
        let cr = ChoiceRule {
            choices: colors.iter().map(|&c| iv2("color", n, c)).collect(),
            lower: Some(1),
            upper: Some(1),
            body: vec![Literal::Pos(iv("node", n))],
        };
        let (r, c) = normalize(&cr, &mut fresh);
        rules.extend(r);
        cons.extend(c);
    }
    // :- edge(X,Y), color(X,C), color(Y,C).
    use strata_ir::high::program::var;
    cons.push(vec![
        Literal::Pos(atom("edge", vec![var("X"), var("Y")])),
        Literal::Pos(atom("color", vec![var("X"), var("C")])),
        Literal::Pos(atom("color", vec![var("Y"), var("C")])),
    ]);

    assert_agrees("3-coloring", &lp, &rules, &facts, &cons, "color");

    // §5.2: the GPU-style simplification pass must PRESERVE the answer sets while
    // shrinking the program. Ground → simplify → clasp must still equal clingo.
    let Some(c_models) = clingo_models(&lp) else {
        return;
    };
    let g = ground(&rules, &facts, &cons).unwrap();
    let s = simplify(&g);
    let (rb, ra, lb, la) = reduction(&g, &s);
    assert!(ra <= rb && la <= lb, "simplify grew the program");
    assert!(la < lb, "simplify removed no literals on 3-coloring");
    if let Some(models) = solve_with("clasp", &s) {
        let projected: BTreeSet<BTreeSet<String>> = models
            .into_iter()
            .map(|m| {
                m.iter()
                    .filter(|(p, _)| p == "color")
                    .map(render_atom)
                    .collect()
            })
            .collect();
        assert_eq!(
            projected, c_models,
            "simplified program's answer sets diverge from clingo"
        );
    }
}

// --- Task 2: independent sets ------------------------------------------------

#[test]
fn independent_set_matches_clingo() {
    let nodes = [1, 2, 3, 4, 5];
    let edges = [(1, 2), (2, 3), (3, 4), (4, 5), (5, 1)]; // 5-cycle

    let mut lp = String::new();
    for n in nodes {
        lp += &format!("node({n}).\n");
    }
    for (x, y) in edges {
        lp += &format!("edge({x},{y}).\n");
    }
    lp += "{ in(X) } :- node(X).\n:- edge(X,Y), in(X), in(Y).\n#show in/1.\n";

    let mut rules = Vec::new();
    let mut cons = Vec::new();
    let mut facts: Vec<GroundAtom> = Vec::new();
    let mut fresh = FreshGen::new();
    for n in nodes {
        facts.push(fact1("node", n));
    }
    for (x, y) in edges {
        facts.push(fact2("edge", x, y));
    }
    for n in nodes {
        let cr = ChoiceRule {
            choices: vec![iv("in", n)],
            lower: None,
            upper: None,
            body: vec![Literal::Pos(iv("node", n))],
        };
        let (r, c) = normalize(&cr, &mut fresh);
        rules.extend(r);
        cons.extend(c);
    }
    use strata_ir::high::program::var;
    cons.push(vec![
        Literal::Pos(atom("edge", vec![var("X"), var("Y")])),
        Literal::Pos(atom("in", vec![var("X")])),
        Literal::Pos(atom("in", vec![var("Y")])),
    ]);

    assert_agrees("independent-set", &lp, &rules, &facts, &cons, "in");
}

// --- Task 3: non-attacking rooks (permutations) ------------------------------

#[test]
fn rooks_matches_clingo() {
    // 1 rook per row on a 4x4 board, no two in the same column ⇒ 4! = 24.
    let n = 4;
    let mut lp = String::new();
    for r in 1..=n {
        lp += &format!("row({r}).\n");
    }
    for c in 1..=n {
        lp += &format!("col({c}).\n");
    }
    lp += "1 { q(R,C) : col(C) } 1 :- row(R).\n";
    lp += ":- q(R1,C), q(R2,C), R1<R2.\n#show q/2.\n";

    let mut rules = Vec::new();
    let mut cons = Vec::new();
    let mut facts: Vec<GroundAtom> = Vec::new();
    let mut fresh = FreshGen::new();
    for r in 1..=n {
        facts.push(fact1("row", r));
    }
    for c in 1..=n {
        facts.push(fact1("col", c));
    }
    for r in 1..=n {
        let cr = ChoiceRule {
            choices: (1..=n).map(|c| iv2("q", r, c)).collect(),
            lower: Some(1),
            upper: Some(1),
            body: vec![Literal::Pos(iv("row", r))],
        };
        let (rr, cc) = normalize(&cr, &mut fresh);
        rules.extend(rr);
        cons.extend(cc);
    }
    // no two rooks in the same column: :- q(R1,C), q(R2,C), R1<R2  → precompute R1<R2 pairs
    // Our constraints lack arithmetic, so enumerate the ordered row pairs as facts.
    use strata_ir::high::program::var;
    for r1 in 1..=n {
        for r2 in (r1 + 1)..=n {
            facts.push(fact2("lt", r1, r2));
        }
    }
    // add lt/2 to clingo too so both share the exact same column constraint form
    // (clingo's R1<R2 already encodes it; the reference uses lt facts).
    cons.push(vec![
        Literal::Pos(atom("q", vec![var("R1"), var("C")])),
        Literal::Pos(atom("q", vec![var("R2"), var("C")])),
        Literal::Pos(atom("lt", vec![var("R1"), var("R2")])),
    ]);

    assert_agrees("rooks-4x4", &lp, &rules, &facts, &cons, "q");
}
