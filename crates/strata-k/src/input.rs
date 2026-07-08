//! `input pred from "file"` EDB loading (CLI-5, D10) — TSV, CSV, or JSON,
//! dispatched by extension.
//!
//! Row convention, per format:
//! - **TSV** (Soufflé-compatible): tab-separated columns.
//! - **CSV**: comma-separated; double-quoted fields with `""` escapes.
//! - **JSON**: an array of rows, each row an array of values — strings intern
//!   as symbols, integers stay integers.
//!
//! Column convention, per predicate:
//! - a `Trop` predicate takes one trailing **integer weight** column;
//! - a `neural` predicate takes one trailing **probability** column in [0, 1]
//!   (its facts are the model's soft outputs — a certain row would be the
//!   E1010 category error), loaded into the probabilistic EDB;
//! - every other predicate loads certain facts, no extra column.
//!
//! Value columns are typed by the declaration: an `int` column yields
//! `GroundVal::Int` regardless of format (text `"5"`, JSON `5`, inline `5` are
//! one value — the value space never silently splits on load path), a domain
//! column yields an interned symbol, and empty cells are errors.

use std::path::Path;

use strata_check::Checked;
use strata_ir::core::Semiring;
use strata_ir::high::program::ItemKind;
use strata_ir::high::sig::ArgType;
use strata_ir::high::Program;
use strata_ir::trop::Weight;
use strata_ir::value::{GroundFact, GroundVal};

/// One parsed cell: a bare value or (in the trailing column) a number.
#[derive(Debug)]
enum Cell {
    Text(String),
    Int(i64),
    Float(f64),
}

impl Cell {
    fn from_str(s: &str) -> Cell {
        // TSV/CSV cells are text; the trailing-column parsers re-read them.
        Cell::Text(s.to_string())
    }
}

/// Resolve every `input pred from "file"` declaration relative to `base`,
/// pushing certain rows into `checked.edb` and neural (soft) rows into
/// `checked.prob_edb`. Value columns are read **against the declared column
/// types**: an `int` column parses as an integer, a domain column interns as a
/// symbol — so a numeric key means the same value loaded from a file, from
/// JSON, or written inline.
pub fn load_inputs(program: &Program, checked: &mut Checked, base: &Path) -> Result<(), String> {
    if checked.inputs_loaded {
        // A second load would append the same `input` rows again — doubling any
        // neural/soft facts and silently shifting every marginal, the same
        // misuse `attach_models` refuses. There is no cheap way to tell
        // input-derived rows from inline facts to make reload idempotent, so
        // refuse instead of corrupting.
        return Err("load_inputs was already called for this program; a second \
                    call would duplicate the input rows (compile a fresh program \
                    instead)"
            .to_string());
    }
    // Accumulate into local buffers and commit only after every file and row
    // has validated: a failure partway through (e.g. the second `input` file is
    // missing) must leave `checked` untouched, so a retry after fixing the file
    // does not double the rows already read. (Dictionary interning during the
    // scan is idempotent, so an aborted load leaving extra symbols is harmless.)
    let mut new_edb: Vec<GroundFact> = Vec::new();
    let mut new_prob_edb: Vec<(String, Vec<GroundVal>, f64)> = Vec::new();
    for item in &program.items {
        let ItemKind::Input(inp) = &item.node else {
            continue;
        };
        let pred = checked
            .core
            .predicates
            .iter()
            .find(|p| p.name == inp.pred)
            .ok_or_else(|| format!("input predicate `{}` is not declared/executable", inp.pred))?;
        let arity = pred.arity as usize;
        let is_trop = pred.semiring == Semiring::Trop;
        let is_neural = checked.neural.iter().any(|(n, _)| n == &inp.pred);
        let ncols = arity + usize::from(is_trop || is_neural);
        // Column types from the declaration (the High-IR signature).
        let col_types: Vec<ArgType> = program
            .items
            .iter()
            .find_map(|it| match &it.node {
                ItemKind::Predicate(p) if p.name == inp.pred => Some(p.sig.args.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let path = base.join(&inp.path);

        let rows: Vec<(usize, Vec<Cell>)> = match path.extension().and_then(|e| e.to_str()) {
            Some("tsv") | None => delimited_rows(&path, '\t')?,
            Some("csv") => delimited_rows(&path, ',')?,
            Some("json") => json_rows(&path)?,
            Some(other) => {
                return Err(format!(
                    "{}: unsupported input format `.{other}` (tsv, csv, or json)",
                    path.display()
                ))
            }
        };

        for (lineno, row) in &rows {
            let at = |msg: String| format!("{}:{}: {msg}", path.display(), lineno);
            if row.len() != ncols {
                return Err(at(format!(
                    "`{}` expects {ncols} column(s), found {}",
                    inp.pred,
                    row.len()
                )));
            }
            let mut args: Vec<GroundVal> = Vec::with_capacity(arity);
            for (col, cell) in row[..arity].iter().enumerate() {
                let want_int = matches!(col_types.get(col), Some(ArgType::Int));
                let v = if want_int {
                    // An `int` column is an integer no matter the format —
                    // "5" in a file and 5 inline are the same value.
                    match cell {
                        Cell::Int(n) => GroundVal::Int(*n),
                        Cell::Text(s) => GroundVal::Int(s.trim().parse::<i64>().map_err(|_| {
                            at(format!(
                                "column {} of `{}` is `int`, got {s:?}",
                                col + 1,
                                inp.pred
                            ))
                        })?),
                        Cell::Float(f) => {
                            return Err(at(format!(
                                "number {f} is not an integer (floats belong only in a \
                                 neural predicate's trailing probability column)"
                            )))
                        }
                    }
                } else {
                    match cell {
                        Cell::Text(s) if s.is_empty() => {
                            return Err(at(format!(
                                "empty value in column {} of `{}`",
                                col + 1,
                                inp.pred
                            )))
                        }
                        Cell::Text(s) => GroundVal::Sym(checked.dict.intern(s)),
                        Cell::Int(n) => {
                            return Err(at(format!(
                                "column {} of `{}` is a symbol column, got the number {n}",
                                col + 1,
                                inp.pred
                            )))
                        }
                        Cell::Float(f) => {
                            return Err(at(format!(
                                "number {f} is not an integer (floats belong only in a \
                                 neural predicate's trailing probability column)"
                            )))
                        }
                    }
                };
                args.push(v);
            }

            if is_neural {
                let p = trailing_f64(&row[arity])
                    .ok_or_else(|| at(format!("`{}` needs a trailing probability", inp.pred)))?;
                if !(0.0..=1.0).contains(&p) {
                    return Err(at(format!("probability {p} is outside [0, 1]")));
                }
                new_prob_edb.push((inp.pred.clone(), args, p));
            } else if is_trop {
                let w = trailing_i64(&row[arity])
                    .ok_or_else(|| at(format!("`{}` needs a trailing integer weight", inp.pred)))?;
                new_edb.push(GroundFact {
                    pred: inp.pred.clone(),
                    args,
                    weight: Some(Weight::Finite(w)),
                });
            } else {
                new_edb.push(GroundFact {
                    pred: inp.pred.clone(),
                    args,
                    weight: None,
                });
            }
        }
    }
    // Every file and row validated — commit atomically.
    checked.edb.extend(new_edb);
    checked.prob_edb.extend(new_prob_edb);
    checked.inputs_loaded = true;
    Ok(())
}

fn trailing_i64(c: &Cell) -> Option<i64> {
    match c {
        Cell::Int(n) => Some(*n),
        Cell::Text(s) => s.trim().parse().ok(),
        Cell::Float(_) => None,
    }
}

fn trailing_f64(c: &Cell) -> Option<f64> {
    match c {
        Cell::Float(f) => Some(*f),
        Cell::Int(n) => Some(*n as f64),
        Cell::Text(s) => s.trim().parse().ok(),
    }
}

/// TSV/CSV rows. Tab-separated cells are taken verbatim (Soufflé convention);
/// comma-separated cells support double quotes with `""` escapes.
fn delimited_rows(path: &Path, delim: char) -> Result<Vec<(usize, Vec<Cell>)>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let cells = if delim == ',' {
            split_csv_line(line).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?
        } else {
            line.split(delim).map(String::from).collect()
        };
        rows.push((i + 1, cells.iter().map(|s| Cell::from_str(s)).collect()));
    }
    Ok(rows)
}

/// A minimal RFC-4180 field splitter: quoted fields, `""` escapes, no
/// embedded newlines (rows are lines). A quoted field must be *whole*: after
/// its closing quote only a delimiter or end-of-line may follow — trailing junk
/// (`"a"x,b`) or a bare quote inside an unquoted field is a load error, not
/// silently-accepted data (a corrupted export must fail loudly).
fn split_csv_line(line: &str) -> Result<Vec<String>, String> {
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    // The current field began with a quote (so a bare `"` inside it is illegal
    // unless doubled), and whether we have passed its closing quote.
    let mut quoted_field = false;
    let mut quote_closed = false;
    while let Some(c) = chars.next() {
        match (in_quotes, c) {
            (false, ',') => {
                cells.push(std::mem::take(&mut cur));
                quoted_field = false;
                quote_closed = false;
            }
            (false, '"') if cur.is_empty() && !quoted_field => {
                in_quotes = true;
                quoted_field = true;
            }
            // A quote anywhere else in an unquoted field, or after a quoted
            // field already closed, is malformed.
            (false, '"') => {
                return Err("unexpected `\"` in CSV field (quotes must wrap the \
                            whole field, and `\"` inside must be doubled)"
                    .to_string());
            }
            // After the closing quote of a quoted field, only a delimiter (the
            // arm above) may follow — anything else is trailing junk.
            (false, _) if quote_closed => {
                return Err("trailing characters after a closing CSV quote".to_string());
            }
            (true, '"') => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                    quote_closed = true;
                }
            }
            (_, c) => cur.push(c),
        }
    }
    if in_quotes {
        return Err("unterminated quoted CSV field".to_string());
    }
    cells.push(cur);
    Ok(cells)
}

/// JSON rows: `[[...], [...]]` — strings intern as symbols, integers stay
/// integers, floats are legal only where a trailing probability is expected.
fn json_rows(path: &Path) -> Result<Vec<(usize, Vec<Cell>)>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
    let serde_json::Value::Array(rows) = value else {
        return Err(format!("{}: expected a JSON array of rows", path.display()));
    };
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            let serde_json::Value::Array(cells) = row else {
                return Err(format!(
                    "{}:{}: a row must be an array",
                    path.display(),
                    i + 1
                ));
            };
            let parsed: Result<Vec<Cell>, String> = cells
                .into_iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => Ok(Cell::Text(s)),
                    serde_json::Value::Number(n) => {
                        if let Some(k) = n.as_i64() {
                            Ok(Cell::Int(k))
                        } else if let Some(f) = n.as_f64() {
                            Ok(Cell::Float(f))
                        } else {
                            Err(format!(
                                "{}:{}: unrepresentable number",
                                path.display(),
                                i + 1
                            ))
                        }
                    }
                    other => Err(format!(
                        "{}:{}: unsupported JSON value {other} (strings and numbers only)",
                        path.display(),
                        i + 1
                    )),
                })
                .collect();
            Ok((i + 1, parsed?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::split_csv_line;

    #[test]
    fn csv_accepts_wellformed_quoted_and_plain() {
        assert_eq!(split_csv_line("a,b").unwrap(), ["a", "b"]);
        assert_eq!(split_csv_line("\"a\",\"b\"").unwrap(), ["a", "b"]);
        // doubled quote is an escaped quote inside the field
        assert_eq!(split_csv_line("\"a\"\"b\",c").unwrap(), ["a\"b", "c"]);
        // empty quoted field
        assert_eq!(split_csv_line("\"\",b").unwrap(), ["", "b"]);
    }

    #[test]
    fn csv_rejects_trailing_junk_after_closing_quote() {
        // The Excel/script corruption case: `"a"junk` must be a load error,
        // not the silently-accepted constant `ajunk`.
        assert!(split_csv_line("\"a\"junk,b").is_err());
    }

    #[test]
    fn csv_rejects_bare_quote_in_unquoted_field() {
        assert!(split_csv_line("a\"b,c").is_err());
    }

    #[test]
    fn csv_rejects_unterminated_quote() {
        assert!(split_csv_line("\"a,b").is_err());
    }
}
