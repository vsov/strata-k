//! GNN-oracle ablation (spec §5.4, §10). Two regimes over graph colouring, with
//! a message-passing GNN oracle emitted to clingo as `#heuristic` level hints:
//!
//! - **SAT / enumerate-all** — an easy distribution where guidance barely moves
//!   the search (the honest null of §10).
//! - **UNSAT / refute** — instances with an embedded `K₄` clique (no 3-colouring),
//!   where proving unsatisfiability is search-heavy and *order matters*.
//!
//! Each regime runs three configurations — no guidance (baseline), the GNN
//! oracle, and an **anti-oracle** negative control (deciding the least-central
//! vertex first). The ablation:
//!   * ASSERTS the correctness invariant И3 — every configuration reports the same
//!     answer-set count (the hint only reorders search), and
//!   * REPORTS the search effort, so the harness's discriminating power is
//!     visible: a bad order is no better than none, a good one can help.
//!
//! Skips cleanly if clingo is absent.

use std::io::Write;
use std::process::{Command, Stdio};

use strata_asp::heuristic::Gnn;

struct Graph {
    n: usize,
    edges: Vec<(usize, usize)>,
    colors: usize,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn upto(&mut self, m: usize) -> usize {
        (self.next() >> 11) as usize % m
    }
}

fn random_graph(rng: &mut Rng, n: usize, m: usize, colors: usize, clique: usize) -> Graph {
    let mut set = std::collections::BTreeSet::new();
    // embed a (clique)-clique on vertices 0..clique (forces >clique-1 colours).
    for a in 0..clique {
        for b in (a + 1)..clique {
            set.insert((a, b));
        }
    }
    while set.len() < m {
        let (a, b) = (rng.upto(n), rng.upto(n));
        if a != b {
            set.insert((a.min(b), a.max(b)));
        }
    }
    Graph {
        n,
        edges: set.into_iter().collect(),
        colors,
    }
}

/// Undirected adjacency lists (for the GNN).
fn adjacency(g: &Graph) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); g.n];
    for &(a, b) in &g.edges {
        adj[a].push(b);
        adj[b].push(a);
    }
    adj
}

#[derive(Clone, Copy)]
enum Guide {
    None,
    Gnn,
    Anti,
}

fn program(g: &Graph, guide: Guide) -> String {
    let mut s = String::new();
    for v in 0..g.n {
        s += &format!("node({v}).\n");
    }
    for &(a, b) in &g.edges {
        s += &format!("edge({a},{b}).\n");
    }
    for c in 0..g.colors {
        s += &format!("color({c}).\n");
    }
    s += "1 { col(X,C) : color(C) } 1 :- node(X).\n";
    s += ":- edge(X,Y), col(X,C), col(Y,C).\n";
    let levels = match guide {
        Guide::None => None,
        Guide::Gnn => Some(Gnn::trained().levels(&adjacency(g))),
        Guide::Anti => Some(Gnn::anti().levels(&adjacency(g))),
    };
    if let Some(lv) = levels {
        for (v, &level) in lv.iter().enumerate() {
            if level != 0 {
                s += &format!("#heuristic col({v},C) : color(C). [{level}, level]\n");
            }
        }
    }
    s
}

/// clingo enumerate-all; return `(models, choices, conflicts)` or `None`.
fn run(lp: &str, hint: bool) -> Option<(u64, u64, u64)> {
    let mut args = vec!["-n", "0", "--stats", "-q"];
    if hint {
        args.push("--heuristic=Domain");
    }
    let mut child = Command::new("clingo")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let lp = lp.to_string();
    let w = std::thread::spawn(move || {
        let _ = stdin.write_all(lp.as_bytes());
    });
    let out = child.wait_with_output().ok()?;
    let _ = w.join();
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |name: &str| -> u64 {
        text.lines()
            .find(|l| l.trim_start().starts_with(name))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    Some((field("Models"), field("Choices"), field("Conflicts")))
}

/// Run one regime across the three configurations; assert И3, report effort.
fn ablate(name: &str, graphs: &[Graph], effort: impl Fn(u64, u64) -> u64) {
    let (mut base, mut gnn, mut anti) = (0u64, 0u64, 0u64);
    for g in graphs {
        let (mb, cb, kb) = run(&program(g, Guide::None), false).unwrap();
        let (mg, cg, kg) = run(&program(g, Guide::Gnn), true).unwrap();
        let (ma, ca, ka) = run(&program(g, Guide::Anti), true).unwrap();
        // И3: guidance never changes which/how many answer sets exist.
        assert_eq!(mb, mg, "{name}: GNN changed the answer-set count");
        assert_eq!(mb, ma, "{name}: anti changed the answer-set count");
        base += effort(cb, kb);
        gnn += effort(cg, kg);
        anti += effort(ca, ka);
    }
    eprintln!("ablation [{name}] effort — baseline {base}, GNN {gnn}, anti {anti}");
}

/// Under `STRATA_REQUIRE_ORACLES` (the oracle CI job), a missing external
/// oracle is a hard failure — the differential must actually run. Locally,
/// absence skips cleanly (INFRA-11).
fn skip_or_die(what: &str) {
    if std::env::var_os("STRATA_REQUIRE_ORACLES").is_some() {
        panic!("{what} — but STRATA_REQUIRE_ORACLES is set, the oracle differential must run");
    }
    eprintln!("skipping: {what}");
}

#[test]
fn gnn_ablation_two_regimes() {
    let mut rng = Rng(0x5EED_5EED);

    // probe clingo once.
    let probe = Graph {
        n: 4,
        edges: vec![(0, 1)],
        colors: 3,
    };
    if run(&program(&probe, Guide::None), false).is_none() {
        skip_or_die("ablation: clingo not installed");
        return;
    }

    // Regime 1: SAT, enumerate-all — easy; effort ≈ choices. Honest null.
    let sat: Vec<Graph> = (0..6)
        .map(|_| random_graph(&mut rng, 9, 13, 3, 0))
        .collect();
    ablate("SAT/enumerate", &sat, |choices, _| choices);

    // Regime 2: UNSAT refutation — embed a K₄ clique so 3-colouring is impossible;
    // proving UNSAT is search-heavy and order-sensitive. Effort = conflicts.
    let unsat: Vec<Graph> = (0..8)
        .map(|_| random_graph(&mut rng, 12, 24, 3, 4))
        .collect();
    // sanity: these really are UNSAT (0 models).
    for g in &unsat {
        assert_eq!(
            run(&program(g, Guide::None), false).unwrap().0,
            0,
            "expected UNSAT"
        );
    }
    ablate("UNSAT/refute", &unsat, |_, conflicts| conflicts);
}
