//! Points-to at scale, reporting the interning-time fraction. [Phase 3 exit]
//!
//! Builds a synthetic field-heavy program (every object has several fields that
//! are stored and loaded), runs the Andersen analysis, and reports how much of
//! the wall-clock time went to interning heap-location terms — the spec's
//! success metric ("приемлемая доля времени на интернирование").
//!
//!   cargo run -p strata-terms --example pointsto --release
//!
//! Env: STRATA_PT_VARS (objects/vars), STRATA_PT_FIELDS (fields per object).

use strata_terms::pointsto::{andersen, Program};

fn main() {
    let env = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let v = env("STRATA_PT_VARS", 300_000);
    let fields = env("STRATA_PT_FIELDS", 4);

    // Disjoint id spaces so vars, objects, load-targets and fields never collide.
    let obj = 1_000_000_000u32;
    let load_base = 500_000_000u32;
    let field_base = 2_000_000_000u32;

    let mut prog = Program::default();
    for i in 0..v {
        prog.addr_of.push((i, obj + i)); // var i = &obj_i
        for fi in 0..fields {
            let f = field_base + fi;
            // obj_i.f = &(obj_(i+1)) via a store from a var that points there,
            // then read it back into a fresh load target.
            let nbr = (i + 1) % v;
            prog.store.push((i, f, nbr)); // obj_i.f ⊇ pt(var_{i+1}) = {obj_{i+1}}
            prog.load.push((load_base + i * fields + fi, i, f)); // t = obj_i.f
        }
    }

    let an = andersen(&prog, 8);
    let (calls, created) = an.interner.stats();

    println!(
        "points-to: {} vars, {} objects, {} fields/obj  ({} store + {} load edges)",
        v,
        v,
        fields,
        prog.store.len(),
        prog.load.len()
    );
    println!(
        "result: {} points-to pairs, {} field-points-to terms, {} fixpoint rounds in {:?}",
        an.pairs(),
        an.fpt.len(),
        an.rounds,
        an.total_time
    );
    println!(
        "interning: {} calls, {} distinct terms ({:.1}% hash-cons hits), {:?} = {:.1}% of total",
        calls,
        created,
        100.0 * (1.0 - created as f64 / calls as f64),
        an.intern_time,
        100.0 * an.intern_fraction(),
    );
    // Spot-check: obj_0.f holds obj_1, so the load target for (var 0, field 0)
    // must point to obj_1.
    let t0 = load_base;
    assert!(an.pt[&t0].contains(&(obj + 1)), "field load correctness");
    println!("OK — points-to computed, field flow verified.");
}
