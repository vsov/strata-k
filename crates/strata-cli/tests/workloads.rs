//! Workload CI check. [honesty policy: "README numbers = real CLI output"]
//!
//! Every workload under `examples/workloads/` must run clean under today's
//! `strata` binary AND produce the exact lines its README quotes — the
//! workloads are the repository's "realistic data" claims, so their numbers
//! are pinned here, not asserted in prose. A completeness guard makes a new
//! workload directory a test failure until it is classified here.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// (directory, program, lines the run must print verbatim).
const WORKLOADS: &[(&str, &str, &[&str])] = &[
    (
        "aml",
        "aml.strata",
        &[
            "0.9833902 :: investigate(g0_hold)  (lower bound, top-4)",
            "1 :: cleared(indie0)  (lower bound, top-4)",
            "0.14 :: cleared(indie7)  (lower bound, top-4)",
            "  \u{2202}/\u{2202}[0.85 :: flag(g4_op1)] = 0.110732  (\u{2192} model \"aml_gnn\")",
        ],
    ),
    (
        "routing",
        "routing.strata",
        &[
            "route(n0_0, n7_7) = 5",
            "route(n0_0, n4_4) = 2",
            "route(n7_7, n0_0) = 44",
        ],
    ),
];

fn workloads_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/workloads")
        .canonicalize()
        .expect("examples/workloads must exist")
}

#[test]
fn workloads_run_and_print_their_pinned_lines() {
    for (dir, program, lines) in WORKLOADS {
        let path = workloads_dir().join(dir).join(program);
        let out = Command::new(env!("CARGO_BIN_EXE_strata"))
            .args(["run", path.to_str().unwrap()])
            .output()
            .expect("spawn strata");
        assert!(
            out.status.success(),
            "`strata run {dir}/{program}` should exit 0, got {:?}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in *lines {
            assert!(
                stdout.lines().any(|l| l == *line),
                "`strata run {dir}/{program}` must print the pinned line:\n  {line}\ngot:\n{}",
                stdout.lines().take(30).collect::<Vec<_>>().join("\n"),
            );
        }
    }
}

/// No silent gaps: every directory under examples/workloads must be pinned
/// above. A new workload fails this until its output lines are classified.
#[test]
fn every_workload_is_classified() {
    let classified: Vec<&str> = WORKLOADS.iter().map(|(d, _, _)| *d).collect();
    for entry in fs::read_dir(workloads_dir()).expect("read workloads dir") {
        let entry = entry.expect("dir entry");
        if !entry.file_type().expect("file type").is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().expect("utf-8 dir name");
        assert!(
            classified.contains(&name),
            "workload `{name}` is not classified in workloads.rs — pin its output lines",
        );
    }
}
