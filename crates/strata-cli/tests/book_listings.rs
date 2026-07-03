//! Book-listing CI check. [honesty policy: "book output = real CLI output"]
//!
//! Every runnable Strata/K listing in the book must actually run under today's
//! `strata` CLI; every listing the book presents as *rejected* (the checker
//! catching an LLM's first draft) or as *future syntax* (parses, not yet
//! executable) must fail exactly that way. This test invokes the real binary so
//! the guarantee is end-to-end, and a completeness guard makes a newly added
//! `examples/book/**` listing a test failure until it is classified here.

use std::path::PathBuf;
use std::process::Command;

/// Listings that must `strata run` cleanly (exit 0). These back the book's
/// non-future-syntax code blocks, whose printed output equals real CLI output.
const RUNNABLE: &[&str] = &[
    "ch01-ownership.strata",
    "ch06-trading-house.strata",
    "ch07-routes.strata",
    "ch07-shared-evidence.strata",
    "ch08-portfolio.strata",
    "ch09-vignette-draft2.strata",
    "ch09-vignette.strata",
];

/// Listings the book shows being *rejected*: each must fail `strata check` with
/// diagnostics (exit 1) and emit the named stable code. `ch09-vignette-draft`
/// is the LLM's wrong first attempt the checker catches (ch. 9); `ch11-neural`
/// is a future-syntax frame that parses then reports "not implemented" (ch. 11).
const CHECK_FAILS: &[(&str, &str)] = &[
    ("ch09-vignette-draft.strata", "E1001"),
    ("ch11-neural.strata", "E0100"),
];

fn book_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/strata-cli; workspace root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/book")
        .canonicalize()
        .expect("examples/book must exist")
}

fn strata() -> Command {
    Command::new(env!("CARGO_BIN_EXE_strata"))
}

#[test]
fn runnable_listings_run_clean() {
    for name in RUNNABLE {
        let path = book_dir().join(name);
        let out = strata()
            .args(["run", path.to_str().unwrap()])
            .output()
            .expect("spawn strata");
        assert!(
            out.status.success(),
            "`strata run {name}` should exit 0, got {:?}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

#[test]
fn rejected_listings_fail_check_with_expected_code() {
    for (name, code) in CHECK_FAILS {
        let path = book_dir().join(name);
        let out = strata()
            .args(["check", path.to_str().unwrap()])
            .output()
            .expect("spawn strata");
        assert_eq!(
            out.status.code(),
            Some(1),
            "`strata check {name}` should exit 1 (diagnostics)",
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(code),
            "`strata check {name}` should report {code}\nstderr:\n{stderr}",
        );
    }
}

/// No silent gaps: every `*.strata` under examples/book must be classified in
/// exactly one of the lists above. A new listing fails this until triaged.
#[test]
fn every_book_listing_is_classified() {
    let mut classified: Vec<&str> = RUNNABLE.to_vec();
    classified.extend(CHECK_FAILS.iter().map(|(n, _)| *n));

    let mut found = Vec::new();
    for entry in walk(&book_dir()) {
        if entry.extension().and_then(|e| e.to_str()) == Some("strata") {
            found.push(entry.file_name().unwrap().to_str().unwrap().to_string());
        }
    }
    found.sort();
    for name in &found {
        assert!(
            classified.contains(&name.as_str()),
            "book listing `{name}` is not classified in book_listings.rs \
             (add it to RUNNABLE or CHECK_FAILS)",
        );
    }
    assert_eq!(
        found.len(),
        classified.len(),
        "classified set names a listing that no longer exists on disk",
    );
}

/// Recursively collect files under `dir` (examples/book has one nested level).
fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read book dir").flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
