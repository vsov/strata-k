//! Magic-sets transformation. [Phase 3]
//!
//! Bottom-up Datalog computes *all* facts of a predicate; a query usually wants
//! only those reachable from a bound argument (`anc(john, Y)?`). Magic sets
//! rewrites the program so bottom-up evaluation is **demand-driven**: it derives
//! a fact only if a `magic` predicate says the query needs it. The rewrite
//! (Beeri–Ramakrishnan) adorns predicates with bound/free patterns via a
//! left-to-right sideways-information-passing strategy, adds `magic_p` demand
//! predicates, and seeds demand from the query.
//!
//! A tiny naive evaluator is included so the transform can be *checked*: the
//! magic program must give the same query answers as the original while deriving
//! strictly fewer facts.

use std::collections::{HashMap, HashSet};

/// A rule/atom argument.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Arg {
    Var(u32),
    Const(u32),
}

/// An atom `pred(args...)`.
#[derive(Clone, Debug)]
pub struct Atom {
    pub pred: String,
    pub args: Vec<Arg>,
}

impl Atom {
    pub fn new(pred: &str, args: Vec<Arg>) -> Self {
        Atom {
            pred: pred.to_string(),
            args,
        }
    }
}

/// A Horn rule `head :- body`.
#[derive(Clone, Debug)]
pub struct Rule {
    pub head: Atom,
    pub body: Vec<Atom>,
}

/// A query: `pred`, a bound/free pattern, and the constants at the bound spots.
#[derive(Clone, Debug)]
pub struct Query {
    pub pred: String,
    pub adorn: Vec<bool>,       // true = bound
    pub bound_consts: Vec<u32>, // one per bound position, in order
}

/// A ground fact `(pred, args)`.
pub type Fact = (String, Vec<u32>);

fn idb_preds(rules: &[Rule]) -> HashSet<String> {
    rules.iter().map(|r| r.head.pred.clone()).collect()
}

fn adorned_name(pred: &str, alpha: &[bool]) -> String {
    let tag: String = alpha.iter().map(|&b| if b { 'b' } else { 'f' }).collect();
    format!("{pred}_{tag}")
}

fn magic_name(adorned: &str) -> String {
    format!("magic_{adorned}")
}

fn bound_args(atom: &Atom, alpha: &[bool]) -> Vec<Arg> {
    atom.args
        .iter()
        .zip(alpha)
        .filter(|(_, &b)| b)
        .map(|(a, _)| a.clone())
        .collect()
}

/// Adorn the program starting from the query, propagating bound/free patterns by
/// left-to-right SIPS. Returns the adorned rules and each adorned predicate's
/// bound/free pattern.
fn adorn(rules: &[Rule], query: &Query) -> (Vec<Rule>, HashMap<String, Vec<bool>>) {
    let idb = idb_preds(rules);
    let mut adorned_rules = Vec::new();
    let mut adornment: HashMap<String, Vec<bool>> = HashMap::new();
    adornment.insert(adorned_name(&query.pred, &query.adorn), query.adorn.clone());

    let mut worklist = vec![(query.pred.clone(), query.adorn.clone())];
    let mut done: HashSet<String> = HashSet::new();

    while let Some((pred, alpha)) = worklist.pop() {
        let aname = adorned_name(&pred, &alpha);
        if !done.insert(aname.clone()) {
            continue;
        }
        for rule in rules.iter().filter(|r| r.head.pred == pred) {
            // Head variables at bound positions start bound.
            let mut bound: HashSet<u32> = HashSet::new();
            for (a, &b) in rule.head.args.iter().zip(&alpha) {
                if b {
                    if let Arg::Var(v) = a {
                        bound.insert(*v);
                    }
                }
            }
            let mut new_body = Vec::new();
            for lit in &rule.body {
                if idb.contains(&lit.pred) {
                    let beta: Vec<bool> = lit
                        .args
                        .iter()
                        .map(|a| match a {
                            Arg::Const(_) => true,
                            Arg::Var(v) => bound.contains(v),
                        })
                        .collect();
                    let laname = adorned_name(&lit.pred, &beta);
                    adornment.insert(laname.clone(), beta.clone());
                    worklist.push((lit.pred.clone(), beta));
                    new_body.push(Atom::new(&laname, lit.args.clone()));
                } else {
                    new_body.push(lit.clone());
                }
                // SIPS: after this literal, all its variables are bound.
                for a in &lit.args {
                    if let Arg::Var(v) = a {
                        bound.insert(*v);
                    }
                }
            }
            adorned_rules.push(Rule {
                head: Atom::new(&aname, rule.head.args.clone()),
                body: new_body,
            });
        }
    }
    (adorned_rules, adornment)
}

/// The magic-sets rewrite of `rules` for `query`: the transformed rules plus the
/// seed fact(s) that inject the query's demand.
pub fn transform(rules: &[Rule], query: &Query) -> (Vec<Rule>, Vec<Fact>) {
    let (adorned, adornment) = adorn(rules, query);
    let mut out = Vec::new();

    for rule in &adorned {
        let head_alpha = &adornment[&rule.head.pred];
        let magic_head = Atom::new(
            &magic_name(&rule.head.pred),
            bound_args(&rule.head, head_alpha),
        );

        // Demand rules: each adorned IDB body literal is only needed once the
        // preceding literals (and the head's demand) are satisfied.
        let mut prefix: Vec<Atom> = vec![magic_head.clone()];
        for lit in &rule.body {
            if let Some(beta) = adornment.get(&lit.pred) {
                out.push(Rule {
                    head: Atom::new(&magic_name(&lit.pred), bound_args(lit, beta)),
                    body: prefix.clone(),
                });
            }
            prefix.push(lit.clone());
        }

        // Modified rule: the original, guarded by its head demand.
        let mut mbody = vec![magic_head];
        mbody.extend(rule.body.clone());
        out.push(Rule {
            head: rule.head.clone(),
            body: mbody,
        });
    }

    let seed = vec![(
        magic_name(&adorned_name(&query.pred, &query.adorn)),
        query.bound_consts.clone(),
    )];
    (out, seed)
}

// --- a minimal naive Datalog evaluator, to check the transform ---------------

fn unify<'a>(args: &'a [Arg], vals: &'a [u32], bind: &mut HashMap<u32, u32>) -> bool {
    if args.len() != vals.len() {
        return false;
    }
    for (a, &val) in args.iter().zip(vals) {
        match a {
            Arg::Const(c) => {
                if *c != val {
                    return false;
                }
            }
            Arg::Var(v) => match bind.get(v) {
                Some(&prev) if prev != val => return false,
                _ => {
                    bind.insert(*v, val);
                }
            },
        }
    }
    true
}

fn solve(
    body: &[Atom],
    i: usize,
    db: &HashSet<Fact>,
    bind: &mut HashMap<u32, u32>,
    hits: &mut Vec<HashMap<u32, u32>>,
) {
    if i == body.len() {
        hits.push(bind.clone());
        return;
    }
    let lit = &body[i];
    for (pred, vals) in db {
        if *pred != lit.pred {
            continue;
        }
        let mut b2 = bind.clone();
        if unify(&lit.args, vals, &mut b2) {
            solve(body, i + 1, db, &mut b2, hits);
        }
    }
}

/// Naive fixpoint. `edb` are the base facts; returns the full derived database.
pub fn evaluate(rules: &[Rule], edb: &[Fact]) -> HashSet<Fact> {
    let mut db: HashSet<Fact> = edb.iter().cloned().collect();
    loop {
        let before = db.len();
        let mut derived = Vec::new();
        for rule in rules {
            let mut hits = Vec::new();
            solve(&rule.body, 0, &db, &mut HashMap::new(), &mut hits);
            for b in hits {
                let args: Option<Vec<u32>> = rule
                    .head
                    .args
                    .iter()
                    .map(|a| match a {
                        Arg::Const(c) => Some(*c),
                        Arg::Var(v) => b.get(v).copied(),
                    })
                    .collect();
                if let Some(args) = args {
                    derived.push((rule.head.pred.clone(), args));
                }
            }
        }
        db.extend(derived);
        if db.len() == before {
            break;
        }
    }
    db
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: u32) -> Arg {
        Arg::Var(n)
    }

    // anc(X,Y):-par(X,Y).  anc(X,Z):-par(X,Y),anc(Y,Z).
    fn ancestor_rules() -> Vec<Rule> {
        vec![
            Rule {
                head: Atom::new("anc", vec![v(0), v(1)]),
                body: vec![Atom::new("par", vec![v(0), v(1)])],
            },
            Rule {
                head: Atom::new("anc", vec![v(0), v(2)]),
                body: vec![
                    Atom::new("par", vec![v(0), v(1)]),
                    Atom::new("anc", vec![v(1), v(2)]),
                ],
            },
        ]
    }

    // Two disjoint chains: 0→1→2→3 (john=0) and 10→11→12. The query anc(0, Y)
    // needs only the first chain; the full program also derives the second.
    fn par_edb() -> Vec<Fact> {
        vec![
            ("par".into(), vec![0, 1]),
            ("par".into(), vec![1, 2]),
            ("par".into(), vec![2, 3]),
            ("par".into(), vec![10, 11]),
            ("par".into(), vec![11, 12]),
        ]
    }

    #[test]
    fn magic_matches_full_and_derives_less() {
        let rules = ancestor_rules();
        let edb = par_edb();
        let query = Query {
            pred: "anc".into(),
            adorn: vec![true, false], // anc^bf(0, Y)
            bound_consts: vec![0],
        };

        // Full program: all anc facts.
        let full = evaluate(&rules, &edb);
        let full_anc0: HashSet<Vec<u32>> = full
            .iter()
            .filter(|(p, a)| p == "anc" && a[0] == 0)
            .map(|(_, a)| a.clone())
            .collect();

        // Magic program.
        let (mrules, seed) = transform(&rules, &query);
        let mut medb = edb.clone();
        medb.extend(seed);
        let mdb = evaluate(&mrules, &medb);
        // Adorned answers anc_bf(0, Y).
        let magic_anc0: HashSet<Vec<u32>> = mdb
            .iter()
            .filter(|(p, a)| p == "anc_bf" && a[0] == 0)
            .map(|(_, a)| a.clone())
            .collect();

        // Same query answers: {0→1, 0→2, 0→3}.
        assert_eq!(magic_anc0, full_anc0);
        assert_eq!(
            magic_anc0,
            [vec![0, 1], vec![0, 2], vec![0, 3]].into_iter().collect()
        );

        // Demand-driven: the magic run never touches the 10→11→12 chain, so it
        // derives fewer `anc*` facts than the full program.
        let full_anc = full.iter().filter(|(p, _)| p == "anc").count();
        let magic_anc = mdb.iter().filter(|(p, _)| p == "anc_bf").count();
        assert!(
            magic_anc < full_anc,
            "magic {magic_anc} should be < full {full_anc}"
        );
        // Concretely: full derives anc for the second chain too; magic must not.
        assert!(!mdb.iter().any(|(p, a)| p == "anc_bf" && a[0] == 10));
        assert!(full.iter().any(|(p, a)| p == "anc" && a[0] == 10));
    }
}
