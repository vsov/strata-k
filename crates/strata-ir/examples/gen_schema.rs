//! Regenerate the published High-IR JSON Schema. [IR-7, D11]
//!
//! Run: `cargo run -p strata-ir --example gen_schema`
//! Writes `schema/high-ir.schema.json` at the repo root. A drift-guard test
//! (INFRA-9, previewed in tests/roundtrip.rs) asserts the committed file matches.

fn main() {
    let mut json = strata_ir::schema::high_ir_schema_json();
    json.push('\n'); // trailing newline for tidy diffs
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schema/high-ir.schema.json"
    );
    std::fs::write(path, json).expect("write schema");
    println!("wrote {path}");
}
